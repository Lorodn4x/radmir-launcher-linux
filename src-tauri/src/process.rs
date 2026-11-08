use std::{
    fs::OpenOptions,
    io::Write,
    process::Stdio,
    sync::{atomic::Ordering, Arc},
};

use crate::events::EventBus;
use chrono::Utc;

use crate::{
    db::validate_nickname,
    error::{AppError, AppResult},
    models::GameProcess,
    official, portproton,
    state::AppState,
    token_server::TokenServer,
};

pub async fn launch_game(
    app: EventBus,
    state: Arc<AppState>,
    server_id: String,
    nickname: String,
) -> AppResult<GameProcess> {
    if !state.ready.load(Ordering::SeqCst) {
        return Err(AppError::new(
            "Запуск разрешён только после успешной проверки файлов.",
        ));
    }
    if state.operation_running.load(Ordering::SeqCst) {
        return Err(AppError::new("Дождитесь завершения проверки файлов."));
    }
    if state
        .game_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return Err(AppError::new("Игра уже запущена."));
    }
    if state.operation_running.load(Ordering::SeqCst) || !state.ready.load(Ordering::SeqCst) {
        state.game_running.store(false, Ordering::SeqCst);
        return Err(AppError::new("Дождитесь завершения проверки файлов."));
    }
    match launch_inner(app, state.clone(), server_id, nickname).await {
        Ok(process) => Ok(process),
        Err(error) => {
            state.game_running.store(false, Ordering::SeqCst);
            Err(error)
        }
    }
}

async fn launch_inner(
    app: EventBus,
    state: Arc<AppState>,
    server_id: String,
    nickname: String,
) -> AppResult<GameProcess> {
    let nickname = nickname.trim().to_string();
    validate_nickname(&nickname)?;
    let settings = state.settings()?;
    let game_dir = state.paths.game_dir(&settings)?;
    let server = official::servers(&state.paths, &game_dir)
        .await?
        .into_iter()
        .find(|server| server.id == server_id)
        .ok_or_else(|| AppError::new("Выбранный сервер отсутствует в конфигурации RADMIR."))?;
    for name in ["gta_sa.exe", "clientside.dll", "samp.dll"] {
        if !game_dir.join(name).is_file() {
            return Err(AppError::new(format!(
                "Не найден обязательный файл: {name}"
            )));
        }
    }
    let portproton = portproton::detect(&settings);
    if !portproton.installed {
        return Err(AppError::new("PortProton не найден; запуск невозможен."));
    }

    let locks = state.resource_locks()?;
    crate::installation::validate_launch(&state)?;
    let run_id = uuid::Uuid::new_v4().to_string();
    let token_server = TokenServer::start().await?;
    let mut arguments = vec![
        "-c".into(),
        "-n".into(),
        nickname,
        "-h".into(),
        server.host,
        "-p".into(),
        server.port.to_string(),
        "-t".into(),
        token_server.port.to_string(),
    ];
    if settings.graphics_preset == "enhanced" {
        arguments.push("-g".into());
    }
    let runner_dir = game_dir.join(".radmir");
    std::fs::create_dir_all(&runner_dir)?;
    if std::fs::symlink_metadata(&runner_dir)?
        .file_type()
        .is_symlink()
        || !runner_dir
            .canonicalize()?
            .starts_with(game_dir.canonicalize()?)
    {
        return Err(AppError::new("Каталог helper выходит за пределы игры."));
    }
    let executable = runner_dir.join("runner.exe");
    for name in [
        "runner.exe",
        "runner.id",
        "runner.args",
        "runner.status",
        "runner.status.tmp",
        "runner.exe.ppdb",
    ] {
        if std::fs::symlink_metadata(runner_dir.join(name))
            .is_ok_and(|m| m.file_type().is_symlink())
        {
            return Err(AppError::new("Symlink в служебных файлах helper запрещён."));
        }
    }
    tokio::fs::write(&executable, include_bytes!("../../runner/runner.exe")).await?;
    let encoded: Vec<u8> = arguments
        .join(" ")
        .encode_utf16()
        .flat_map(u16::to_le_bytes)
        .collect();
    tokio::fs::write(runner_dir.join("runner.args"), encoded).await?;
    tokio::fs::write(runner_dir.join("runner.id"), run_id.as_bytes()).await?;
    let status_path = runner_dir.join("runner.status");
    let _ = tokio::fs::remove_file(&status_path).await;
    // A dedicated profile makes one-click launch persistent despite PortProton
    // clearing incoming environment flags. Never alter the user's game profile.
    tokio::fs::write(
        runner_dir.join("runner.exe.ppdb"),
        helper_profile(&game_dir, &settings)?,
    )
    .await?;
    let timestamp = Utc::now().format("%Y%m%dT%H%M%SZ");
    let log_path = state.paths.state_dir.join(format!(
        "game-{timestamp}-{}.log",
        uuid::Uuid::new_v4().simple()
    ));
    let log = OpenOptions::new()
        .create(true)
        .append(true)
        .write(true)
        .open(&log_path)?;
    let mut command = portproton::launch_command(&portproton, &executable, &[])?;
    command
        .current_dir(&game_dir)
        .env("RADMIR_RUN_ID", &run_id)
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    unsafe {
        command.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = command
        .spawn()
        .map_err(|error| AppError::new(format!("Не удалось запустить PortProton: {error}")))?;
    let Some(pid) = child.id() else {
        let _ = child.kill().await;
        return Err(AppError::new(
            "Не удалось получить PID игрового wrapper-процесса.",
        ));
    };
    let process = GameProcess {
        id: run_id,
        pid,
        executable: executable.to_string_lossy().into_owned(),
        server_id: Some(server_id),
        status: "starting".into(),
        exit_code: None,
        log_path: log_path.to_string_lossy().into_owned(),
        started_at: Utc::now().to_rfc3339(),
        ended_at: None,
        windows_pid: None,
        identity: crate::locks::ProcessIdentity::read(pid).ok(),
        token_port: Some(token_server.port),
        status_path: Some(status_path.to_string_lossy().into_owned()),
        prefix: Some(
            crate::runtime::prefix(&settings)
                .to_string_lossy()
                .into_owned(),
        ),
    };
    *state
        .process
        .lock()
        .map_err(|_| AppError::new("Блокировка процесса"))? = Some(process.clone());
    state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .record_process_start(&process)?;
    let monitored = process.clone();
    tokio::spawn(async move {
        monitor(app, state, monitored, Some(child), token_server, locks).await;
    });
    Ok(process)
}

#[derive(Debug, PartialEq)]
pub(crate) struct HelperStatus {
    pub phase: String,
    pub value: u32,
}
pub(crate) fn parse_status(text: &str, id: &str) -> Option<HelperStatus> {
    let mut parts = text.split_whitespace();
    let phase = parts.next()?;
    let value = parts.next()?.parse().ok()?;
    if parts.next()? != id || parts.next().is_some() {
        return None;
    }
    if !["starting", "clientside", "samp", "running", "exited"].contains(&phase)
        && !phase.starts_with("failed-")
    {
        return None;
    }
    Some(HelperStatus {
        phase: phase.into(),
        value,
    })
}
fn fresh_status(path: &str) -> bool {
    std::fs::metadata(path)
        .ok()
        .and_then(|m| m.modified().ok())
        .and_then(|v| v.elapsed().ok())
        .is_some_and(|a| a.as_secs() < 15)
}
async fn monitor(
    app: EventBus,
    state: Arc<AppState>,
    mut process: GameProcess,
    mut child: Option<tokio::process::Child>,
    _token: TokenServer,
    _locks: Vec<crate::locks::FileLock>,
) {
    let start = std::time::Instant::now();
    let mut had_running = process.status == "running";
    let mut last = String::new();
    let mut timed_out = false;
    loop {
        let text = if let Some(path) = &process.status_path {
            tokio::fs::read_to_string(path).await.unwrap_or_default()
        } else {
            String::new()
        };
        if text != last {
            if let Ok(mut log) = OpenOptions::new().append(true).open(&process.log_path) {
                let _ = writeln!(log, "[runner] {}", text.trim());
            }
            last = text.clone();
        }
        if let Some(status) = parse_status(&text, &process.id) {
            if status.phase == "exited" || status.phase.starts_with("failed-") {
                process.status = if status.phase == "exited" && status.value == 0 {
                    "exited"
                } else {
                    "failed"
                }
                .into();
                process.exit_code = Some(status.value as i32);
                break;
            }
            if status.phase == "running"
                && fresh_status(process.status_path.as_deref().unwrap_or(""))
                && same_boot(&process)
            {
                process.windows_pid = Some(status.value);
                let changed = process.status != "running";
                process.status = "running".into();
                had_running = true;
                if changed {
                    let _ = app.emit("game://status", &process);
                }
            }
        }
        let alive = process.identity.as_ref().is_some_and(|i| i.alive());
        let fresh = process.status_path.as_deref().is_some_and(fresh_status)
            && parse_status(&text, &process.id).is_some();
        if !had_running && !timed_out && start.elapsed().as_secs() > 300 {
            timed_out = true;
            if process
                .identity
                .as_ref()
                .is_some_and(|i| i.pid == process.pid && i.alive())
            {
                unsafe {
                    libc::kill(-(process.pid as i32), libc::SIGTERM);
                }
            }
            process.status = "recovering".into();
            let _=app.emit("launcher://error",serde_json::json!({"code":"startup_timeout","message":"Игра не подтвердила запуск за 300 секунд. Ожидаем завершения принадлежащих запуску процессов.","retryable":false}));
        }
        if !alive && !fresh && start.elapsed().as_secs() > 5 {
            // A detached wrapper is normal. A live matching helper keeps the
            // session locked even when its original Linux wrapper exited.
            if !helper_alive(&process) {
                process.status = "failed".into();
                process.exit_code = None;
                break;
            }
        }
        if had_running && !fresh {
            process.status = "recovering".into();
        }
        if let Ok(mut slot) = state.process.lock() {
            *slot = Some(process.clone());
        }
        if let Ok(db) = state.db.lock() {
            let _ = db.record_process_finish(&process);
        }
        if let Some(child) = &mut child {
            let _ = child.try_wait();
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    process.ended_at = Some(Utc::now().to_rfc3339());
    if let Ok(db) = state.db.lock() {
        let _ = db.record_process_finish(&process);
    }
    if let Ok(mut slot) = state.process.lock() {
        *slot = Some(process.clone());
    }
    state.game_running.store(false, Ordering::SeqCst);
    let _ = app.emit("game://exit", process);
}
fn helper_alive(process: &GameProcess) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Some(prefix) = &process.prefix else {
        return false;
    };
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return true;
    };
    for entry in entries.flatten() {
        if entry.file_name().to_string_lossy().parse::<u32>().is_err() {
            continue;
        }
        if !entry
            .metadata()
            .is_ok_and(|m| m.uid() == unsafe { libc::geteuid() })
        {
            continue;
        }
        let Ok(pid) = entry.file_name().to_string_lossy().parse::<u32>() else {
            continue;
        };
        let Some(original) = &process.identity else {
            continue;
        };
        let Ok(current) = crate::locks::ProcessIdentity::read(pid) else {
            continue;
        };
        if current.boot_id != original.boot_id || current.start_ticks < original.start_ticks {
            continue;
        }
        let cmd = std::fs::read(entry.path().join("cmdline")).unwrap_or_default();
        if !String::from_utf8_lossy(&cmd).contains("runner.exe") {
            continue;
        }
        let env = std::fs::read(entry.path().join("environ")).unwrap_or_default();
        if !env
            .split(|b| *b == 0)
            .any(|v| v == format!("RADMIR_RUN_ID={}", process.id).as_bytes())
        {
            continue;
        }
        if env
            .split(|b| *b == 0)
            .any(|v| v == format!("WINEPREFIX={prefix}").as_bytes())
        {
            return true;
        }
    }
    false
}
fn same_boot(process: &GameProcess) -> bool {
    process.identity.as_ref().is_some_and(|id| {
        std::fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .is_ok_and(|boot| boot.trim() == id.boot_id)
    })
}
pub async fn recover(state: Arc<AppState>) -> AppResult<()> {
    let records = state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .active_processes()?;
    for mut process in records {
        let text = process
            .status_path
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .unwrap_or_default();
        let alive = same_boot(&process)
            && (process.identity.as_ref().is_some_and(|i| i.alive())
                || process.status_path.as_deref().is_some_and(fresh_status)
                    && parse_status(&text, &process.id).is_some_and(|s| s.phase == "running")
                    && helper_alive(&process)
                || helper_alive(&process));
        if alive {
            let locks = state.resource_locks()?;
            let token = TokenServer::start_on(process.token_port.unwrap_or(0)).await?;
            process.status = "recovering".into();
            state.game_running.store(true, Ordering::SeqCst);
            *state
                .process
                .lock()
                .map_err(|_| AppError::new("Блокировка процесса"))? = Some(process.clone());
            let app = state.events.clone();
            tokio::spawn(async move {
                monitor(app, state, process, None, token, locks).await;
            });
            return Ok(());
        }
        process.status = "interrupted".into();
        process.ended_at = Some(Utc::now().to_rfc3339());
        state
            .db
            .lock()
            .map_err(|_| AppError::new("Блокировка БД"))?
            .record_process_finish(&process)?;
    }
    Ok(())
}

pub(crate) fn helper_profile(
    game: &std::path::Path,
    settings: &crate::models::Settings,
) -> AppResult<String> {
    let prefix = game
        .ancestors()
        .find(|p| p.file_name().is_some_and(|n| n == "drive_c"))
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("DEFAULT");
    if !prefix
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_-".contains(&b))
    {
        return Err(AppError::new(
            "Имя префикса PortProton содержит неподдерживаемые символы.",
        ));
    }
    let mut profile = format!("#!/usr/bin/env bash\nexport PW_PREFIX_NAME=\"{prefix}\"\nexport PW_GUI_DISABLED_CS=\"1\"\nexport PORTPROTON_NAME=\"RADMIR Standalone\"\nexport LAUNCH_PARAMETERS=\"\"\n");
    let original = game
        .parent()
        .map(|p| p.join("Launcher/RADMIR Launcher.exe.ppdb"));
    if let Some(text) = original.and_then(|p| std::fs::read_to_string(p).ok()) {
        // Parse only literal safe values. Never source/evaluate a shell profile.
        for line in text.lines() {
            if let Some((key, value)) = line.strip_prefix("export ").and_then(|l| l.split_once('='))
            {
                let value = value.trim().trim_matches('"');
                if [
                    "PW_WINE_USE",
                    "PW_VULKAN_USE",
                    "PW_USE_ESYNC",
                    "PW_USE_FSYNC",
                    "PW_USE_D3D_EXTRAS",
                    "PW_USE_GSTREAMER",
                    "PW_USE_EAC_AND_BE",
                ]
                .contains(&key)
                    && !value.is_empty()
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                {
                    profile.push_str(&format!("export {key}=\"{value}\"\n"));
                }
            }
        }
    }
    if settings.wine_build.contains("..")
        || !settings
            .wine_build
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Err(AppError::new("Некорректная сборка Wine."));
    }
    profile.push_str(&format!(
        "export PW_WINE_USE=\"{}\"\nexport PW_VULKAN_USE=\"{}\"\n",
        settings.wine_build,
        crate::runtime::graphics_value(&settings.graphics_api)?
    ));
    // PortProton's per-application setting prevents polkit dialogs for each
    // prerequisite/probe and game launch. Do not alter global user.conf.
    profile.push_str("export PW_USE_INHIBIT_SLEEP=\"0\"\n");
    Ok(profile)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_or_foreign_heartbeat_cannot_confirm_a_new_launch() {
        assert!(parse_status("running 123 old-session", "new-session").is_none());
        assert!(parse_status("running 123", "new-session").is_none());
        assert_eq!(
            parse_status("running 123 new-session", "new-session")
                .unwrap()
                .value,
            123
        );
    }
    #[test]
    fn fresh_profile_pins_wine_and_latest_graphics() {
        let profile = helper_profile(
            std::path::Path::new("/tmp/radmir-profile-test/game"),
            &crate::models::Settings::default(),
        )
        .unwrap();
        assert!(profile.contains("PW_WINE_USE=\"PROTON_LG_10-28\""));
        assert!(profile.contains("PW_VULKAN_USE=\"6\""));
        assert!(
            profile.contains("PW_USE_INHIBIT_SLEEP=\"0\""),
            "RADMIR helpers must not request privileged sleep inhibition"
        );
    }
}
