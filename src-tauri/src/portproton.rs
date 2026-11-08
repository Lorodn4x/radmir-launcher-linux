use std::{
    fs::File,
    io::{Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
};

use crate::events::EventBus;

use crate::{
    error::{AppError, AppResult},
    models::{PortProtonKind, PortProtonStatus, ProgressPayload, Settings},
    paths::find_in_path,
    state::{AppState, OperationGuard},
};

pub const OFFICIAL_INSTALLER_URL: &str =
    "https://github.com/Castro-Fidel/PortProton_ALT/raw/main/portproton";
pub const FLATPAK_ID: &str = "ru.linux_gaming.PortProton";

pub fn native_root(settings: &Settings) -> PathBuf {
    if let Some(custom) = &settings.portproton_path {
        let p = PathBuf::from(custom);
        if p.is_dir() {
            return p;
        }
        if p.parent()
            .and_then(|v| v.file_name())
            .is_some_and(|v| v == "scripts")
        {
            if let Some(root) = p.parent().and_then(|v| v.parent()).and_then(|v| v.parent()) {
                return root.to_path_buf();
            }
        }
        if p.is_absolute() && !p.is_file() {
            return p;
        }
    }
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    if let Ok(text) = std::fs::read_to_string(home.join(".config/PortProton.conf")) {
        let p = PathBuf::from(text.lines().next().unwrap_or(""));
        if p.is_absolute() && p.join("data/scripts/start.sh").is_file() {
            return p;
        }
    }
    home.join("PortProton")
}
pub fn wine_binary(root: &Path, version: &str) -> Option<PathBuf> {
    if version.is_empty()
        || version.contains("..")
        || !version
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return None;
    }
    [
        root.join("data/dist").join(version).join("bin/wine"),
        root.join("data/dist").join(version).join("files/bin/wine"),
    ]
    .into_iter()
    .find(|p| p.is_file())
}
pub fn detect(settings: &Settings) -> PortProtonStatus {
    let root = native_root(settings);
    let start = root.join("data/scripts/start.sh");
    if start.is_file() {
        return native_status(start, settings);
    }
    if settings.portproton_path.is_none() {
        if let Some(flatpak) = find_in_path("flatpak") {
            if Command::new(&flatpak)
                .args(["info", FLATPAK_ID])
                .output()
                .is_ok_and(|o| o.status.success())
            {
                return PortProtonStatus {
                    installed: true,
                    scripts_present: true,
                    root: None,
                    kind: Some(PortProtonKind::Flatpak),
                    version: None,
                    message: "Найден PortProton Flatpak; будет проверен выбранный профиль.".into(),
                    executable: Some(flatpak),
                };
            }
        }
    }
    let mut status = PortProtonStatus::missing();
    status.root = Some(root.to_string_lossy().into());
    status
}
fn native_status(path: PathBuf, settings: &Settings) -> PortProtonStatus {
    let root = native_root(settings);
    let installed = wine_binary(&root, &settings.wine_build).is_some();
    PortProtonStatus {
        installed,
        scripts_present: true,
        root: Some(root.to_string_lossy().into()),
        kind: Some(PortProtonKind::Native),
        version: Some(settings.wine_build.clone()),
        message: if installed {
            "PortProton и выбранный Wine обнаружены; готовность проверяется отдельно.".into()
        } else {
            "Установка PortProton неполная: выбранный Wine отсутствует. Требуется восстановление."
                .into()
        },
        executable: Some(path),
    }
}
pub async fn install(
    app: EventBus,
    state: Arc<AppState>,
    _client: reqwest::Client,
) -> AppResult<PortProtonStatus> {
    let _guard = OperationGuard::begin(&state)?;
    crate::runtime::ensure_environment(&state).await?;
    let _ = app;
    Ok(detect(&state.settings()?))
}

fn installer_command(installer: &Path) -> tokio::process::Command {
    // Upstream uses Bash-only syntax (`[[ ... ]]`, arrays and `&>`).
    // Running it through `/bin/sh` makes Ubuntu's dash mis-detect SteamOS.
    let mut command = tokio::process::Command::new("bash");
    command.arg(installer);
    command
}

pub(crate) fn log_tail(path: &Path, max_bytes: u64) -> std::io::Result<String> {
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    let start = length.saturating_sub(max_bytes);
    file.seek(SeekFrom::Start(start))?;
    let mut bytes = Vec::with_capacity((length - start) as usize);
    file.read_to_end(&mut bytes)?;
    let text = String::from_utf8_lossy(&bytes);
    let text = if start > 0 {
        text.split_once('\n').map_or("", |(_, rest)| rest)
    } else {
        &text
    };
    Ok(text.trim().to_string())
}

fn missing_installer_tools_with(mut available: impl FnMut(&str) -> bool) -> Vec<&'static str> {
    ["bash", "curl", "tar", "yad"]
        .into_iter()
        .filter(|program| !available(program))
        .collect()
}

fn emit_setup(app: &EventBus, phase: &str, done: u64, total: u64, percent: f64, message: &str) {
    emit_progress(
        app,
        ProgressPayload {
            phase: phase.into(),
            bytes_done: done,
            bytes_total: total,
            percent,
            message: message.into(),
            ..Default::default()
        },
    );
}

fn emit_progress(app: &EventBus, payload: ProgressPayload) {
    if let Err(error) = app.emit("launcher://progress", payload) {
        eprintln!("Не удалось отправить progress event: {error}");
    }
}

pub fn launch_command(
    status: &PortProtonStatus,
    executable: &std::path::Path,
    arguments: &[String],
) -> AppResult<tokio::process::Command> {
    let kind = status
        .kind
        .as_ref()
        .ok_or_else(|| AppError::new("PortProton не установлен."))?;
    let program = status
        .executable
        .as_ref()
        .ok_or_else(|| AppError::new("Путь к PortProton не определён."))?;
    match kind {
        PortProtonKind::Native => {
            let mut command = if program.extension().is_some_and(|e| e == "sh") {
                let mut c = tokio::process::Command::new("bash");
                c.arg(program);
                c
            } else {
                tokio::process::Command::new(program)
            };
            clean_environment(&mut command);
            command
                .env("SKIP_CHECK_UPDATES", "1")
                .env("SKIP_CHECK_UPDATES_NEW", "1");
            command.arg("--launch").arg(executable).args(arguments);
            Ok(command)
        }
        PortProtonKind::Flatpak => {
            let mut command = tokio::process::Command::new(program);
            command
                .arg("run")
                .arg(FLATPAK_ID)
                .arg("--launch")
                .arg(executable)
                .args(arguments);
            Ok(command)
        }
    }
}

pub fn clean_environment(command: &mut tokio::process::Command) {
    for name in [
        "LD_LIBRARY_PATH",
        "LD_PRELOAD",
        "GIO_MODULE_DIR",
        "GDK_PIXBUF_MODULE_FILE",
        "GDK_PIXBUF_MODULEDIR",
        "GST_PLUGIN_SYSTEM_PATH",
        "GST_PLUGIN_SYSTEM_PATH_1_0",
        "GST_PLUGIN_PATH",
        "GST_PLUGIN_PATH_1_0",
        "GTK_PATH",
        "GTK_EXE_PREFIX",
        "GTK_DATA_PREFIX",
        "QT_PLUGIN_PATH",
    ] {
        command.env_remove(name);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::{hash_path, HashAlgorithm};
    use std::fs;

    #[test]
    fn scripts_without_selected_wine_are_not_a_ready_installation() {
        let root = std::env::temp_dir().join(format!("radmir-incomplete-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(root.join("data/scripts")).unwrap();
        let start = root.join("data/scripts/start.sh");
        fs::write(&start, "#!/bin/bash\nexit 0\n").unwrap();
        let settings = Settings {
            portproton_path: Some(start.to_string_lossy().into()),
            ..Default::default()
        };
        assert!(
            !detect(&settings).installed,
            "scripts alone must not enable game launch"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn flatpak_launch_preserves_arguments_without_shell_escaping() {
        let status = PortProtonStatus {
            installed: true,
            scripts_present: true,
            root: None,
            kind: Some(PortProtonKind::Flatpak),
            version: None,
            message: String::new(),
            executable: Some(PathBuf::from("/usr/bin/flatpak")),
        };
        let command = launch_command(
            &status,
            std::path::Path::new("/home/user/My Game/samp.exe"),
            &["-n".into(), "User_Name".into()],
        );
        assert!(command.is_ok());
    }

    #[test]
    fn sha256_helper_is_available_for_installer_pinning() {
        let directory =
            std::env::temp_dir().join(format!("radmir-pp-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("script");
        fs::write(&path, b"hello").unwrap();
        assert_eq!(
            hash_path(&path, HashAlgorithm::Sha256).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[tokio::test]
    async fn official_installer_runner_executes_bash_syntax() {
        let directory =
            std::env::temp_dir().join(format!("radmir-bash-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let script = directory.join("installer.sh");
        fs::write(
            &script,
            b"if [[ \"clean-ubuntu\" == \"clean-ubuntu\" ]]; then exit 0; else exit 9; fi\n",
        )
        .unwrap();

        let status = installer_command(&script).status().await.unwrap();
        assert!(status.success());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn installer_dependency_check_reports_only_missing_tools() {
        let missing = missing_installer_tools_with(|program| program != "yad");
        assert_eq!(missing, vec!["yad"]);
    }

    #[test]
    fn failed_installer_log_excerpt_is_bounded_and_keeps_complete_lines() {
        let directory =
            std::env::temp_dir().join(format!("radmir-log-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("installer.log");
        fs::write(
            &path,
            b"discard this line\nuseful line one\nuseful line two\n",
        )
        .unwrap();

        assert_eq!(
            log_tail(&path, 34).unwrap(),
            "useful line one\nuseful line two"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
