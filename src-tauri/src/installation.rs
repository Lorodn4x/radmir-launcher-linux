//! Successful preparation receipts and a separate, read-only version check.
//! Metadata is a warm-start cache, not a substitute for explicit hash checking.
use crate::{
    error::{AppError, AppResult},
    manifest::{hash_path, load_local_manifest, HashAlgorithm},
    models::{EnvironmentReport, Settings, UpdateStatus},
    runtime,
    state::AppState,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct FileStamp {
    path: PathBuf,
    device: u64,
    inode: u64,
    size: u64,
    modified: (i64, i64),
    changed: (i64, i64),
    mode: u32,
    link: Option<PathBuf>,
}
impl FileStamp {
    fn read(path: &Path) -> AppResult<Self> {
        let m = fs::symlink_metadata(path)?;
        if !m.is_file() && !m.file_type().is_symlink() {
            return Err(AppError::new("Ожидался файл компонента"));
        }
        Ok(Self {
            path: path.into(),
            device: m.dev(),
            inode: m.ino(),
            size: m.len(),
            modified: (m.mtime(), m.mtime_nsec()),
            changed: (m.ctime(), m.ctime_nsec()),
            mode: m.mode(),
            link: if m.file_type().is_symlink() {
                Some(fs::read_link(path)?)
            } else {
                None
            },
        })
    }
    fn unchanged(&self) -> bool {
        Self::read(&self.path).is_ok_and(|s| s == *self)
    }
}
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct PrefixIdentity {
    device: u64,
    inode: u64,
    wine: String,
}
fn prefix_identity(settings: &Settings) -> AppResult<PrefixIdentity> {
    let prefix = runtime::prefix(settings);
    let m = fs::metadata(&prefix)?;
    for path in [
        "system.reg",
        "drive_c/windows/system32/kernel32.dll",
        "drive_c/windows/syswow64/kernel32.dll",
    ] {
        if !prefix.join(path).is_file() {
            return Err(AppError::coded(
                "prefix_incomplete",
                "Служебные файлы префикса отсутствуют. Повторите подготовку.",
            ));
        }
    }
    Ok(PrefixIdentity {
        device: m.dev(),
        inode: m.ino(),
        wine: fs::read_to_string(prefix.join(".wine_ver"))?,
    })
}
// Registry files and user directories legitimately change while playing.
// Their contents are not cached; directory identity and runtime DLLs detect
// replacement/reset of the prefix without invalidating on a normal game exit.
pub fn prerequisite_context(settings: &Settings) -> AppResult<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(
        &prefix_identity(settings)?,
    )?)))
}
fn context(state: &Arc<AppState>, settings: &Settings) -> AppResult<String> {
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&(
        1u32,
        crate::locks::resource_key(&runtime::runtime_root(settings)),
        crate::locks::resource_key(&state.paths.game_dir(settings)?),
        &settings.wine_build,
        &settings.graphics_api,
        &settings.manifest_url,
        &settings.download_base_url,
        hex::encode(Sha256::digest(include_bytes!("../../runner/probe.exe"))),
    ))?)))
}
fn driver_context() -> AppResult<String> {
    let packages = FileStamp::read(Path::new("/var/lib/dpkg/status")).ok();
    let kernel = fs::read_to_string("/proc/sys/kernel/osrelease").unwrap_or_default();
    let mut devices = Vec::new();
    if let Ok(entries) = fs::read_dir("/sys/class/drm") {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().starts_with("card") {
                for file in [
                    "device/vendor",
                    "device/device",
                    "device/driver/module/version",
                ] {
                    let p = e.path().join(file);
                    if let Ok(value) = fs::read_to_string(&p) {
                        devices.push((p, value));
                    }
                }
            }
        }
    }
    devices.sort();
    Ok(hex::encode(Sha256::digest(serde_json::to_vec(&(
        packages, kernel, devices,
    ))?)))
}
#[derive(Serialize, Deserialize)]
struct Receipt {
    schema: u32,
    build: String,
    manifest_hash: String,
    files: Vec<FileStamp>,
    runtime: Vec<FileStamp>,
    prefix: PrefixIdentity,
    drivers: String,
}
fn receipt(state: &Arc<AppState>, settings: &Settings) -> AppResult<Option<Receipt>> {
    let text = state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .installation_receipt(&context(state, settings)?)?;
    // A truncated/obsolete receipt requires preparation; it never grants Play.
    Ok(text
        .and_then(|t| serde_json::from_str::<Receipt>(&t).ok())
        .filter(|r| r.schema == 1))
}
fn environment_matches(r: &Receipt, settings: &Settings) -> bool {
    !r.runtime.is_empty()
        && r.runtime.iter().all(FileStamp::unchanged)
        && prefix_identity(settings).is_ok_and(|p| p == r.prefix)
        && driver_context().is_ok_and(|d| d == r.drivers)
}
pub fn environment_current(state: &Arc<AppState>, settings: &Settings) -> AppResult<bool> {
    Ok(receipt(state, settings)?.is_some_and(|r| environment_matches(&r, settings)))
}
fn files_match(r: &Receipt, game: &Path) -> bool {
    !r.files.is_empty()
        && r.files.iter().all(FileStamp::unchanged)
        && hash_path(&game.join("manifest.json"), HashAlgorithm::Sha256)
            .is_ok_and(|h| h == r.manifest_hash)
}
pub fn invalidate(state: &Arc<AppState>, settings: &Settings) -> AppResult<()> {
    state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .save_installation_receipt(&context(state, settings)?, None)
}
pub fn record_success(state: &Arc<AppState>, settings: &Settings) -> AppResult<()> {
    if !state.files_ready.load(Ordering::SeqCst)
        || !state
            .environment
            .lock()
            .map_err(|_| AppError::new("Блокировка среды"))?
            .launch_ready
    {
        return Err(AppError::new(
            "Нельзя сохранить готовность без успешной подготовки",
        ));
    }
    let game = state.paths.game_dir(settings)?;
    let manifest = load_local_manifest(&game)?;
    let canonical = game.canonicalize()?;
    let files = manifest
        .files
        .iter()
        .map(|f| {
            let p = crate::paths::safe_join(&game, &f.path)?;
            let s = FileStamp::read(&p)?;
            if s.link.is_some() || s.size != f.size || !p.canonicalize()?.starts_with(&canonical) {
                return Err(AppError::coded(
                    "game_files_changed",
                    "Файлы изменились во время подготовки. Повторите проверку.",
                ));
            }
            Ok(s)
        })
        .collect::<AppResult<Vec<_>>>()?;
    let root = runtime::runtime_root(settings);
    let mut paths = crate::components::tracked_runtime_paths(state, &root)?;
    paths.push(root.join("data/scripts/start.sh"));
    paths.push(
        crate::portproton::wine_binary(&root, &settings.wine_build)
            .ok_or_else(|| AppError::new("Wine отсутствует"))?,
    );
    for path in [
        "drive_c/windows/system32/kernel32.dll",
        "drive_c/windows/syswow64/kernel32.dll",
    ] {
        let p = runtime::prefix(settings).join(path);
        paths.push(p.clone());
        paths.push(p.canonicalize()?);
    }
    for path in ["libs/directx/DXSETUP.exe", "libs/VC_redist.x86.exe"] {
        paths.push(game.join(path));
    }
    paths.sort();
    paths.dedup();
    let runtime = paths
        .iter()
        .map(|p| FileStamp::read(p))
        .collect::<AppResult<Vec<_>>>()?;
    let r = Receipt {
        schema: 1,
        build: manifest.build,
        manifest_hash: hash_path(&game.join("manifest.json"), HashAlgorithm::Sha256)?,
        files,
        runtime,
        prefix: prefix_identity(settings)?,
        drivers: driver_context()?,
    };
    state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .save_installation_receipt(
            &context(state, settings)?,
            Some(&serde_json::to_string(&r)?),
        )?;
    installed_status(state, Some(r.build))?;
    Ok(())
}
fn installed_status(state: &Arc<AppState>, build: Option<String>) -> AppResult<()> {
    let mut updates = state
        .updates
        .lock()
        .map_err(|_| AppError::new("Блокировка версии"))?;
    if updates.status.is_empty() || updates.installed_build != build {
        *updates = UpdateStatus {
            status: "checking".into(),
            installed_build: build,
            latest_build: None,
            message: "Проверяем наличие обновления…".into(),
        };
    }
    Ok(())
}
pub fn restore(state: &Arc<AppState>) -> AppResult<()> {
    let settings = state.settings()?;
    let game = state.paths.game_dir(&settings)?;
    let build = load_local_manifest(&game).ok().map(|m| m.build);
    installed_status(state, build.clone())?;
    let saved = receipt(state, &settings)?;
    let files_ok = saved.as_ref().is_some_and(|r| files_match(r, &game));
    let env_ok = saved
        .as_ref()
        .is_some_and(|r| environment_matches(r, &settings));
    state.set_files_ready(build.clone(), files_ok);
    let mut env = state
        .environment
        .lock()
        .map_err(|_| AppError::new("Блокировка среды"))?;
    let ready = files_ok && env_ok && env.graphics_error.is_none();
    let graphics_error = env.graphics_error.clone();
    *env = EnvironmentReport {
        runtime_ready: env_ok,
        launch_ready: ready,
        root: Some(runtime::runtime_root(&settings).to_string_lossy().into()),
        prefix: Some(runtime::prefix(&settings).to_string_lossy().into()),
        wine_build: settings.wine_build,
        graphics_api: settings.graphics_api,
        phase: if ready { "ready" } else { "idle" }.into(),
        message: if ready {
            "Сохранённая подготовка действительна."
        } else {
            "Среда требует подготовки или проверки."
        }
        .into(),
        graphics_error,
        ..Default::default()
    };
    state.set_ready(build, ready);
    Ok(())
}
pub fn validate_launch(state: &Arc<AppState>) -> AppResult<()> {
    let settings = state.settings()?;
    if state
        .updates
        .lock()
        .map_err(|_| AppError::new("Блокировка версии"))?
        .status
        == "update_available"
    {
        return Err(AppError::coded(
            "game_update_available",
            "Доступно обновление игры. Нажмите «Обновить».",
        ));
    }
    let r = receipt(state, &settings)?;
    if !r.is_some_and(|r| {
        files_match(&r, &state.paths.game_dir(&settings).unwrap_or_default())
            && environment_matches(&r, &settings)
    }) {
        state.set_ready(None, false);
        return Err(AppError::coded(
            "preparation_required",
            "Установка изменилась. Повторите подготовку перед запуском.",
        ));
    }
    Ok(())
}
pub fn compare_versions(
    installed: Option<String>,
    latest: AppResult<String>,
    previous: &UpdateStatus,
) -> UpdateStatus {
    match latest {
        Ok(latest) => {
            let status = if installed.is_none() {
                "not_installed"
            } else if installed.as_ref() == Some(&latest) {
                "current"
            } else {
                "update_available"
            };
            let message = match status {
                "current" => format!("Установлена актуальная версия {latest}."),
                "update_available" => {
                    format!("Доступна версия {latest}. Нажмите «Обновить», чтобы загрузить её.")
                }
                _ => "Игра ещё не установлена.".into(),
            };
            UpdateStatus {
                status: status.into(),
                installed_build: installed,
                latest_build: Some(latest),
                message,
            }
        }
        Err(_) => {
            // Network failure must not hide an update we already know about.
            if previous.status == "update_available" && previous.installed_build == installed {
                return previous.clone();
            }
            UpdateStatus {status:"unavailable".into(),installed_build:installed,latest_build:None,
                message:"Сервис обновлений недоступен. Используется ранее подготовленная версия; проверку можно повторить.".into()}
        }
    }
}
pub async fn check_updates(state: &Arc<AppState>) -> AppResult<UpdateStatus> {
    // Coalesce simultaneous windows. No OperationGuard: this check does not
    // install, modify the prefix, hash the game, or stop an active session.
    let _check = match state.update_check.try_lock() {
        Ok(g) => g,
        Err(_) => {
            let _wait = state.update_check.lock().await;
            return Ok(state
                .updates
                .lock()
                .map_err(|_| AppError::new("Блокировка версии"))?
                .clone());
        }
    };
    let settings = state.settings()?;
    let game = state.paths.game_dir(&settings)?;
    let installed = load_local_manifest(&game).ok().map(|m| m.build);
    let previous = state
        .updates
        .lock()
        .map_err(|_| AppError::new("Блокировка версии"))?
        .clone();
    let client = crate::updater::http_client()?;
    let latest = if let Some(url) = &settings.manifest_url {
        crate::manifest::fetch_manifest(url, &client)
            .await
            .and_then(|m| {
                crate::manifest::validate_manifest(&m)?;
                Ok(m.build)
            })
    } else {
        crate::official::latest_version(&client).await
    };
    // Do not publish a response for settings/build that changed mid-request.
    let current = state.settings()?;
    if context(state, &current)? != context(state, &settings)?
        || load_local_manifest(&game).ok().map(|m| m.build) != installed
    {
        return Ok(state
            .updates
            .lock()
            .map_err(|_| AppError::new("Блокировка версии"))?
            .clone());
    }
    let status = compare_versions(installed, latest, &previous);
    *state
        .updates
        .lock()
        .map_err(|_| AppError::new("Блокировка версии"))? = status.clone();
    Ok(status)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::Fixture;
    fn prepared() -> Fixture {
        let f = Fixture::new();
        let mut s = f.state.settings().unwrap();
        s.manifest_url = Some("https://127.0.0.1:1/manifest.json".into());
        f.state.db.lock().unwrap().save_settings(&s).unwrap();
        let game = f.state.paths.game_dir(&s).unwrap();
        let root = runtime::runtime_root(&s);
        let prefix = runtime::prefix(&s);
        for path in [
            root.join("data/scripts/start.sh"),
            root.join("data/dist/PROTON_LG_10-28/bin/wine"),
            prefix.join("system.reg"),
            prefix.join("drive_c/windows/system32/kernel32.dll"),
            prefix.join("drive_c/windows/syswow64/kernel32.dll"),
            game.join("libs/directx/DXSETUP.exe"),
            game.join("libs/VC_redist.x86.exe"),
            game.join("gta_sa.exe"),
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, b"hello").unwrap();
        }
        fs::write(prefix.join(".wine_ver"), s.wine_build.as_bytes()).unwrap();
        fs::write(game.join("manifest.json"),r#"{"build":"8.9.3","files":[{"path":"gta_sa.exe","size":5,"hash":"5d41402abc4b2a76b9719d911017c592"}]}"#).unwrap();
        f.state.set_files_ready(Some("8.9.3".into()), true);
        {
            let mut env = f.state.environment.lock().unwrap();
            env.launch_ready = true;
            env.runtime_ready = true;
        }
        record_success(&f.state, &s).unwrap();
        f
    }
    #[test]
    fn warm_restart_reuses_verified_preparation_without_any_helpers_or_events() {
        let f = prepared();
        let db = crate::db::Database::open(&f.state.paths.config_dir.join("launcher.db")).unwrap();
        let restarted = Arc::new(AppState::new(f.state.paths.clone(), db));
        restore(&restarted).unwrap();
        let status = restarted.snapshot(0);
        assert!(status.ready && status.files_ready && status.environment.launch_ready);
        assert_eq!(status.updates.installed_build.as_deref(), Some("8.9.3"));
        assert!(!status.active);
        assert!(status.events.is_empty());
    }
    #[tokio::test]
    async fn explicit_hash_check_reuses_healthy_runtime_without_prefix_or_graphics_helpers() {
        let f = prepared();
        restore(&f.state).unwrap();
        let report = runtime::prepare_installation(f.state.clone(), true)
            .await
            .unwrap();
        assert!(report.ready);
        assert_eq!(report.downloaded_files, 0);
        assert_eq!(report.total_files, 1);
        let phases: Vec<_> = f
            .state
            .events
            .since(0)
            .into_iter()
            .filter(|e| e.event == "launcher://progress")
            .map(|e| e.payload["phase"].as_str().unwrap_or("").to_string())
            .collect();
        assert!(phases.iter().any(|p| p == "hashing"));
        assert!(!phases.iter().any(|p| [
            "prefix-init",
            "graphics-check",
            "game-libraries",
            "runtime-install"
        ]
        .contains(&p.as_str())));
        assert!(!f.state.paths.state_dir.read_dir().unwrap().any(|e| e
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("runtime-")));
    }
    #[test]
    fn known_update_is_enforced_by_the_worker_even_if_a_window_still_shows_ready() {
        let f = prepared();
        restore(&f.state).unwrap();
        f.state.updates.lock().unwrap().status = "update_available".into();
        assert_eq!(
            validate_launch(&f.state).unwrap_err().code(),
            "game_update_available"
        );
    }
    #[test]
    fn same_size_modified_game_requires_verification_but_does_not_reset_runtime() {
        let f = prepared();
        let s = f.state.settings().unwrap();
        let game = f.state.paths.game_dir(&s).unwrap();
        fs::write(game.join("gta_sa.exe"), b"other").unwrap();
        restore(&f.state).unwrap();
        assert!(!f.state.snapshot(0).files_ready);
        assert!(!f.state.snapshot(0).ready);
        assert!(environment_current(&f.state, &s).unwrap());
    }
    #[test]
    fn runtime_damage_or_changed_selection_cannot_restore_play() {
        let f = prepared();
        let s = f.state.settings().unwrap();
        fs::remove_file(runtime::runtime_root(&s).join("data/scripts/start.sh")).unwrap();
        restore(&f.state).unwrap();
        assert!(!f.state.snapshot(0).ready);
        assert!(f.state.snapshot(0).files_ready);
        let mut changed = s;
        changed.graphics_api = "opengl".into();
        f.state.db.lock().unwrap().save_settings(&changed).unwrap();
        restore(&f.state).unwrap();
        assert!(!f.state.snapshot(0).ready);
    }
    #[test]
    fn registry_write_does_not_force_another_probe_but_prefix_reset_does() {
        let f = prepared();
        let s = f.state.settings().unwrap();
        let p = runtime::prefix(&s);
        fs::write(p.join("user.reg"), b"game settings").unwrap();
        fs::write(p.join("system.reg"), b"registry changed").unwrap();
        restore(&f.state).unwrap();
        assert!(f.state.snapshot(0).ready);
        fs::remove_file(p.join("drive_c/windows/syswow64/kernel32.dll")).unwrap();
        restore(&f.state).unwrap();
        assert!(!f.state.snapshot(0).ready);
    }
    #[test]
    fn version_check_handles_current_update_missing_offline_without_automatic_download() {
        let prev = UpdateStatus::default();
        let current = compare_versions(Some("8.9.3".into()), Ok("8.9.3".into()), &prev);
        assert_eq!(current.status, "current");
        let update = compare_versions(Some("8.9.3".into()), Ok("8.9.4".into()), &prev);
        assert_eq!(update.status, "update_available");
        assert_eq!(
            compare_versions(None, Ok("8.9.4".into()), &prev).status,
            "not_installed"
        );
        assert_eq!(
            compare_versions(
                Some("8.9.3".into()),
                Err(AppError::new("offline")),
                &current
            )
            .status,
            "unavailable"
        );
        assert_eq!(
            compare_versions(Some("8.9.3".into()), Err(AppError::new("offline")), &update).status,
            "update_available"
        );
    }
}
