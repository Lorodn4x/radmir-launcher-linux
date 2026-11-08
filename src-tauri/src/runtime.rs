//! One preparation pipeline: dependencies -> runtime -> prefix -> game -> probes.
use crate::{
    components,
    error::{AppError, AppResult},
    models::{
        DependencyPlan, EnvironmentReport, GraphicsProfile, PreparationReport, ProgressPayload,
        ReadyPayload, Settings,
    },
    portproton,
    state::{AppState, OperationGuard},
    updater,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Stdio,
    sync::{atomic::Ordering, Arc},
    time::{Duration, Instant},
};

pub fn graphics_profiles() -> Vec<GraphicsProfile> {
    [
        (
            "latest",
            "DXVK/VKD3D — новейшие",
            "6",
            Some("2.7.1-509"),
            Some("1.1-5122"),
            Some("1.3"),
        ),
        (
            "stable",
            "DXVK/VKD3D — стабильные",
            "2",
            Some("2.6.2"),
            Some("2.14.1"),
            Some("1.3"),
        ),
        (
            "sarek",
            "DXVK/VKD3D Sarek",
            "1",
            Some("sarek-1.11.0"),
            Some("sarek-2.6.0"),
            Some("1.1"),
        ),
        ("opengl", "WineD3D / OpenGL", "0", None, None, None),
    ]
    .into_iter()
    .map(|(id, label, value, dxvk, vkd3d, vulkan)| GraphicsProfile {
        id: id.into(),
        label: label.into(),
        portproton_value: value.into(),
        dxvk_version: dxvk.map(str::to_string),
        vkd3d_version: vkd3d.map(str::to_string),
        minimum_vulkan: vulkan.map(str::to_string),
    })
    .collect()
}
pub fn graphics_value(id: &str) -> AppResult<String> {
    graphics_profiles()
        .into_iter()
        .find(|p| p.id == id)
        .map(|p| p.portproton_value)
        .ok_or_else(|| AppError::coded("invalid_settings", "Неизвестный 3D API."))
}
pub fn runtime_root(settings: &Settings) -> PathBuf {
    portproton::detect(settings)
        .root
        .map(PathBuf::from)
        .unwrap_or_else(|| portproton::native_root(settings))
}
pub fn prefix(settings: &Settings) -> PathBuf {
    runtime_root(settings).join("data/prefixes/DEFAULT")
}
fn phase(state: &Arc<AppState>, name: &str, message: &str) {
    if let Ok(mut env) = state.environment.lock() {
        env.phase = name.into();
        env.message = message.into();
    }
    let _ = state.events.emit(
        "launcher://progress",
        ProgressPayload {
            phase: name.into(),
            message: message.into(),
            ..Default::default()
        },
    );
}

pub fn migrate_preferences(state: &Arc<AppState>) -> AppResult<()> {
    let db = state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?;
    if db.preferences_imported()? {
        return Ok(());
    }
    let mut settings = db.load_settings()?;
    if let Ok(text) = fs::read_to_string(
        state
            .paths
            .game_dir(&settings)?
            .join(".radmir/runner.exe.ppdb"),
    ) {
        for line in text.lines() {
            if let Some((key, value)) = line.strip_prefix("export ").and_then(|v| v.split_once('='))
            {
                let value = value.trim().trim_matches('"');
                if key == "PW_WINE_USE"
                    && !value.is_empty()
                    && value
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
                {
                    let version = if value == "PROTON_LG" {
                        "PROTON_LG_10-28"
                    } else {
                        value
                    };
                    if portproton::wine_binary(&runtime_root(&settings), version).is_some() {
                        settings.wine_build = version.into();
                    }
                }
                if key == "PW_VULKAN_USE" {
                    if let Some(p) = graphics_profiles()
                        .into_iter()
                        .find(|p| p.portproton_value == value)
                    {
                        settings.graphics_api = p.id;
                    }
                }
            }
        }
    }
    db.save_settings(&settings)?;
    db.mark_preferences_imported()?;
    Ok(())
}

const TOOLS: &[(&str, &str)] = &[
    ("bash", "bash"),
    ("curl", "curl"),
    ("tar", "tar"),
    ("xz", "xz-utils"),
    ("jq", "jq"),
    ("file", "file"),
];
// scripts ships its own Yad; plugins ships zstd and vulkaninfo. Their absence
// belongs to component repair, not to a request for duplicate host packages.
pub fn tool_path(settings: &Settings, name: &str) -> AppResult<PathBuf> {
    use std::os::unix::fs::PermissionsExt;
    let root = runtime_root(settings);
    let bundled = match name {
        "yad" => root.join("data/themes/gui/yad_gui_pp"),
        "vulkaninfo" | "zstd" => root.join("data/tmp/plugins_v20/portable/bin").join(name),
        _ => return Err(AppError::new("Неизвестный инструмент среды")),
    };
    if fs::metadata(&bundled).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0) {
        return Ok(bundled);
    }
    crate::paths::find_in_path(name).ok_or_else(|| {
        AppError::coded(
            "runtime_tool_failed",
            format!(
                "Инструмент {name} отсутствует в PortProton. Повторите восстановление компонентов."
            ),
        )
    })
}
fn tool_version_ok(name: &str, output: &std::process::Output) -> bool {
    if name != "yad" {
        return output.status.success();
    }
    // PortProton's Yad 14 prints --version successfully, then uses its normal
    // GUI cleanup exit code 252. Accept that only with a recognised version.
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.split_whitespace().next().unwrap_or("");
    let parts: Vec<_> = version.split('.').collect();
    let recognised = (2..=3).contains(&parts.len())
        && parts
            .iter()
            .all(|p| !p.is_empty() && p.bytes().all(|b| b.is_ascii_digit()))
        && text.contains("(GTK+ ");
    recognised && (output.status.success() || output.status.code() == Some(252))
}
pub async fn validate_bundled_tools(state: &Arc<AppState>) -> AppResult<()> {
    use std::io::Write;
    let settings = state.settings()?;
    let log_path = state.paths.state_dir.join("runtime-tools.log");
    let mut log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)?;
    for name in ["yad", "zstd"] {
        let path = tool_path(&settings, name)?;
        let mut cmd = tokio::process::Command::new(&path);
        cmd.arg("--version").kill_on_drop(true);
        portproton::clean_environment(&mut cmd);
        let output = tokio::time::timeout(Duration::from_secs(10), cmd.output()).await;
        let (valid, detail) = match output {
            Ok(Ok(o)) => {
                writeln!(
                    log,
                    "{} {} --version: exit {:?}\nstdout: {}\nstderr: {}",
                    chrono::Utc::now(),
                    path.display(),
                    o.status.code(),
                    String::from_utf8_lossy(&o.stdout[..o.stdout.len().min(4096)]),
                    String::from_utf8_lossy(&o.stderr[..o.stderr.len().min(4096)])
                )?;
                (
                    tool_version_ok(name, &o),
                    format!("код {:?}", o.status.code()),
                )
            }
            Ok(Err(e)) => {
                writeln!(log, "{} {}: {e}", chrono::Utc::now(), path.display())?;
                (false, e.to_string())
            }
            Err(_) => {
                writeln!(
                    log,
                    "{} {}: timeout 10s",
                    chrono::Utc::now(),
                    path.display()
                )?;
                (false, "нет ответа за 10 секунд".into())
            }
        };
        if !valid {
            return Err(AppError::with_log("runtime_tool_failed",
                format!("Проверка {name} не пройдена ({detail}). Откройте журнал runtime-tools.log для точной причины."),&log_path));
        }
    }
    tool_path(&settings, "vulkaninfo")?;
    Ok(())
}
fn ubuntu() -> bool {
    fs::read_to_string("/etc/os-release").is_ok_and(|s| {
        s.lines().any(|l| l == "ID=ubuntu")
            && s.lines()
                .any(|l| l == "VERSION_ID=\"24.04\"" || l == "VERSION_ID=\"26.04\"")
    })
}
fn package_installed(package: &str) -> bool {
    std::process::Command::new("dpkg-query")
        .args(["-W", "-f=${Status}", package])
        .output()
        .is_ok_and(|o| {
            o.status.success()
                && String::from_utf8_lossy(&o.stdout).contains("install ok installed")
        })
}
fn missing_graphics_packages(
    api: &str,
    gpu: &str,
    mesa_failure: bool,
    installed: impl Fn(&str) -> bool,
) -> Vec<String> {
    let mut candidates = vec!["libgl1:i386", "libfreetype6:i386"];
    let mesa = mesa_failure || gpu.contains("amd") || gpu.contains("intel") || gpu.contains("mesa");
    if mesa {
        candidates.push("libgl1-mesa-dri:i386");
    }
    if api != "opengl" {
        candidates.push("libvulkan1:i386");
        if mesa {
            candidates.push("mesa-vulkan-drivers:i386");
        }
    }
    candidates
        .into_iter()
        .filter(|p| !installed(p))
        .map(str::to_string)
        .collect()
}
fn graphics_context(settings: &Settings) -> String {
    hex::encode(Sha256::digest(
        serde_json::to_vec(&(
            crate::locks::resource_key(&prefix(settings)),
            &settings.wine_build,
            &settings.graphics_api,
        ))
        .expect("serializable graphics context"),
    ))
}
pub fn restore_graphics_diagnostic(state: &Arc<AppState>) -> AppResult<()> {
    let settings = state.settings()?;
    let error = state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .graphics_failure(&graphics_context(&settings))?;
    let mut env = state
        .environment
        .lock()
        .map_err(|_| AppError::new("Блокировка среды"))?;
    env.graphics_error = error.clone();
    if let Some(error) = error {
        env.launch_ready = false;
        env.message = error["message"]
            .as_str()
            .unwrap_or("32-битная графическая проверка не пройдена.")
            .into();
    }
    Ok(())
}
pub async fn dependency_plan(state: &Arc<AppState>) -> AppResult<DependencyPlan> {
    let mut packages: Vec<String> = TOOLS
        .iter()
        .filter(|(tool, _)| crate::paths::find_in_path(tool).is_none())
        .map(|(_, pkg)| (*pkg).into())
        .collect();
    let settings = state.settings()?;
    let failure = state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .graphics_failure(&graphics_context(&settings))?;
    let mut enable_i386 = false;
    if let Some(failure) = failure.filter(|f| {
        matches!(
            f["code"].as_str(),
            Some("missing_32bit_graphics" | "graphics_probe_failed")
        )
    }) {
        let architectures = tokio::process::Command::new("dpkg")
            .arg("--print-foreign-architectures")
            .output()
            .await?;
        enable_i386 = !String::from_utf8_lossy(&architectures.stdout)
            .lines()
            .any(|l| l == "i386");
        let mut vulkan = tokio::process::Command::new(tool_path(&settings, "vulkaninfo")?);
        vulkan.arg("--summary");
        portproton::clean_environment(&mut vulkan);
        let gpu = if let Ok(output) = vulkan.output().await {
            String::from_utf8_lossy(&output.stdout).to_lowercase()
        } else {
            String::new()
        };
        packages.extend(missing_graphics_packages(
            &settings.graphics_api,
            &gpu,
            failure["code"] == "missing_32bit_graphics",
            package_installed,
        ));
    }
    packages.sort();
    packages.dedup();
    let mut blocked = None;
    let mut changes = String::new();
    if !packages.is_empty() {
        if !ubuntu() {
            blocked=Some("Автоматическая установка пакетов проверяется для Ubuntu 24.04/26.04. Установите зависимости через пакетный менеджер системы.".into());
        } else if enable_i386 {
            changes="Включение архитектуры i386 и обновление индексов пакетов. Затем лаунчер покажет полный план установки.".into();
        } else {
            let output = tokio::process::Command::new("apt-get")
                .args(["--simulate", "install", "--"])
                .args(&packages)
                .output()
                .await?;
            changes = format!(
                "{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            );
            if !output.status.success() {
                blocked=Some("APT не смог рассчитать установку. Обновите индексы пакетов и проверьте источники Ubuntu.".into());
            }
            if changes.lines().any(|l| l.starts_with("Remv ")) {
                blocked = Some(
                    "План APT удаляет установленные пакеты. Автоматическое выполнение остановлено."
                        .into(),
                );
            }
        }
    }
    let stamp = fs::read("/var/lib/dpkg/status").unwrap_or_default();
    let id = hex::encode(Sha256::digest(
        [
            serde_json::to_vec(&packages)?,
            changes.as_bytes().to_vec(),
            stamp,
            vec![enable_i386 as u8],
        ]
        .concat(),
    ));
    Ok(DependencyPlan {
        id,
        packages,
        enable_i386,
        changes,
        blocked,
    })
}
pub async fn install_dependencies(state: &Arc<AppState>, id: &str) -> AppResult<DependencyPlan> {
    let _guard = OperationGuard::begin(state)?;
    let plan = dependency_plan(state).await?;
    if plan.id != id {
        return Err(AppError::coded(
            "dependency_plan_changed",
            "План пакетов изменился. Просмотрите новый план перед подтверждением.",
        ));
    }
    if let Some(reason) = &plan.blocked {
        return Err(AppError::coded("dependency_plan_blocked", reason));
    }
    if plan.packages.is_empty() {
        return Ok(plan);
    }
    if plan.enable_i386 {
        privileged(
            state,
            "/usr/bin/dash",
            &[
                "-c".into(),
                "/usr/bin/dpkg --add-architecture i386 && /usr/bin/apt-get update".into(),
            ],
        )
        .await?;
        return dependency_plan(state).await;
    }
    let mut args = vec!["install".into(), "--assume-yes".into(), "--".into()];
    args.extend(plan.packages.clone());
    privileged(state, "/usr/bin/apt-get", &args).await?;
    dependency_plan(state).await
}
async fn privileged(state: &Arc<AppState>, program: &str, args: &[String]) -> AppResult<()> {
    phase(
        state,
        "system-dependencies",
        "Ожидаем системное подтверждение администратора…",
    );
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.paths.state_dir.join("system-dependencies.log"))?;
    let mut cmd = tokio::process::Command::new("pkexec");
    cmd.arg(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log));
    portproton::clean_environment(&mut cmd);
    let status = cmd.status().await?;
    if !status.success() {
        return Err(AppError::coded("authorization_denied",format!("Установка пакетов не завершена (код {:?}). Можно повторить подтверждение. Журнал system-dependencies.log.",status.code())));
    }
    Ok(())
}

pub async fn ensure_environment(state: &Arc<AppState>) -> AppResult<()> {
    let settings = state.settings()?;
    let root = runtime_root(&settings);
    phase(
        state,
        "preflight",
        "Проверяем зависимости и выбранный PortProton…",
    );
    let plan = dependency_plan(state).await?;
    if !plan.packages.is_empty() {
        return Err(AppError::coded(
            "missing_dependencies",
            format!(
                "Для подготовки нужны пакеты: {}. Откройте план установки в настройках.",
                plan.packages.join(", ")
            ),
        ));
    }
    if !settings.auto_install_portproton && !portproton::detect(&settings).installed {
        return Err(AppError::coded(
            "runtime_missing",
            "PortProton не готов. Включите автоматическую подготовку.",
        ));
    }
    {
        let mut env = state
            .environment
            .lock()
            .map_err(|_| AppError::new("Блокировка среды"))?;
        *env = EnvironmentReport {
            root: Some(root.to_string_lossy().into()),
            prefix: Some(prefix(&settings).to_string_lossy().into()),
            wine_build: settings.wine_build.clone(),
            graphics_api: settings.graphics_api.clone(),
            phase: "runtime-install".into(),
            graphics_error: state
                .db
                .lock()
                .map_err(|_| AppError::new("Блокировка БД"))?
                .graphics_failure(&graphics_context(&settings))?,
            ..Default::default()
        };
    }
    for id in ["scripts", "plugins", "libs"] {
        components::install_component(state, &root, &components::component(id)?).await?;
    }
    components::selected_wine(state, &root, &settings.wine_build).await?;
    // Upstream prepares all installed graphics profiles; seed all pinned pairs
    // so an unavailable upstream metadata host cannot restart the first launch.
    for id in [
        "dxvk-2.7.1-509",
        "vkd3d-proton-1.1-5122",
        "dxvk-2.6.2",
        "vkd3d-proton-2.14.1",
        "dxvk-sarek-1.11.0",
        "vkd3d-proton-sarek-2.6.0",
    ] {
        components::install_component(state, &root, &components::component(id)?).await?;
    }
    validate_bundled_tools(state).await?;
    if let Ok(mut env) = state.environment.lock() {
        env.runtime_ready = true;
    }
    Ok(())
}

fn win_path(path: &Path) -> String {
    format!("Z:{}", path.to_string_lossy().replace('/', "\\"))
}
fn write_wide(path: &Path, strings: &[String]) -> AppResult<()> {
    let text = strings.join("\0") + "\0";
    let bytes: Vec<u8> = text.encode_utf16().flat_map(u16::to_le_bytes).collect();
    if bytes.len() > 120 * 1024 {
        return Err(AppError::new("Слишком длинный путь Windows."));
    }
    fs::write(path, bytes)?;
    Ok(())
}
async fn windows_command(
    state: &Arc<AppState>,
    exe: Option<&Path>,
    args: &str,
    label: &str,
) -> AppResult<u32> {
    let settings = state.settings()?;
    let game = state.paths.game_dir(&settings)?;
    let own = game.join(".radmir");
    fs::create_dir_all(&own)?;
    let helper = own.join("command.exe");
    fs::write(&helper, include_bytes!("../../runner/command.exe"))?;
    let result = own.join("command.status");
    let _ = fs::remove_file(&result);
    write_wide(
        &own.join("command.args"),
        &[
            exe.map(win_path).unwrap_or_default(),
            args.into(),
            exe.and_then(Path::parent).map(win_path).unwrap_or_default(),
        ],
    )?;
    fs::write(
        own.join("command.exe.ppdb"),
        crate::process::helper_profile(&game, &settings)?,
    )?;
    run_helper(state, &helper, &result, label, false).await
}
async fn run_helper(
    state: &Arc<AppState>,
    exe: &Path,
    result: &Path,
    label: &str,
    probe: bool,
) -> AppResult<u32> {
    let settings = state.settings()?;
    let status = portproton::detect(&settings);
    let log_path = state
        .paths
        .state_dir
        .join(format!("runtime-{}.log", uuid::Uuid::new_v4()));
    let log = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&log_path)?;
    let mut cmd = portproton::launch_command(&status, exe, &[])?;
    cmd.current_dir(exe.parent().unwrap())
        .stdout(Stdio::from(log.try_clone()?))
        .stderr(Stdio::from(log))
        .kill_on_drop(true);
    unsafe {
        cmd.pre_exec(|| {
            if libc::setsid() == -1 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let mut child = cmd.spawn()?;
    let start = Instant::now();
    let mut terminal = None;
    let mut cleanup_started = None;
    loop {
        if state.check_cancelled().is_err() || start.elapsed() > Duration::from_secs(600) {
            stop_helper(&mut child).await?;
            return Err(AppError::with_log(
                if state.cancelled.load(Ordering::SeqCst) {
                    "cancelled"
                } else {
                    "runtime_timeout"
                },
                format!("{label}: подготовка остановлена"),
                &log_path,
            ));
        }
        if terminal.is_none() {
            terminal = helper_result(result, label, probe, &log_path);
            if terminal.is_some() {
                cleanup_started = Some(Instant::now());
            }
        }
        if let Some(exit) = child.try_wait()? {
            // The status file may have been closed between the first read and wait.
            return terminal
                .or_else(|| helper_result(result, label, probe, &log_path))
                .unwrap_or_else(|| {
                    Err(helper_failure(
                        label,
                        probe,
                        &format!("PortProton завершился ({exit}) без результата Windows-helper"),
                        &log_path,
                    ))
                });
        }
        if cleanup_started.is_some_and(|t| t.elapsed() > Duration::from_secs(30)) {
            stop_helper(&mut child).await?;
            return Err(AppError::with_log("runtime_cleanup_failed",
                format!("{label}: PortProton не завершил очистку служебного сеанса. Повторите подготовку"), &log_path));
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

async fn stop_helper(child: &mut tokio::process::Child) -> AppResult<()> {
    if child.try_wait()?.is_some() {
        return Ok(());
    }
    // This child created its own session; never signal a process found by name.
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
    }
    match tokio::time::timeout(Duration::from_secs(5), child.wait()).await {
        Ok(result) => {
            result?;
        }
        Err(_) => {
            if let Some(pid) = child.id() {
                unsafe {
                    libc::kill(-(pid as i32), libc::SIGKILL);
                }
            }
            child.kill().await?;
        }
    }
    Ok(())
}
fn helper_result(result: &Path, label: &str, probe: bool, log: &Path) -> Option<AppResult<u32>> {
    let text = fs::read_to_string(result).ok()?;
    let (phase, code) = text.trim().split_once(' ')?;
    let code = code.parse::<u32>().ok()?;
    if phase == "exited" || phase == "ready" {
        return Some(if probe && code != 0 {
            Err(helper_failure(label, probe, &format!("код {code}"), log))
        } else {
            Ok(code)
        });
    }
    if phase.starts_with("failed-") {
        return Some(Err(helper_failure(
            label,
            probe,
            &format!("{phase}, код {code}"),
            log,
        )));
    }
    None
}
fn helper_failure(label: &str, probe: bool, detail: &str, log: &Path) -> AppError {
    let tail = portproton::log_tail(log, 32 * 1024).unwrap_or_default();
    let missing_mesa = tail.contains("i386-linux-gnu/dri/")
        && tail.contains("failed to open")
        && (tail.contains("radeonsi") || tail.contains("swrast") || tail.contains("iris"));
    if probe && missing_mesa {
        AppError::with_log("missing_32bit_graphics",
            "В 32-битной среде Wine отсутствуют драйверы Mesa. Откройте план системных пакетов и подтвердите установку, затем повторите подготовку", log)
    } else {
        AppError::with_log(
            if probe {
                "graphics_probe_failed"
            } else {
                "prerequisite_failed"
            },
            format!("{label}: {detail}"),
            log,
        )
    }
}

#[derive(Serialize, Deserialize)]
struct PrefixStamp {
    wine: String,
    api: String,
    installers: Vec<(String, String)>,
}
fn copy_service_tree(source: &Path, dest: &Path, game: &Path) -> AppResult<()> {
    use std::os::unix::fs::symlink;
    fs::create_dir_all(dest)?;
    for e in fs::read_dir(source)? {
        let e = e?;
        let path = e.path();
        if path == game || game.starts_with(&path) && path.is_file() {
            continue;
        }
        let target = dest.join(e.file_name());
        let m = fs::symlink_metadata(&path)?;
        if m.file_type().is_symlink() {
            symlink(fs::read_link(&path)?, target)?;
        } else if m.is_dir() {
            copy_service_tree(&path, &target, game)?;
        } else if m.is_file() {
            fs::copy(&path, &target)?;
        }
    }
    Ok(())
}
async fn backup_prefix(state: &Arc<AppState>, settings: &Settings) -> AppResult<Option<PathBuf>> {
    let p = prefix(settings);
    let marker = state.paths.state_dir.join("prefix-prepared.json");
    let old = fs::read(&marker)
        .ok()
        .and_then(|b| serde_json::from_slice::<PrefixStamp>(&b).ok());
    let wine_old = fs::read_to_string(p.join(".wine_ver")).unwrap_or_default();
    if !p.join("system.reg").exists()
        || old
            .as_ref()
            .is_some_and(|o| o.wine == settings.wine_build && o.api == settings.graphics_api)
        || old.is_none() && wine_old.trim() == settings.wine_build
    {
        return Ok(None);
    }
    let backup = runtime_root(settings)
        .join("data/backups")
        .join(format!("prefix-{}", uuid::Uuid::new_v4()));
    phase(
        state,
        "prefix-backup",
        "Сохраняем служебную часть префикса перед сменой версии…",
    );
    let game = state.paths.game_dir(settings)?;
    let b = backup.clone();
    tokio::task::spawn_blocking(move || copy_service_tree(&p, &b, &game)).await??;
    Ok(Some(backup))
}
fn restore_prefix(backup: &Path, prefix: &Path) -> AppResult<()> {
    // Registry and Windows runtime are the mutable compatibility state. Games
    // and user directories are deliberately not replaced during rollback.
    for name in ["system.reg", "user.reg", "userdef.reg", ".wine_ver"] {
        let p = backup.join(name);
        if p.is_file() {
            fs::copy(p, prefix.join(name))?;
        }
    }
    let windows = backup.join("drive_c/windows");
    let target = prefix.join("drive_c/windows");
    if windows.is_dir() {
        let failed = prefix.join(format!(".failed-windows-{}", uuid::Uuid::new_v4()));
        if target.exists() {
            fs::rename(&target, &failed)?;
        }
        fs::rename(windows, &target)?;
    }
    Ok(())
}
async fn libraries(state: &Arc<AppState>, settings: &Settings) -> AppResult<Vec<(String, String)>> {
    let game = state.paths.game_dir(settings)?;
    let mut result = Vec::new();
    for (path, args) in [
        ("libs/directx/DXSETUP.exe", "/silent"),
        ("libs/VC_redist.x86.exe", "/install /quiet /norestart"),
    ] {
        let exe = game.join(path);
        if !exe.is_file() {
            return Err(AppError::coded(
                "prerequisite_missing",
                format!("В проверенных игровых файлах отсутствует {path}"),
            ));
        }
        let hash = hash_path(&exe, HashAlgorithm::Sha256)?;
        result.push((path.into(), hash));
        let stamp = state.paths.state_dir.join(format!(
            "prerequisite-{}.json",
            hex::encode(Sha256::digest(
                [
                    prefix(settings).to_string_lossy().as_bytes(),
                    path.as_bytes()
                ]
                .concat()
            ))
        ));
        let signature = serde_json::to_vec(&(
            settings.wine_build.clone(),
            crate::installation::prerequisite_context(settings)?,
            result.last().unwrap(),
        ))?;
        if fs::read(&stamp).is_ok_and(|b| b == signature) {
            continue;
        }
        phase(
            state,
            "game-libraries",
            &format!("Устанавливаем {path} в игровой префикс…"),
        );
        let code = windows_command(state, Some(&exe), args, path).await?;
        if ![0, 1638, 3010].contains(&code) {
            return Err(AppError::coded(
                "prerequisite_failed",
                format!("{path}: код {code}. Подготовка библиотек не завершена."),
            ));
        }
        fs::write(stamp, signature)?;
    }
    Ok(result)
}
use crate::manifest::{hash_path, HashAlgorithm};
pub fn compatible_version(api: &str, major: u32, minor: u32) -> bool {
    match api {
        "opengl" => true,
        "sarek" => (major, minor) >= (1, 1),
        "latest" | "stable" => (major, minor) >= (1, 3),
        _ => false,
    }
}
async fn graphics_probe(state: &Arc<AppState>, settings: &Settings) -> AppResult<()> {
    let result = graphics_probe_inner(state, settings).await;
    let error = result
        .as_ref()
        .err()
        .filter(|e| matches!(e.code(), "missing_32bit_graphics" | "graphics_probe_failed"));
    if result.is_ok() || error.is_some() {
        state
            .db
            .lock()
            .map_err(|_| AppError::new("Блокировка БД"))?
            .set_graphics_failure(&graphics_context(settings), error)?;
        restore_graphics_diagnostic(state)?;
    }
    result
}
async fn graphics_probe_inner(state: &Arc<AppState>, settings: &Settings) -> AppResult<()> {
    phase(
        state,
        "graphics-check",
        "Проверяем выбранный 3D API через PE32 и Wine…",
    );
    if settings.graphics_api != "opengl" {
        let mut cmd = tokio::process::Command::new(tool_path(settings, "vulkaninfo")?);
        cmd.arg("--summary");
        portproton::clean_environment(&mut cmd);
        let output = cmd.output().await?;
        let text = String::from_utf8_lossy(&output.stdout);
        let mut current = None;
        let mut version = None;
        for line in text.lines() {
            let line = line.trim();
            if line.starts_with("apiVersion") {
                current = line.split('=').nth(1).map(str::trim).and_then(|s| {
                    let mut v = s.split('.');
                    Some((
                        v.next()?.parse::<u32>().ok()?,
                        v.next()?.parse::<u32>().ok()?,
                    ))
                });
            }
            if line.starts_with("deviceName") && !line.to_lowercase().contains("llvmpipe") {
                version = current;
            }
        }
        if let Some((major, minor)) = version {
            if !compatible_version(&settings.graphics_api, major, minor) {
                if let Ok(mut env) = state.environment.lock() {
                    env.suggested_graphics_api = Some(
                        if (major, minor) >= (1, 1) {
                            "sarek"
                        } else {
                            "opengl"
                        }
                        .into(),
                    );
                }
                return Err(AppError::coded("graphics_incompatible",format!("Выбранный профиль требует более нового Vulkan; GPU сообщает {major}.{minor}. Выберите предложенный 3D API в настройках.")));
            }
        }
    }
    let game = state.paths.game_dir(settings)?;
    let own = game.join(".radmir");
    let exe = own.join("probe.exe");
    let status = own.join("probe.status");
    fs::write(&exe, include_bytes!("../../runner/probe.exe"))?;
    let _ = fs::remove_file(&status);
    fs::write(
        own.join("probe.exe.ppdb"),
        crate::process::helper_profile(&game, settings)?,
    )?;
    let code = run_helper(state, &exe, &status, "PE32 DirectDraw/D3D9", true).await?;
    if code != 0 {
        return Err(AppError::coded(
            "graphics_probe_failed",
            format!("32-битная проверка графики завершилась с кодом {code}"),
        ));
    }
    Ok(())
}

pub async fn prepare_installation(
    state: Arc<AppState>,
    verify_only: bool,
) -> AppResult<PreparationReport> {
    let _guard = OperationGuard::begin(&state)?;
    let id = uuid::Uuid::new_v4().to_string();
    let settings = state.settings()?;
    *state
        .operation_id
        .lock()
        .map_err(|_| AppError::new("Блокировка операции"))? = Some(id.clone());
    state.events.operation(Some(id.clone()));
    state.events.clear("launcher://error");
    state.set_ready(None, false);
    state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .begin_preparation(&id, &settings)?;
    let mut backup = None;
    let result: AppResult<PreparationReport> = async {
        let environment_current = crate::installation::environment_current(&state, &settings)?;
        if !environment_current {
            ensure_environment(&state).await?;
            state.check_cancelled()?;
            backup = backup_prefix(&state, &settings).await?;
            phase(
                &state,
                "prefix-init",
                "Подготавливаем игровой Wine-префикс…",
            );
            if windows_command(&state, None, "", "Инициализация префикса").await? != 0
            {
                return Err(AppError::coded(
                    "prefix_failed",
                    "Wine-префикс не подготовлен.",
                ));
            }
        }
        phase(
            &state,
            "game-files",
            "PortProton готов. Проверяем и загружаем игру…",
        );
        let mut report = if verify_only {
            let r = updater::verify_inner(state.events.clone(), state.clone()).await?;
            PreparationReport {
                build: r.build,
                ready: r.ready,
                total_files: r.total_files,
                valid_files: r.valid_files,
                missing_files: r.missing_files,
                changed_files: r.changed_files,
                downloaded_files: 0,
                message: r.message,
            }
        } else {
            updater::prepare_inner(state.events.clone(), state.clone()).await?
        };
        if !report.ready {
            return Err(AppError::coded("game_files_incomplete", &report.message));
        }
        // Updates to game data alone do not recreate a healthy prefix. New
        // installers/driver/runtime fingerprints still require a PE32 probe.
        if !environment_current || !crate::installation::environment_current(&state, &settings)? {
            let installers = libraries(&state, &settings).await?;
            graphics_probe(&state, &settings).await?;
            fs::write(
                state.paths.state_dir.join("prefix-prepared.json"),
                serde_json::to_vec(&PrefixStamp {
                    wine: settings.wine_build.clone(),
                    api: settings.graphics_api.clone(),
                    installers,
                })?,
            )?;
        }
        if let Ok(mut env) = state.environment.lock() {
            env.launch_ready = true;
            env.phase = "ready".into();
            env.message = "PortProton, игровой клиент и 3D API готовы.".into();
            env.suggested_graphics_api = None;
        }
        crate::installation::record_success(&state, &settings)?;
        state.set_ready(Some(report.build.clone()), true);
        report.ready = true;
        report.message = "Игра и выбранная среда готовы к запуску.".into();
        let _ = state.events.emit(
            "launcher://ready",
            ReadyPayload {
                build: report.build.clone(),
                ready: true,
            },
        );
        Ok(report)
    }
    .await;
    if let Err(error) = &result {
        state.set_ready(None, false);
        if let Ok(mut env) = state.environment.lock() {
            env.launch_ready = false;
        }
        crate::installation::invalidate(&state, &settings)?;
        if let Some(backup) = backup {
            restore_prefix(&backup, &prefix(&settings))?;
        }
        state
            .db
            .lock()
            .map_err(|_| AppError::new("Блокировка БД"))?
            .rollback_runtime_selection()?;
        let _ = state.events.emit("launcher://error", error);
    } else {
        state
            .db
            .lock()
            .map_err(|_| AppError::new("Блокировка БД"))?
            .complete_runtime_selection()?;
    }
    let error = result.as_ref().err().map(ToString::to_string);
    state
        .db
        .lock()
        .map_err(|_| AppError::new("Блокировка БД"))?
        .finish_preparation(&id, error.as_deref())?;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn bundled_yad_version_252_is_not_a_broken_runtime() {
        use std::os::unix::fs::PermissionsExt;
        let f = crate::test_support::Fixture::new();
        let settings = f.state.settings().unwrap();
        let root = runtime_root(&settings);
        for (path, script) in [
            (
                root.join("data/themes/gui/yad_gui_pp"),
                "#!/bin/sh\nprintf '14.0 (GTK+ 3.24.52)\\n'\nexit 252\n",
            ),
            (
                root.join("data/tmp/plugins_v20/portable/bin/zstd"),
                "#!/bin/sh\nprintf 'Zstandard v1.4.8\\n'\n",
            ),
            (
                root.join("data/tmp/plugins_v20/portable/bin/vulkaninfo"),
                "#!/bin/sh\nexit 0\n",
            ),
        ] {
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, script).unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
        }
        validate_bundled_tools(&f.state).await.unwrap();
    }
    #[tokio::test]
    async fn broken_yad_still_fails_and_preserves_the_actual_error_in_the_log() {
        use std::os::unix::fs::PermissionsExt;
        for (code, message) in [
            (252, ""),
            (127, "error while loading shared libraries: missing.so"),
        ] {
            let f = crate::test_support::Fixture::new();
            let settings = f.state.settings().unwrap();
            let path = runtime_root(&settings).join("data/themes/gui/yad_gui_pp");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(
                &path,
                format!("#!/bin/sh\nprintf '%s\\n' '{message}' >&2\nexit {code}\n"),
            )
            .unwrap();
            fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            let error = validate_bundled_tools(&f.state).await.unwrap_err();
            assert_eq!(error.code(), "runtime_tool_failed");
            assert!(error.log_path().unwrap().ends_with("runtime-tools.log"));
            let log = fs::read_to_string(error.log_path().unwrap()).unwrap();
            assert!(log.contains(&format!("exit Some({code})")));
            assert!(log.contains(message));
        }
    }
    #[tokio::test]
    async fn working_bundled_yad_does_not_require_a_system_package() {
        use std::os::unix::fs::PermissionsExt;
        let f = crate::test_support::Fixture::new();
        let root = runtime_root(&f.state.settings().unwrap());
        let yad = root.join("data/themes/gui/yad_gui_pp");
        fs::create_dir_all(yad.parent().unwrap()).unwrap();
        fs::write(&yad, "#!/bin/sh\nprintf '14.0\\n'\n").unwrap();
        fs::set_permissions(&yad, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!dependency_plan(&f.state)
            .await
            .unwrap()
            .packages
            .iter()
            .any(|p| p == "yad"));
        assert_eq!(tool_path(&f.state.settings().unwrap(), "yad").unwrap(), yad);
    }
    #[test]
    fn failed_probe_survives_preflight_and_worker_restart_but_not_other_runtime() {
        let f = crate::test_support::Fixture::new();
        let settings = f.state.settings().unwrap();
        let key = graphics_context(&settings);
        let error = AppError::with_log(
            "missing_32bit_graphics",
            "Mesa i386 missing",
            &f.root.join("runtime.log"),
        );
        f.state
            .db
            .lock()
            .unwrap()
            .set_graphics_failure(&key, Some(&error))
            .unwrap();
        phase(&f.state, "preflight", "retry");
        restore_graphics_diagnostic(&f.state).unwrap();
        assert_eq!(
            f.state
                .environment
                .lock()
                .unwrap()
                .graphics_error
                .as_ref()
                .unwrap()["code"],
            "missing_32bit_graphics"
        );
        let paths = f.state.paths.clone();
        let db_path = paths.config_dir.join("launcher.db");
        let restarted = Arc::new(AppState::new(
            paths,
            crate::db::Database::open(&db_path).unwrap(),
        ));
        restore_graphics_diagnostic(&restarted).unwrap();
        assert!(restarted
            .environment
            .lock()
            .unwrap()
            .graphics_error
            .is_some());
        let mut other = settings.clone();
        other.graphics_api = "opengl".into();
        assert!(restarted
            .db
            .lock()
            .unwrap()
            .graphics_failure(&graphics_context(&other))
            .unwrap()
            .is_none());
        other = settings.clone();
        other.wine_build = "OTHER_WINE".into();
        assert!(restarted
            .db
            .lock()
            .unwrap()
            .graphics_failure(&graphics_context(&other))
            .unwrap()
            .is_none());
        restarted
            .db
            .lock()
            .unwrap()
            .set_graphics_failure(&key, None)
            .unwrap();
        restore_graphics_diagnostic(&restarted).unwrap();
        assert!(restarted
            .environment
            .lock()
            .unwrap()
            .graphics_error
            .is_none());
    }
    #[tokio::test]
    async fn expired_dependency_plan_is_rejected_before_authorization() {
        let f = crate::test_support::Fixture::new();
        assert_eq!(
            install_dependencies(&f.state, "expired-plan")
                .await
                .unwrap_err()
                .code(),
            "dependency_plan_changed"
        );
        assert!(!f
            .state
            .paths
            .state_dir
            .join("system-dependencies.log")
            .exists());
    }
    #[tokio::test]
    async fn missing_i386_mesa_error_contains_the_log_and_fails_without_waiting() {
        let (f, exe, status) = helper_fixture("printf '%s\\n' 'libGL error: MESA-LOADER: failed to open radeonsi: /usr/lib/pressure-vessel/overrides/lib/i386-linux-gnu/dri/radeonsi_dri.so: cannot open shared object file' >&2\nexit 1\n");
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            run_helper(&f.state, &exe, &status, "PE32", true),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error.code(), "missing_32bit_graphics");
        let value = serde_json::to_value(&error).unwrap();
        assert!(Path::new(value["log_path"].as_str().unwrap()).is_file());
        assert_eq!(value["retryable"], true);
    }
    #[test]
    fn installed_graphics_packages_and_unrelated_nvidia_do_not_add_mesa() {
        assert!(missing_graphics_packages("latest", "amd", true, |_| true).is_empty());
        assert!(
            !missing_graphics_packages("latest", "nvidia", false, |_| false)
                .iter()
                .any(|p| p.contains("mesa"))
        );
    }
    #[test]
    fn missing_mesa_probe_requires_the_actual_i386_dri_package() {
        let packages = missing_graphics_packages("latest", "amd", true, |_| false);
        assert!(packages.iter().any(|p| p == "libgl1-mesa-dri:i386"));
        assert!(packages.iter().any(|p| p == "mesa-vulkan-drivers:i386"));
    }
    #[test]
    fn opengl_probe_does_not_require_vulkan_packages() {
        let packages = missing_graphics_packages("opengl", "amd", true, |_| false);
        assert!(packages.iter().any(|p| p == "libgl1-mesa-dri:i386"));
        assert!(!packages.iter().any(|p| p.contains("vulkan")));
    }
    fn helper_fixture(script: &str) -> (crate::test_support::Fixture, PathBuf, PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let f = crate::test_support::Fixture::new();
        let root = runtime_root(&f.state.settings().unwrap());
        fs::create_dir_all(root.join("data/scripts")).unwrap();
        fs::write(root.join("data/scripts/start.sh"), script).unwrap();
        let wine = root.join("data/dist/PROTON_LG_10-28/bin/wine");
        fs::create_dir_all(wine.parent().unwrap()).unwrap();
        fs::write(&wine, "#!/bin/sh\nexit 0\n").unwrap();
        fs::set_permissions(&wine, fs::Permissions::from_mode(0o700)).unwrap();
        let exe = f.root.join("command.exe");
        fs::write(&exe, "fixture").unwrap();
        let status = exe.with_extension("status");
        (f, exe, status)
    }
    #[tokio::test]
    async fn helper_waits_for_wrapper_cleanup_before_the_next_launch() {
        let (f, exe, status) = helper_fixture(
            "printf 'ready 0\\n' > \"${2%.exe}.status\"\nsleep 0.4\ntouch cleanup-done\n",
        );
        assert_eq!(
            run_helper(&f.state, &exe, &status, "fixture", false)
                .await
                .unwrap(),
            0
        );
        assert!(
            f.root.join("cleanup-done").is_file(),
            "helper status precedes the PortProton EXIT trap"
        );
    }
    #[tokio::test]
    async fn helper_wrapper_exit_without_status_fails_promptly() {
        let (f, exe, status) = helper_fixture("exit 7\n");
        let error = tokio::time::timeout(
            Duration::from_secs(2),
            run_helper(&f.state, &exe, &status, "fixture", true),
        )
        .await
        .expect("exited wrapper must not wait for the 600-second timeout")
        .unwrap_err();
        assert_eq!(error.code(), "graphics_probe_failed");
        assert!(error.to_string().contains('7'));
        assert!(error.to_string().contains("runtime-"));
    }
    #[tokio::test]
    async fn failed_helper_also_waits_for_wrapper_cleanup() {
        let (f, exe, status) = helper_fixture(
            "printf 'failed-device 42\\n' > \"${2%.exe}.status\"\nsleep 0.4\ntouch cleanup-done\n",
        );
        let error = run_helper(&f.state, &exe, &status, "fixture", true)
            .await
            .unwrap_err();
        assert_eq!(error.code(), "graphics_probe_failed");
        assert!(f.root.join("cleanup-done").is_file());
    }
    #[tokio::test]
    async fn cancellation_allows_the_owned_wrapper_exit_trap_to_finish() {
        let (f, exe, status) = helper_fixture(
            "trap 'sleep 0.4; touch cleanup-done; exit 0' TERM\ntouch started\nwhile :; do sleep 0.1; done\n",
        );
        let state = f.state.clone();
        let started = f.root.join("started");
        let cancel = async move {
            while !started.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            state.cancelled.store(true, Ordering::SeqCst);
        };
        let (result, _) = tokio::join!(
            run_helper(&f.state, &exe, &status, "fixture", false),
            cancel
        );
        assert_eq!(result.unwrap_err().code(), "cancelled");
        assert!(f.root.join("cleanup-done").is_file());
    }
    #[test]
    fn graphics_mapping_uses_effective_function_not_stale_log() {
        assert_eq!(graphics_value("latest").unwrap(), "6");
        assert_eq!(graphics_value("stable").unwrap(), "2");
        assert_eq!(graphics_value("sarek").unwrap(), "1");
        assert_eq!(graphics_value("opengl").unwrap(), "0");
    }
    #[test]
    fn vulkan_12_is_not_silently_accepted_for_latest_dxvk() {
        assert!(!compatible_version("latest", 1, 2));
        assert!(compatible_version("latest", 1, 3));
        assert!(compatible_version("sarek", 1, 2));
        assert!(compatible_version("opengl", 0, 0));
    }
}
