//! Verified official runtime downloads and transactional component activation.
use crate::{
    error::{AppError, AppResult},
    manifest::{hash_path, HashAlgorithm},
    models::{ProgressPayload, RuntimeVersion},
    state::AppState,
};
use futures_util::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::io::AsyncWriteExt;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Component {
    pub id: String,
    pub version: String,
    pub url: String,
    pub sha256: String,
    pub size: u64,
    pub target: String,
    pub archive_root: String,
    #[serde(default)]
    pub mirrors: Vec<String>,
}
#[derive(Deserialize)]
struct ComponentLock {
    components: Vec<Component>,
}
#[derive(Serialize, Deserialize)]
struct Receipt {
    files: Vec<(String, u64, String)>,
    #[serde(default)]
    links: Vec<(String, String)>,
}

pub fn locked() -> Vec<Component> {
    serde_json::from_str::<ComponentLock>(include_str!("../components.lock.json"))
        .expect("compiled component lock")
        .components
}
pub fn component(id: &str) -> AppResult<Component> {
    locked()
        .into_iter()
        .find(|c| c.id == id)
        .ok_or_else(|| AppError::new(format!("Неизвестный компонент {id}")))
}
fn check_hash(path: &Path, expected: &str) -> bool {
    hash_path(path, HashAlgorithm::Sha256).is_ok_and(|h| h.eq_ignore_ascii_case(expected))
}

pub async fn cached_archive(state: &Arc<AppState>, spec: &Component) -> AppResult<PathBuf> {
    cached_archive_policy(state, spec, Duration::from_secs(60)).await
}
fn validate_source(url: &str) -> AppResult<()> {
    #[cfg(test)]
    if url.starts_with("http://127.0.0.1:") {
        return Ok(());
    }
    crate::db::validate_https_url(url)?;
    Ok(())
}
async fn cached_archive_policy(
    state: &Arc<AppState>,
    spec: &Component,
    inactivity: Duration,
) -> AppResult<PathBuf> {
    if spec.sha256.len() != 64 || !spec.sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(AppError::coded(
            "missing_checksum",
            "Официальная сборка не имеет проверенной SHA256.",
        ));
    }
    let cache = state.paths.cache_dir.join("components");
    fs::create_dir_all(&cache)?;
    let name = spec
        .url
        .rsplit('/')
        .next()
        .ok_or_else(|| AppError::new("Некорректный URL компонента."))?;
    let archive = cache.join(format!("{}-{name}", spec.sha256));
    if archive.is_file() && check_hash(&archive, &spec.sha256) {
        return Ok(archive);
    }
    crate::locks::require_space(&cache, spec.size)?;
    let client = crate::updater::http_client()?;
    let mut urls = vec![spec.url.clone()];
    urls.extend(spec.mirrors.clone());
    let mut last = None;
    for attempt in 0..3 {
        state.check_cancelled()?;
        let url = &urls[attempt % urls.len()];
        validate_source(url)?;
        let part = archive.with_extension(format!("part-{}", uuid::Uuid::new_v4()));
        let result: AppResult<()> = async {
            let response = tokio::time::timeout(inactivity, client.get(url).send())
                .await
                .map_err(|_| AppError::coded("network_timeout", "Источник runtime не отвечает."))??
                .error_for_status()?;
            let limit = if spec.size > 0 {
                spec.size
            } else {
                4 * 1024 * 1024 * 1024
            };
            let total = response.content_length().unwrap_or(spec.size);
            let mut stream = response.bytes_stream();
            let mut output = tokio::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(&part)
                .await?;
            let mut hash = Sha256::new();
            let mut done = 0u64;
            let start = Instant::now();
            let mut last_emit = Instant::now();
            while let Some(chunk) = tokio::time::timeout(inactivity, stream.next())
                .await
                .map_err(|_| {
                    AppError::coded(
                        "network_timeout",
                        "Загрузка runtime остановилась: нет данных 60 секунд.",
                    )
                })?
            {
                state.check_cancelled()?;
                let chunk = chunk?;
                done = done.saturating_add(chunk.len() as u64);
                if done > limit {
                    return Err(AppError::coded(
                        "download_size",
                        "Размер компонента превышает официальный размер.",
                    ));
                }
                output.write_all(&chunk).await?;
                hash.update(&chunk);
                if last_emit.elapsed() > Duration::from_millis(200) {
                    let _ = state.events.emit(
                        "launcher://progress",
                        ProgressPayload {
                            phase: "runtime-download".into(),
                            current_file: Some(spec.version.clone()),
                            bytes_done: done,
                            bytes_total: total,
                            bytes_per_second: done as f64
                                / start.elapsed().as_secs_f64().max(0.001),
                            percent: if total > 0 {
                                done as f64 * 100.0 / total as f64
                            } else {
                                0.0
                            },
                            message: format!("Загрузка {}…", spec.version),
                            ..Default::default()
                        },
                    );
                    last_emit = Instant::now();
                }
            }
            output.sync_all().await?;
            drop(output);
            if !hex::encode(hash.finalize()).eq_ignore_ascii_case(&spec.sha256) {
                return Err(AppError::coded(
                    "hash_mismatch",
                    format!(
                        "SHA256 {} не совпала; компонент не установлен.",
                        spec.version
                    ),
                ));
            }
            tokio::fs::rename(&part, &archive).await?;
            Ok(())
        }
        .await;
        let _ = tokio::fs::remove_file(&part).await;
        match result {
            Ok(()) => return Ok(archive),
            Err(e) => {
                let cancelled = e.code() == "cancelled";
                if cancelled {
                    return Err(e);
                }
                last = Some(e);
            }
        }
    }
    let last = last.unwrap_or_else(|| AppError::new("Источник недоступен."));
    if ["hash_mismatch", "download_size", "filesystem_error"].contains(&last.code()) {
        return Err(last);
    }
    Err(AppError::coded(
        "runtime_source_unavailable",
        format!(
            "Не удалось загрузить {} после трёх попыток. {last}",
            spec.version
        ),
    ))
}

fn safe_archive_path(path: &Path) -> bool {
    !path.is_absolute()
        && path.components().all(|c| {
            matches!(
                c,
                std::path::Component::Normal(_) | std::path::Component::CurDir
            )
        })
}
fn link_inside(path: &Path, target: &Path, hard: bool) -> bool {
    if target.is_absolute() {
        return false;
    }
    let mut depth = if hard {
        0
    } else {
        path.parent().map(|p| p.components().count()).unwrap_or(0)
    };
    for c in target.components() {
        match c {
            std::path::Component::ParentDir => {
                if depth == 0 {
                    return false;
                }
                depth -= 1;
            }
            std::path::Component::Normal(_) => depth += 1,
            std::path::Component::CurDir => {}
            _ => return false,
        }
    }
    true
}
pub fn extract_archive(archive: &Path, stage: &Path) -> AppResult<()> {
    fs::create_dir_all(stage)?;
    let file = fs::File::open(archive)?;
    let mut magic = [0u8; 6];
    let mut header = fs::File::open(archive)?;
    let count = header.read(&mut magic)?;
    let reader: Box<dyn Read> = if count >= 2 && magic[..2] == [0x1f, 0x8b] {
        Box::new(flate2::read::GzDecoder::new(file))
    } else if count == 6 && magic == [0xfd, b'7', b'z', b'X', b'Z', 0] {
        Box::new(xz2::read::XzDecoder::new(file))
    } else {
        Box::new(file)
    };
    let mut tar = tar::Archive::new(reader);
    for entry in tar.entries()? {
        let mut entry = entry?;
        // GitHub tarballs carry a global PAX comment header. It is metadata,
        // never a filesystem entry; tar applies local path extensions itself.
        if entry.header().entry_type().is_pax_global_extensions() {
            continue;
        }
        let path = entry.path()?.into_owned();
        if !safe_archive_path(&path) {
            return Err(AppError::coded(
                "unsafe_archive",
                "Путь архива выходит за каталог компонента.",
            ));
        }
        let kind = entry.header().entry_type();
        if !(kind.is_file() || kind.is_dir() || kind.is_symlink() || kind.is_hard_link()) {
            return Err(AppError::coded(
                "unsafe_archive",
                "Специальные устройства в архиве запрещены.",
            ));
        }
        if let Some(link) = entry.link_name()? {
            if !link_inside(&path, &link, kind.is_hard_link()) {
                return Err(AppError::coded(
                    "unsafe_archive",
                    "Ссылка архива выходит за каталог компонента.",
                ));
            }
        }
        let mut ancestor = PathBuf::new();
        if let Some(parent) = path.parent() {
            for c in parent.components() {
                ancestor.push(c);
                if fs::symlink_metadata(stage.join(&ancestor))
                    .is_ok_and(|m| m.file_type().is_symlink())
                {
                    return Err(AppError::coded(
                        "unsafe_archive",
                        "Запись через symlink архива запрещена.",
                    ));
                }
            }
        }
        if !entry.unpack_in(stage)? {
            return Err(AppError::coded(
                "unsafe_archive",
                "Небезопасная запись архива.",
            ));
        }
    }
    Ok(())
}
fn receipt(root: &Path) -> AppResult<Receipt> {
    fn visit(root: &Path, current: &Path, result: &mut Receipt) -> AppResult<()> {
        for entry in fs::read_dir(current)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = fs::symlink_metadata(&path)?;
            let relative = path.strip_prefix(root).unwrap();
            if metadata.is_dir() {
                visit(root, &path, result)?;
            } else if metadata.is_file() {
                result.files.push((
                    relative.to_string_lossy().into(),
                    metadata.len(),
                    hash_path(&path, HashAlgorithm::Sha256)?,
                ));
            } else if metadata.file_type().is_symlink() {
                let target = fs::read_link(&path)?;
                if !link_inside(relative, &target, false) {
                    return Err(AppError::coded(
                        "unsafe_archive",
                        "Ссылка выходит за активируемый компонент.",
                    ));
                }
                result.links.push((
                    relative.to_string_lossy().into(),
                    target.to_string_lossy().into(),
                ));
            }
        }
        Ok(())
    }
    let mut result = Receipt {
        files: Vec::new(),
        links: Vec::new(),
    };
    visit(root, root, &mut result)?;
    Ok(result)
}
fn matches_receipt(root: &Path, receipt: &Receipt) -> bool {
    !receipt.files.is_empty()
        && receipt.files.iter().all(|(path, size, hash)| {
            let p = root.join(path);
            fs::symlink_metadata(&p)
                .is_ok_and(|m| m.is_file() && !m.file_type().is_symlink() && m.len() == *size)
                && check_hash(&p, hash)
        })
        && receipt.links.iter().all(|(path, target)| {
            fs::read_link(root.join(path)).is_ok_and(|v| v == Path::new(target))
        })
}

// Paths from the successful component verification, excluding upstream's
// mutable caches/configuration. Warm checks stat these, rather than rehashing
// several GiB of already verified runtime on every window open.
pub fn tracked_runtime_paths(state: &Arc<AppState>, root: &Path) -> AppResult<Vec<PathBuf>> {
    let settings = state.settings()?;
    let mut paths = Vec::new();
    for spec in locked()
        .into_iter()
        .filter(|s| s.id != "proton" || s.version == settings.wine_build)
    {
        let text = state
            .db
            .lock()
            .map_err(|_| AppError::new("Блокировка БД"))?
            .component_receipt(&root.to_string_lossy(), &spec.id, &spec.version)?;
        if let Some(text) = text {
            let receipt: Receipt = serde_json::from_str(&text)?;
            let base = if spec.id == "scripts" {
                root.join("data")
            } else {
                root.join(&spec.target).join(&spec.archive_root)
            };
            for relative in receipt
                .files
                .into_iter()
                .map(|p| p.0)
                .chain(receipt.links.into_iter().map(|p| p.0))
            {
                let first = relative.split('/').next().unwrap_or("");
                if spec.id == "scripts"
                    && ["user.conf", "prefixes", "dist", "tmp", "backups"].contains(&first)
                    || spec.id == "libs" && first == "var"
                {
                    continue;
                }
                paths.push(crate::paths::safe_join(&base, &relative)?);
            }
        }
    }
    // Locally installed nondefault Wine need not have a launcher receipt.
    fn visit(path: &Path, paths: &mut Vec<PathBuf>) -> AppResult<()> {
        for e in fs::read_dir(path)? {
            let p = e?.path();
            let m = fs::symlink_metadata(&p)?;
            if m.is_dir() {
                visit(&p, paths)?;
            } else {
                paths.push(p);
            }
        }
        Ok(())
    }
    if settings.wine_build != "PROTON_LG_10-28" {
        visit(
            &root.join("data/dist").join(&settings.wine_build),
            &mut paths,
        )?;
    }
    paths.sort();
    paths.dedup();
    Ok(paths)
}

pub async fn install_component(
    state: &Arc<AppState>,
    root: &Path,
    spec: &Component,
) -> AppResult<()> {
    state.check_cancelled()?;
    let destination = if spec.id == "scripts" {
        root.join("data")
    } else {
        root.join(&spec.target).join(&spec.archive_root)
    };
    let root_name = root.to_string_lossy().into_owned();
    let previous = state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .component_receipt(&root_name, &spec.id, &spec.version)?;
    if let Some(text) = previous {
        if let Ok(r) = serde_json::from_str::<Receipt>(&text) {
            let d = destination.clone();
            if tokio::task::spawn_blocking(move || matches_receipt(&d, &r)).await? {
                return Ok(());
            }
        }
    }
    let archive = cached_archive(state, spec).await?;
    crate::locks::require_space(root, spec.size.saturating_mul(8))?;
    let stage = root
        .join("data/tmp")
        .join(format!(".install-{}", uuid::Uuid::new_v4()));
    let archive_copy = archive.clone();
    let stage_copy = stage.clone();
    let _ = state.events.emit(
        "launcher://progress",
        ProgressPayload {
            phase: "runtime-extract".into(),
            current_file: Some(spec.version.clone()),
            message: format!("Проверка и распаковка {}…", spec.version),
            ..Default::default()
        },
    );
    if let Err(error) =
        tokio::task::spawn_blocking(move || extract_archive(&archive_copy, &stage_copy)).await?
    {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    if let Err(error) = state.check_cancelled() {
        let _ = fs::remove_dir_all(&stage);
        return Err(error);
    }
    let source = stage.join(&spec.archive_root);
    if !source.is_dir() {
        let _ = fs::remove_dir_all(&stage);
        return Err(AppError::coded(
            "archive_layout",
            format!("Архив {} не содержит ожидаемый каталог.", spec.version),
        ));
    }
    if spec.target == "data/dist"
        && ![source.join("bin/wine"), source.join("files/bin/wine")]
            .iter()
            .any(|p| p.is_file())
    {
        let _ = fs::remove_dir_all(&stage);
        return Err(AppError::coded(
            "archive_layout",
            "В архиве выбранной сборки нет Wine.",
        ));
    }
    let source_clone = source.clone();
    let r = tokio::task::spawn_blocking(move || receipt(&source_clone)).await??;
    let destination_copy = destination.clone();
    let expected = serde_json::to_string(&r)?;
    let r_copy = serde_json::from_str::<Receipt>(&expected)?;
    if !tokio::task::spawn_blocking(move || matches_receipt(&destination_copy, &r_copy)).await? {
        let backup =
            root.join("data/backups")
                .join(format!("{}-{}", spec.id, uuid::Uuid::new_v4()));
        fs::create_dir_all(&backup)?;
        if spec.id == "scripts" {
            fs::create_dir_all(&destination)?;
            for entry in fs::read_dir(&source)? {
                let entry = entry?;
                let name = entry.file_name();
                if ["user.conf", "prefixes", "dist", "tmp", "backups"]
                    .iter()
                    .any(|v| name == std::ffi::OsStr::new(v))
                {
                    continue;
                }
                let target = destination.join(&name);
                if target.exists() || fs::symlink_metadata(&target).is_ok() {
                    fs::rename(&target, backup.join(&name))?;
                }
                if let Err(error) = fs::rename(entry.path(), &target) {
                    let _ = fs::rename(backup.join(&name), &target);
                    return Err(error.into());
                }
            }
        } else {
            fs::create_dir_all(destination.parent().unwrap())?;
            let old = backup.join("previous");
            if destination.exists() {
                fs::rename(&destination, &old)?;
            }
            if let Err(error) = fs::rename(&source, &destination) {
                let _ = fs::rename(&old, &destination);
                return Err(error.into());
            }
        }
    }
    state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .record_component(&root_name, &spec.id, &spec.version, &spec.sha256, &expected)?;
    let _ = fs::remove_dir_all(stage);
    Ok(())
}

#[derive(Clone, Serialize, Deserialize)]
pub struct CatalogItem {
    pub id: String,
    pub url: String,
    pub checksum: Option<String>,
    pub size: u64,
}
pub async fn catalog(state: &Arc<AppState>) -> AppResult<Vec<CatalogItem>> {
    let cache = state.paths.cache_dir.join("wine-catalog.json");
    let client = crate::updater::http_client()?;
    let mut result = BTreeMap::new();
    let default = component("proton")?;
    result.insert(
        default.version.clone(),
        CatalogItem {
            id: default.version,
            url: default.url,
            checksum: Some(default.sha256),
            size: default.size,
        },
    );
    if let Ok(text) = fs::read(&cache) {
        if let Ok(saved) = serde_json::from_slice::<Vec<CatalogItem>>(&text) {
            for item in saved {
                if crate::db::validate_https_url(&item.url).is_ok() {
                    result.insert(item.id.clone(), item);
                }
            }
        }
    }
    let url = "https://api.github.com/repos/Castro-Fidel/wine_builds/releases?per_page=100";
    if let Ok(Ok(response)) =
        tokio::time::timeout(Duration::from_secs(12), client.get(url).send()).await
    {
        if response.status().is_success() {
            if let Ok(Ok(value)) = tokio::time::timeout(
                Duration::from_secs(12),
                response.json::<serde_json::Value>(),
            )
            .await
            {
                if let Some(releases) = value.as_array() {
                    for release in releases {
                        if let Some(assets) = release["assets"].as_array() {
                            for asset in assets {
                                let name = asset["name"].as_str().unwrap_or("");
                                if !(name.starts_with("PROTON_") || name.starts_with("WINE_"))
                                    || !name.ends_with(".tar.xz")
                                {
                                    continue;
                                }
                                let Some(url) = asset["browser_download_url"].as_str() else {
                                    continue;
                                };
                                let digest = asset["digest"]
                                    .as_str()
                                    .and_then(|d| d.strip_prefix("sha256:"))
                                    .map(str::to_string);
                                let id = name.trim_end_matches(".tar.xz").to_string();
                                result.entry(id.clone()).or_insert(CatalogItem {
                                    id,
                                    url: url.into(),
                                    checksum: digest,
                                    size: asset["size"].as_u64().unwrap_or(0),
                                });
                            }
                        }
                    }
                }
            }
        }
    }
    let items: Vec<_> = result.into_values().collect();
    let _ = fs::write(cache, serde_json::to_vec(&items)?);
    Ok(items)
}
pub async fn versions(state: &Arc<AppState>) -> AppResult<Vec<RuntimeVersion>> {
    let root = crate::portproton::native_root(&state.settings()?);
    let mut result = BTreeMap::new();
    for item in catalog(state).await? {
        let installed = crate::portproton::wine_binary(&root, &item.id).is_some();
        result.insert(
            item.id.clone(),
            RuntimeVersion {
                id: item.id.clone(),
                label: item.id,
                installed,
                downloadable: true,
                source: "Официальный PortProton".into(),
            },
        );
    }
    if let Ok(entries) = fs::read_dir(root.join("data/dist")) {
        for entry in entries.flatten() {
            let id = entry.file_name().to_string_lossy().into_owned();
            if crate::portproton::wine_binary(&root, &id).is_some() {
                result.entry(id.clone()).or_insert(RuntimeVersion {
                    id: id.clone(),
                    label: id,
                    installed: true,
                    downloadable: false,
                    source: "Установлена локально".into(),
                });
            }
        }
    }
    Ok(result.into_values().collect())
}
pub async fn selected_wine(state: &Arc<AppState>, root: &Path, id: &str) -> AppResult<()> {
    if id == "PROTON_LG_10-28" {
        return install_component(state, root, &component("proton")?).await;
    }
    if crate::portproton::wine_binary(root, id).is_some() {
        return Ok(());
    }
    let item = catalog(state)
        .await?
        .into_iter()
        .find(|v| v.id == id)
        .ok_or_else(|| {
            AppError::coded(
                "wine_version_unavailable",
                "Выбранная сборка отсутствует в установленном и официальном каталоге.",
            )
        })?;
    let hash = if let Some(h) = item.checksum {
        h
    } else {
        let checksum_url = item.url.trim_end_matches(".tar.xz").to_string() + ".sha256sum";
        let response = crate::updater::http_client()?
            .get(checksum_url)
            .timeout(Duration::from_secs(30))
            .send()
            .await?
            .error_for_status()?;
        if response.content_length().is_some_and(|n| n > 4096) {
            return Err(AppError::new("Некорректная контрольная сумма."));
        }
        response
            .text()
            .await?
            .split_whitespace()
            .next()
            .unwrap_or("")
            .into()
    };
    install_component(
        state,
        root,
        &Component {
            id: format!("wine-{id}"),
            version: id.into(),
            url: item.url,
            sha256: hash,
            size: item.size,
            target: "data/dist".into(),
            archive_root: id.into(),
            mirrors: Vec::new(),
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn component_manifest_pins_every_hash_and_uses_official_https() {
        for c in locked() {
            assert_eq!(c.sha256.len(), 64);
            assert!(c.sha256.bytes().all(|b| b.is_ascii_hexdigit()));
            assert!(
                c.url.starts_with("https://github.com/Castro-Fidel/")
                    || c.url
                        .starts_with("https://codeload.github.com/Castro-Fidel/")
            );
        }
    }
    #[test]
    fn extraction_rejects_paths_and_symlinks_outside_stage() {
        assert!(!safe_archive_path(Path::new("../escape")));
        assert!(!safe_archive_path(Path::new("/etc/passwd")));
        assert!(!link_inside(
            Path::new("folder/link"),
            Path::new("../../escape"),
            false
        ));
        assert!(link_inside(
            Path::new("folder/link"),
            Path::new("../inside"),
            false
        ));
    }
    #[test]
    fn archive_symlink_cannot_redirect_a_later_file() {
        let root = std::env::temp_dir().join(format!("radmir-tar-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let path = root.join("bad.tar");
        let mut builder = tar::Builder::new(fs::File::create(&path).unwrap());
        let mut h = tar::Header::new_gnu();
        h.set_entry_type(tar::EntryType::Symlink);
        h.set_size(0);
        h.set_mode(0o777);
        h.set_link_name("../outside").unwrap();
        h.set_cksum();
        builder
            .append_data(&mut h, "link", std::io::empty())
            .unwrap();
        builder.finish().unwrap();
        assert!(extract_archive(&path, &root.join("stage")).is_err());
        assert!(!root.join("outside").exists());
        fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod download_tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    async fn server(responses: Vec<Vec<u8>>) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/fixture.tar", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            for response in responses {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 4096];
                let _ = stream.read(&mut request).await;
                stream.write_all(&response).await.unwrap();
            }
        });
        (url, task)
    }
    fn response(body: &[u8]) -> Vec<u8> {
        [
            format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            )
            .into_bytes(),
            body.to_vec(),
        ]
        .concat()
    }
    fn spec(url: String, body: &[u8]) -> Component {
        Component {
            id: "fixture".into(),
            version: "test".into(),
            url,
            sha256: hex::encode(Sha256::digest(body)),
            size: body.len() as u64,
            target: "data/tmp".into(),
            archive_root: "test".into(),
            mirrors: Vec::new(),
        }
    }
    #[tokio::test]
    async fn unavailable_primary_uses_verified_mirror_and_cache() {
        let f = crate::test_support::Fixture::new();
        let body = b"verified archive";
        let (mirror, task) = server(vec![response(body)]).await;
        let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/fixture.tar", dead.local_addr().unwrap());
        drop(dead);
        let mut c = spec(url, body);
        c.mirrors.push(mirror);
        let path = cached_archive(&f.state, &c).await.unwrap();
        assert_eq!(fs::read(&path).unwrap(), body);
        task.await.unwrap();
        assert_eq!(cached_archive(&f.state, &c).await.unwrap(), path);
    }
    #[tokio::test]
    async fn hash_mismatch_never_activates_and_cleans_temporary_files() {
        let f = crate::test_support::Fixture::new();
        let (url, task) = server(vec![response(b"evil"); 3]).await;
        let c = spec(url, b"good");
        let error = cached_archive(&f.state, &c).await.unwrap_err();
        assert_eq!(error.code(), "hash_mismatch");
        task.await.unwrap();
        assert_eq!(
            fs::read_dir(f.state.paths.cache_dir.join("components"))
                .unwrap()
                .count(),
            0
        );
    }
    #[tokio::test]
    async fn offline_sources_stop_after_three_attempts() {
        let f = crate::test_support::Fixture::new();
        let (url, task) = server(vec![
                b"HTTP/1.1 503 Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    .to_vec();
                3
            ])
        .await;
        let error = cached_archive(&f.state, &spec(url, b"good"))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "runtime_source_unavailable");
        task.await.unwrap();
    }
    #[tokio::test]
    async fn stalled_download_times_out_and_cancellation_blocks_download() {
        let f = crate::test_support::Fixture::new();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/fixture.tar", listener.local_addr().unwrap());
        let task = tokio::spawn(async move {
            for _ in 0..3 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut b = [0; 4096];
                let _ = stream.read(&mut b).await;
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 4\r\n\r\ng")
                    .await
                    .unwrap();
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        });
        let c = spec(url, b"good");
        let error = cached_archive_policy(&f.state, &c, Duration::from_millis(50))
            .await
            .unwrap_err();
        assert_eq!(error.code(), "runtime_source_unavailable");
        task.abort();
        f.state
            .cancelled
            .store(true, std::sync::atomic::Ordering::SeqCst);
        assert_eq!(
            cached_archive(&f.state, &c).await.unwrap_err().code(),
            "cancelled"
        );
    }
    #[tokio::test]
    async fn interrupted_or_corrupt_component_is_restored_without_touching_prefix() {
        use std::os::unix::fs::symlink;
        let f = crate::test_support::Fixture::new();
        let root = f.root.join("PortProton");
        let archive = f.root.join("fixture.tar");
        let mut tar = tar::Builder::new(fs::File::create(&archive).unwrap());
        let mut header = tar::Header::new_gnu();
        header.set_size(4);
        header.set_mode(0o755);
        header.set_cksum();
        tar.append_data(&mut header, "test/bin/wine", &b"wine"[..])
            .unwrap();
        tar.finish().unwrap();
        drop(tar);
        let body = fs::read(&archive).unwrap();
        let c = Component {
            target: "data/dist".into(),
            ..spec("https://github.com/Castro-Fidel/fixture.tar".into(), &body)
        };
        let cache = f.state.paths.cache_dir.join("components");
        fs::create_dir_all(&cache).unwrap();
        fs::copy(&archive, cache.join(format!("{}-fixture.tar", c.sha256))).unwrap();
        let game = root.join("data/prefixes/DEFAULT/drive_c/game");
        fs::create_dir_all(&game).unwrap();
        fs::write(game.join("keep"), "game").unwrap();
        install_component(&f.state, &root, &c).await.unwrap();
        let wine = root.join("data/dist/test/bin/wine");
        fs::remove_file(&wine).unwrap();
        symlink("/dev/null", &wine).unwrap();
        install_component(&f.state, &root, &c).await.unwrap();
        assert_eq!(fs::read(&wine).unwrap(), b"wine");
        assert_eq!(fs::read(game.join("keep")).unwrap(), b"game");
    }
}

#[cfg(test)]
mod pax_tests {
    use super::*;
    #[test]
    fn github_global_pax_metadata_does_not_block_scripts() {
        let f = crate::test_support::Fixture::new();
        let archive = f.root.join("github.tar");
        let mut builder = tar::Builder::new(fs::File::create(&archive).unwrap());
        let data = b"20 comment=metadata\n";
        let mut global = tar::Header::new_gnu();
        global.set_entry_type(tar::EntryType::new(b'g'));
        global.set_mode(0o644);
        global.set_size(data.len() as u64);
        global.set_cksum();
        builder
            .append_data(&mut global, "pax_global_header", &data[..])
            .unwrap();
        let mut file = tar::Header::new_gnu();
        file.set_mode(0o755);
        file.set_size(4);
        file.set_cksum();
        builder
            .append_data(&mut file, "scripts/start.sh", &b"bash"[..])
            .unwrap();
        builder.finish().unwrap();
        drop(builder);
        let stage = f.root.join("stage");
        extract_archive(&archive, &stage).unwrap();
        assert_eq!(fs::read(stage.join("scripts/start.sh")).unwrap(), b"bash");
        assert!(!stage.join("pax_global_header").exists());
    }
}

#[cfg(test)]
mod official_archive_tests {
    use super::*;
    #[test]
    #[ignore = "requires previously downloaded and SHA256-verified official archives"]
    fn official_archives_extract_into_the_pinned_layout() {
        let downloads = PathBuf::from(
            std::env::var("RADMIR_COMPONENT_FIXTURE_DIR").expect("fixture directory"),
        );
        let f = crate::test_support::Fixture::new();
        for spec in locked() {
            let archive = if spec.id == "scripts" {
                downloads.parent().unwrap().join("scripts.tar.gz")
            } else {
                downloads.join(format!("{}.tar.xz", spec.id))
            };
            assert!(check_hash(&archive, &spec.sha256), "{} checksum", spec.id);
            let stage = f.root.join(&spec.id);
            extract_archive(&archive, &stage).unwrap();
            let source = stage.join(&spec.archive_root);
            assert!(source.is_dir(), "{} layout", spec.id);
            let receipt = receipt(&source).unwrap();
            assert!(!receipt.files.is_empty(), "{} is empty", spec.id);
            if spec.id == "proton" {
                assert!(source.join("bin/wine").is_file());
            }
            fs::remove_dir_all(stage).unwrap();
        }
    }
}
