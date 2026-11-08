use std::{
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

use crate::events::EventBus;
use futures_util::{StreamExt, TryStreamExt};
use tokio::io::AsyncWriteExt;
use url::Url;

use crate::{
    db::validate_https_url,
    error::{AppError, AppResult},
    manifest::{check_file_at, check_game_file, resolve_manifest, secure_destination, FileCheck},
    models::{
        ErrorPayload, GameManifest, ManifestFile, PreparationReport, ProgressPayload, ReadyPayload,
        VerificationReport,
    },
    state::{AppState, OperationGuard},
};

pub fn http_client() -> AppResult<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("RADMIR-Standalone/0.1 (Linux launcher)")
        .connect_timeout(Duration::from_secs(10))
        .build()
        .map_err(AppError::from)
}

pub async fn verify_game(app: EventBus, state: Arc<AppState>) -> AppResult<VerificationReport> {
    let _guard = OperationGuard::begin(&state)?;
    let result = verify_inner(app.clone(), state.clone()).await;
    if let Err(error) = &result {
        emit_error(&app, error.to_string());
    }
    result
}

pub(crate) async fn verify_inner(
    app: EventBus,
    state: Arc<AppState>,
) -> AppResult<VerificationReport> {
    state.set_files_ready(None, false);
    let settings = state.settings()?;
    let game_dir = state.paths.game_dir(&settings)?;
    let client = http_client()?;
    emit(
        &app,
        ProgressPayload {
            phase: "manifest".into(),
            message: "Получение manifest…".into(),
            ..Default::default()
        },
    );
    let resolved = resolve_manifest(&settings, &game_dir, &client).await?;
    let manifest = resolved.manifest;
    let warning = resolved.warning;
    let scan = scan_files(
        &app,
        &state,
        &manifest,
        &game_dir,
        settings.hash_concurrency,
    )
    .await?;
    let ready = scan.invalid.is_empty();
    if ready {
        save_manifest(&game_dir, &manifest).await?;
    }
    state.set_files_ready(Some(manifest.build.clone()), ready);
    emit_ready(&app, &manifest.build, ready);
    Ok(scan.verification_report(manifest.build, warning))
}

pub async fn prepare_game(app: EventBus, state: Arc<AppState>) -> AppResult<PreparationReport> {
    let _guard = OperationGuard::begin(&state)?;
    let result = prepare_inner(app.clone(), state.clone()).await;
    if let Err(error) = &result {
        emit_error(&app, error.to_string());
    }
    result
}

pub(crate) async fn prepare_inner(
    app: EventBus,
    state: Arc<AppState>,
) -> AppResult<PreparationReport> {
    state.set_files_ready(None, false);
    let settings = state.settings()?;
    let game_dir = state.paths.game_dir(&settings)?;
    let client = http_client()?;
    emit(
        &app,
        ProgressPayload {
            phase: "manifest".into(),
            message: "Получение manifest…".into(),
            ..Default::default()
        },
    );
    let resolved = resolve_manifest(&settings, &game_dir, &client).await?;
    let manifest = resolved.manifest;
    let warning = resolved.warning;
    let scan = scan_files(
        &app,
        &state,
        &manifest,
        &game_dir,
        settings.hash_concurrency,
    )
    .await?;
    let total_files = scan.total_files;
    let valid_before = scan.valid_files;
    let missing_files = scan.missing_files;
    let changed_files = scan.changed_files;

    if scan.invalid.is_empty() {
        save_manifest(&game_dir, &manifest).await?;
        state.set_files_ready(Some(manifest.build.clone()), true);
        emit_ready(&app, &manifest.build, true);
        return Ok(PreparationReport {
            build: manifest.build,
            ready: true,
            total_files,
            valid_files: valid_before,
            missing_files: 0,
            changed_files: 0,
            downloaded_files: 0,
            message: warning.unwrap_or_else(|| "Все файлы соответствуют manifest.".into()),
        });
    }

    let Some(base_url) = resolved.base_url.as_deref() else {
        state.set_files_ready(None, false);
        emit_ready(&app, &manifest.build, false);
        return Ok(PreparationReport {
            build: manifest.build,
            ready: false,
            total_files,
            valid_files: valid_before,
            missing_files,
            changed_files,
            downloaded_files: 0,
            message: format!(
                "Нужно обновить {} файлов. Официальный CDN сейчас недоступен. {}",
                missing_files + changed_files,
                warning.unwrap_or_default()
            ),
        });
    };

    let to_download = scan.invalid.len();
    download_release(
        &state,
        &app,
        &client,
        &game_dir,
        scan.invalid,
        base_url,
        settings.download_concurrency,
        &resolved.archives,
    )
    .await?;
    save_manifest(&game_dir, &manifest).await?;
    state.set_files_ready(Some(manifest.build.clone()), true);
    emit_ready(&app, &manifest.build, true);
    emit(
        &app,
        ProgressPayload {
            phase: "ready".into(),
            files_done: total_files,
            files_total: total_files,
            percent: 100.0,
            message: "Проверка и обновление завершены.".into(),
            ..Default::default()
        },
    );
    Ok(PreparationReport {
        build: manifest.build,
        ready: true,
        total_files,
        valid_files: total_files,
        missing_files,
        changed_files,
        downloaded_files: to_download,
        message: warning.unwrap_or_else(|| format!("Обновлено файлов: {to_download}.")),
    })
}

struct ScanResult {
    total_files: usize,
    valid_files: usize,
    missing_files: usize,
    changed_files: usize,
    invalid: Vec<ManifestFile>,
}

impl ScanResult {
    fn verification_report(self, build: String, warning: Option<String>) -> VerificationReport {
        let ready = self.invalid.is_empty();
        VerificationReport {
            build,
            ready,
            total_files: self.total_files,
            valid_files: self.valid_files,
            missing_files: self.missing_files,
            changed_files: self.changed_files,
            message: warning.unwrap_or_else(|| {
                if ready {
                    "Все файлы соответствуют manifest.".into()
                } else {
                    format!(
                        "Отсутствует: {}, изменено: {}.",
                        self.missing_files, self.changed_files
                    )
                }
            }),
        }
    }
}

async fn scan_files(
    app: &EventBus,
    state: &Arc<AppState>,
    manifest: &GameManifest,
    game_dir: &Path,
    concurrency: usize,
) -> AppResult<ScanResult> {
    let total_files = manifest.files.len();
    let total_bytes = manifest.files.iter().map(|file| file.size).sum::<u64>();
    let started = Instant::now();
    emit(
        app,
        ProgressPayload {
            phase: "hashing".into(),
            files_total: total_files,
            bytes_total: total_bytes,
            message: "Параллельная проверка файлов…".into(),
            ..Default::default()
        },
    );

    let mut tasks = futures_util::stream::iter(manifest.files.iter().cloned().map(|file| {
        let root = game_dir.to_path_buf();
        tokio::task::spawn_blocking(move || {
            let result = check_game_file(&root, &file);
            (file, result)
        })
    }))
    .buffer_unordered(concurrency.clamp(1, 32));

    let mut result = ScanResult {
        total_files,
        valid_files: 0,
        missing_files: 0,
        changed_files: 0,
        invalid: Vec::new(),
    };
    let mut processed = 0_usize;
    let mut processed_bytes = 0_u64;
    while let Some(joined) = tasks.next().await {
        state.check_cancelled()?;
        let (file, check) = joined?;
        match check? {
            FileCheck::Valid => result.valid_files += 1,
            FileCheck::Missing => {
                result.missing_files += 1;
                result.invalid.push(file.clone());
            }
            FileCheck::Changed => {
                result.changed_files += 1;
                result.invalid.push(file.clone());
            }
        }
        processed += 1;
        processed_bytes = processed_bytes.saturating_add(file.size);
        if processed % 16 == 0 || processed == total_files {
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            emit(
                app,
                ProgressPayload {
                    phase: "hashing".into(),
                    current_file: Some(file.path),
                    bytes_done: processed_bytes,
                    bytes_total: total_bytes,
                    files_done: processed,
                    files_total: total_files,
                    bytes_per_second: processed_bytes as f64 / elapsed,
                    percent: processed as f64 * 100.0 / total_files as f64,
                    message: "Проверка MD5/SHA256…".into(),
                },
            );
        }
    }
    state.check_cancelled()?;
    Ok(result)
}

async fn save_manifest(root: &Path, manifest: &GameManifest) -> AppResult<()> {
    let file = ManifestFile {
        path: "manifest.json".into(),
        hash: String::new(),
        archive: -1,
        size: 0,
        url: None,
    };
    let destination = secure_destination(root, &file)?;
    let part = destination.with_file_name(format!(".manifest-{}.json", uuid::Uuid::new_v4()));
    tokio::fs::write(&part, serde_json::to_vec(manifest)?).await?;
    tokio::fs::rename(part, destination).await?;
    Ok(())
}

async fn download_release(
    state: &Arc<AppState>,
    app: &EventBus,
    client: &reqwest::Client,
    root: &Path,
    files: Vec<ManifestFile>,
    base: &str,
    concurrency: usize,
    archives: &std::collections::HashMap<i32, ManifestFile>,
) -> AppResult<()> {
    let mut direct = Vec::new();
    let mut groups = std::collections::BTreeMap::<i32, Vec<ManifestFile>>::new();
    for file in files {
        if file.archive == -1 || archives.is_empty() {
            direct.push(file);
        } else {
            groups.entry(file.archive).or_default().push(file);
        }
    }
    // Download each required ZIP exactly once, with its own manifest hash.
    // Stage only archives separately so a ZIP can never overwrite user extras.
    let archive_jobs: Vec<ManifestFile> = groups
        .keys()
        .map(|id| {
            archives
                .get(id)
                .cloned()
                .ok_or_else(|| AppError::new(format!("Не найден архив {id}")))
        })
        .collect::<AppResult<_>>()?;
    let jobs: Vec<_> = direct.iter().chain(archive_jobs.iter()).cloned().collect();
    let needed = jobs
        .iter()
        .map(|f| f.size)
        .sum::<u64>()
        .saturating_add(groups.values().flatten().map(|f| f.size).sum::<u64>());
    crate::locks::require_space(root, needed)?;
    let tracker = Arc::new(DownloadTracker::new(state.clone(), app.clone(), &jobs));
    tracker.emit(None, true, "Подготовка параллельной загрузки…");
    if !direct.is_empty() {
        download_files(client, root, direct, base, concurrency, tracker.clone()).await?;
    }
    if archive_jobs.is_empty() {
        return Ok(());
    }
    let stage_file = ManifestFile {
        path: format!(".radmir/update-{}/placeholder", uuid::Uuid::new_v4()),
        hash: String::new(),
        archive: -1,
        size: 0,
        url: None,
    };
    let stage = secure_destination(root, &stage_file)?
        .parent()
        .unwrap()
        .to_path_buf();
    let result: AppResult<()> = async {
        download_files(
            client,
            &stage,
            archive_jobs,
            base,
            concurrency,
            tracker.clone(),
        )
        .await?;
        for (id, files) in groups {
            state.check_cancelled()?;
            let archive = archives.get(&id).unwrap();
            let zip = crate::paths::safe_join(&stage, &archive.path)?;
            let target = root.to_path_buf();
            emit(
                app,
                ProgressPayload {
                    phase: "extracting".into(),
                    current_file: Some(archive.path.clone()),
                    message: "Распаковка и проверка игровых файлов…".into(),
                    ..Default::default()
                },
            );
            tokio::task::spawn_blocking(move || extract_selected(&zip, &target, &files)).await??;
        }
        Ok(())
    }
    .await;
    let _ = tokio::fs::remove_dir_all(&stage).await;
    result
}

fn extract_selected(zip_path: &Path, root: &Path, files: &[ManifestFile]) -> AppResult<()> {
    use std::io::{Read, Write};
    let mut archive = zip::ZipArchive::new(std::fs::File::open(zip_path)?)
        .map_err(|e| AppError::new(format!("ZIP: {e}")))?;
    let wanted: std::collections::HashMap<_, _> = files
        .iter()
        .map(|f| (f.path.replace('\\', "/"), f))
        .collect();
    let mut extracted = std::collections::HashSet::new();
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| AppError::new(format!("ZIP entry: {e}")))?;
        let name = entry.name().replace('\\', "/");
        crate::paths::safe_join(root, &name)?;
        if entry.is_dir() {
            continue;
        }
        if entry.unix_mode().is_some_and(|m| m & 0o170000 == 0o120000) {
            return Err(AppError::new("Symlink в ZIP запрещён."));
        }
        let Some(file) = wanted.get(&name) else {
            continue;
        };
        if !extracted.insert(name.clone()) || entry.size() != file.size {
            return Err(AppError::new(format!("Размер или дубликат ZIP: {name}")));
        }
        let destination = secure_destination(root, file)?;
        let part = destination.with_file_name(format!(".extract-{}", uuid::Uuid::new_v4()));
        let result: AppResult<()> = (|| {
            let mut output = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&part)?;
            let count = std::io::copy(
                &mut (&mut entry).take(file.size.saturating_add(1)),
                &mut output,
            )?;
            output.flush()?;
            drop(output);
            if count != file.size || check_file_at(&part, file)? != FileCheck::Valid {
                return Err(AppError::new(format!(
                    "Hash после распаковки не совпал: {name}"
                )));
            }
            std::fs::rename(&part, &destination)?;
            Ok(())
        })();
        let _ = std::fs::remove_file(part);
        result?;
    }
    if extracted.len() != wanted.len() {
        return Err(AppError::new("ZIP не содержит все требуемые файлы."));
    }
    Ok(())
}

async fn download_files(
    client: &reqwest::Client,
    game_dir: &Path,
    files: Vec<ManifestFile>,
    base_url: &str,
    concurrency: usize,
    tracker: Arc<DownloadTracker>,
) -> AppResult<()> {
    let mut downloads = futures_util::stream::iter(files.into_iter().map(|file| {
        let client = client.clone();
        let root = game_dir.to_path_buf();
        let tracker = tracker.clone();
        async move {
            download_with_retries(&client, &root, &file, base_url, &tracker).await?;
            tracker.complete_file(&file.path);
            Ok::<(), AppError>(())
        }
    }))
    .buffer_unordered(concurrency.clamp(1, 16));
    while let Some(result) = downloads.try_next().await? {
        let () = result;
    }
    Ok(())
}

async fn download_with_retries(
    client: &reqwest::Client,
    game_dir: &Path,
    file: &ManifestFile,
    base_url: &str,
    tracker: &DownloadTracker,
) -> AppResult<()> {
    let mut last_error = None;
    for attempt in 0..3_u32 {
        tracker.state.check_cancelled()?;
        let fallback = if attempt == 1 {
            crate::official::CDNS
                .iter()
                .enumerate()
                .find_map(|(index, host)| {
                    base_url
                        .strip_prefix(&format!("{host}/"))
                        .map(|suffix| format!("{}/{suffix}", crate::official::CDNS[1 - index]))
                })
        } else {
            None
        };
        match download_attempt(
            client,
            game_dir,
            file,
            fallback.as_deref().unwrap_or(base_url),
            tracker,
        )
        .await
        {
            Ok(_) => return Ok(()),
            Err((error, written)) => {
                tracker.rollback(written, &file.path);
                if error.code() == "cancelled" {
                    return Err(error);
                }
                last_error = Some(error);
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_millis(300 * u64::from(attempt + 1))).await;
                }
            }
        }
    }
    Err(last_error.unwrap_or_else(|| AppError::new("Неизвестная ошибка загрузки.")))
}

async fn download_attempt(
    client: &reqwest::Client,
    game_dir: &Path,
    file: &ManifestFile,
    base_url: &str,
    tracker: &DownloadTracker,
) -> Result<u64, (AppError, u64)> {
    let mut written = 0_u64;
    let result: AppResult<()> = async {
        let url = file_download_url(file, base_url)?;
        let destination = secure_destination(game_dir, file)?;
        let part_path = part_path(&destination)?;
        tracker.state.check_cancelled()?;
        let response = tokio::time::timeout(Duration::from_secs(60), client.get(url).send())
            .await
            .map_err(|_| AppError::coded("network_timeout", "CDN не отвечает 60 секунд."))??
            .error_for_status()?;
        let mut output = tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part_path)
            .await?;
        let mut stream = response.bytes_stream();
        while let Some(chunk) = tokio::time::timeout(Duration::from_secs(60), stream.next())
            .await
            .map_err(|_| {
                AppError::coded("network_timeout", "CDN не передаёт данные более 60 секунд.")
            })?
        {
            tracker.state.check_cancelled()?;
            let chunk = chunk?;
            if written.saturating_add(chunk.len() as u64) > file.size {
                return Err(AppError::new("Загрузка превышает размер из manifest."));
            }
            output.write_all(&chunk).await?;
            written += chunk.len() as u64;
            tracker.record(chunk.len() as u64, &file.path);
        }
        output.flush().await?;
        drop(output);

        let check_path = part_path.clone();
        let check_file = file.clone();
        let check =
            tokio::task::spawn_blocking(move || check_file_at(&check_path, &check_file)).await??;
        if check != FileCheck::Valid {
            return Err(AppError::new(format!(
                "Hash или размер после загрузки не совпал: {}",
                file.path
            )));
        }
        tokio::fs::rename(&part_path, &destination).await?;
        Ok(())
    }
    .await;

    match result {
        Ok(()) => Ok(written),
        Err(error) => {
            if let Ok(destination) = secure_destination(game_dir, file) {
                if let Ok(part) = part_path(&destination) {
                    let _ = tokio::fs::remove_file(part).await;
                }
            }
            Err((error, written))
        }
    }
}

fn part_path(destination: &Path) -> AppResult<PathBuf> {
    let name = destination
        .file_name()
        .ok_or_else(|| AppError::new("Некорректное имя файла."))?
        .to_string_lossy();
    Ok(destination.with_file_name(format!(".{name}.radmir-part")))
}

fn file_download_url(file: &ManifestFile, base_url: &str) -> AppResult<Url> {
    if let Some(url) = &file.url {
        return validate_https_url(url);
    }
    let mut url = validate_https_url(base_url)?;
    {
        let mut segments = url
            .path_segments_mut()
            .map_err(|_| AppError::new("Download base URL не поддерживает пути."))?;
        segments.pop_if_empty();
        for segment in file
            .path
            .replace('\\', "/")
            .split('/')
            .filter(|part| !part.is_empty())
        {
            segments.push(segment);
        }
    }
    Ok(url)
}

struct DownloadTracker {
    state: Arc<AppState>,
    app: EventBus,
    started: Instant,
    bytes_done: AtomicU64,
    files_done: AtomicUsize,
    bytes_total: u64,
    files_total: usize,
    last_emit: Mutex<Instant>,
}

impl DownloadTracker {
    fn new(state: Arc<AppState>, app: EventBus, files: &[ManifestFile]) -> Self {
        Self {
            state,
            app,
            started: Instant::now(),
            bytes_done: AtomicU64::new(0),
            files_done: AtomicUsize::new(0),
            bytes_total: files.iter().map(|file| file.size).sum(),
            files_total: files.len(),
            last_emit: Mutex::new(Instant::now() - Duration::from_secs(1)),
        }
    }

    fn record(&self, bytes: u64, current_file: &str) {
        self.bytes_done.fetch_add(bytes, Ordering::SeqCst);
        self.emit(Some(current_file), false, "Скачивание файлов…");
    }

    fn rollback(&self, bytes: u64, current_file: &str) {
        let mut current = self.bytes_done.load(Ordering::SeqCst);
        loop {
            let next = current.saturating_sub(bytes);
            match self.bytes_done.compare_exchange(
                current,
                next,
                Ordering::SeqCst,
                Ordering::SeqCst,
            ) {
                Ok(_) => break,
                Err(actual) => current = actual,
            }
        }
        self.emit(Some(current_file), true, "Повторная попытка загрузки…");
    }

    fn complete_file(&self, current_file: &str) {
        self.files_done.fetch_add(1, Ordering::SeqCst);
        self.emit(Some(current_file), true, "Файл проверен и установлен.");
    }

    fn emit(&self, current_file: Option<&str>, force: bool, message: &str) {
        if !force {
            let Ok(mut last) = self.last_emit.lock() else {
                return;
            };
            if last.elapsed() < Duration::from_millis(200) {
                return;
            }
            *last = Instant::now();
        }
        let bytes_done = self.bytes_done.load(Ordering::SeqCst);
        let files_done = self.files_done.load(Ordering::SeqCst);
        let elapsed = self.started.elapsed().as_secs_f64().max(0.001);
        let percent = if self.bytes_total > 0 {
            (bytes_done as f64 * 100.0 / self.bytes_total as f64).clamp(0.0, 100.0)
        } else {
            files_done as f64 * 100.0 / self.files_total.max(1) as f64
        };
        emit(
            &self.app,
            ProgressPayload {
                phase: "downloading".into(),
                current_file: current_file.map(str::to_string),
                bytes_done,
                bytes_total: self.bytes_total,
                files_done,
                files_total: self.files_total,
                bytes_per_second: bytes_done as f64 / elapsed,
                percent,
                message: message.into(),
            },
        );
    }
}

fn emit(app: &EventBus, payload: ProgressPayload) {
    if let Err(error) = app.emit("launcher://progress", payload) {
        eprintln!("Не удалось отправить progress event: {error}");
    }
}

fn emit_ready(app: &EventBus, build: &str, ready: bool) {
    let _ = app.emit(
        "files://ready",
        ReadyPayload {
            build: build.into(),
            ready,
        },
    );
}

fn emit_error(app: &EventBus, message: String) {
    let _ = app.emit(
        "launcher://error",
        ErrorPayload {
            message,
            retryable: true,
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn archive_extracts_only_verified_manifest_members_and_rejects_traversal() {
        use std::io::Write;
        let root = std::env::temp_dir().join(format!("radmir-zip-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let archive = root.join("source.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file("data/hello.bin", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"hello").unwrap();
        zip.start_file("unwanted.bin", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"extra").unwrap();
        zip.finish().unwrap();
        let file = ManifestFile {
            path: "data/hello.bin".into(),
            hash: "5d41402abc4b2a76b9719d911017c592".into(),
            size: 5,
            archive: 0,
            url: None,
        };
        extract_selected(&archive, &root.join("game"), &[file.clone()]).unwrap();
        assert_eq!(
            std::fs::read(root.join("game/data/hello.bin")).unwrap(),
            b"hello"
        );
        assert!(!root.join("game/unwanted.bin").exists());
        let mut bad = file.clone();
        bad.hash = "00000000000000000000000000000000".into();
        assert!(extract_selected(&archive, &root.join("bad"), &[bad]).is_err());
        assert!(!root.join("bad/data/hello.bin").exists());
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&archive).unwrap());
        zip.start_file("../escape", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"bad").unwrap();
        zip.finish().unwrap();
        assert!(extract_selected(&archive, &root.join("game"), &[file]).is_err());
        assert!(!root.join("escape").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn direct_url_preserves_spaces_and_special_characters() {
        let file = ManifestFile {
            hash: "5d41402abc4b2a76b9719d911017c592".into(),
            path: "SAMP/my file #1.bin".into(),
            archive: -1,
            size: 5,
            url: None,
        };
        let url = file_download_url(&file, "https://cdn.example.com/game/").unwrap();
        assert_eq!(url.host_str(), Some("cdn.example.com"));
        assert!(url.path().ends_with("/game/SAMP/my%20file%20%231.bin"));
    }

    #[test]
    fn rejects_non_https_download_sources() {
        let file = ManifestFile {
            hash: "5d41402abc4b2a76b9719d911017c592".into(),
            path: "file.bin".into(),
            archive: -1,
            size: 5,
            url: None,
        };
        assert!(file_download_url(&file, "http://cdn.example.com").is_err());
    }
}
