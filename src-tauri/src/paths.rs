use std::{
    env,
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
};

use crate::{
    error::{AppError, AppResult},
    models::{PathsInfo, Settings},
};

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub cache_dir: PathBuf,
    pub state_dir: PathBuf,
    pub default_game_dir: PathBuf,
}

impl AppPaths {
    pub fn discover() -> AppResult<Self> {
        let home = env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
            .ok_or_else(|| {
                AppError::new("Переменная HOME не задана; переносимый путь вычислить нельзя.")
            })?;
        if !home.is_absolute() {
            return Err(AppError::new("HOME должен содержать абсолютный путь."));
        }

        let config_dir = xdg_dir("XDG_CONFIG_HOME", &home, ".config")?.join("radmir-standalone");
        let cache_dir = xdg_dir("XDG_CACHE_HOME", &home, ".cache")?.join("radmir-standalone");
        let state_dir = xdg_dir("XDG_STATE_HOME", &home, ".local/state")?.join("radmir-standalone");
        let default_game_dir = home
            .join("PortProton")
            .join("prefixes")
            .join("DEFAULT")
            .join("drive_c")
            .join("Program Files (x86)")
            .join("RADMIR Games")
            .join("RADMIR CRMP");

        Ok(Self {
            config_dir,
            cache_dir,
            state_dir,
            default_game_dir,
        })
    }

    pub fn ensure_private_dirs(&self) -> AppResult<()> {
        for path in [&self.config_dir, &self.cache_dir, &self.state_dir] {
            fs::create_dir_all(path)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }

    pub fn game_dir(&self, settings: &Settings) -> AppResult<PathBuf> {
        if !settings.auto_game_path {
            if let Some(path) = &settings.game_path_override {
                let path = PathBuf::from(path);
                if !path.is_absolute() {
                    return Err(AppError::new(
                        "Пользовательский путь к игре должен быть абсолютным.",
                    ));
                }
                return Ok(path);
            }
        }
        let root = crate::portproton::native_root(settings);
        Ok(root.join("data/prefixes/DEFAULT/drive_c/Program Files (x86)/RADMIR Games/RADMIR CRMP"))
    }

    pub fn info(&self, settings: &Settings) -> AppResult<PathsInfo> {
        Ok(PathsInfo {
            game_dir: self.game_dir(settings)?.to_string_lossy().into_owned(),
            config_dir: self.config_dir.to_string_lossy().into_owned(),
            cache_dir: self.cache_dir.to_string_lossy().into_owned(),
            state_dir: self.state_dir.to_string_lossy().into_owned(),
        })
    }
}

fn xdg_dir(variable: &str, home: &Path, fallback: &str) -> AppResult<PathBuf> {
    match env::var_os(variable).filter(|value| !value.is_empty()) {
        Some(value) => {
            let path = PathBuf::from(value);
            if path.is_absolute() {
                Ok(path)
            } else {
                Ok(home.join(path))
            }
        }
        None => Ok(home.join(fallback)),
    }
}

pub fn safe_join(root: &Path, relative: &str) -> AppResult<PathBuf> {
    let normalized = relative.replace('\\', "/");
    if normalized.is_empty() || normalized.contains('\0') {
        return Err(AppError::new("Пустой или некорректный путь в manifest."));
    }
    let bytes = normalized.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        return Err(AppError::new(format!("Windows-путь запрещён: {relative}")));
    }
    let path = Path::new(&normalized);
    if path.is_absolute() {
        return Err(AppError::new(format!(
            "Абсолютный путь запрещён: {relative}"
        )));
    }
    for component in path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::new(format!(
                    "Небезопасный путь в manifest: {relative}"
                )));
            }
        }
    }
    let joined = root.join(path);
    if !joined.starts_with(root) {
        return Err(AppError::new("Путь выходит за каталог игры."));
    }
    Ok(joined)
}

pub fn find_in_path(program: &str) -> Option<PathBuf> {
    let paths: OsString = env::var_os("PATH")?;
    env::split_paths(&paths)
        .map(|directory| directory.join(program))
        .find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_join_accepts_nested_game_paths() {
        let root = Path::new("/home/example/game");
        assert_eq!(
            safe_join(root, "SAMP/SAMP.img").unwrap(),
            root.join("SAMP/SAMP.img")
        );
        assert_eq!(
            safe_join(root, "data\\file.dat").unwrap(),
            root.join("data/file.dat")
        );
    }

    #[test]
    fn safe_join_rejects_escape_and_absolute_paths() {
        let root = Path::new("/home/example/game");
        for path in [
            "../secret",
            "/etc/passwd",
            "C:/Windows/system.ini",
            "a/../../b",
        ] {
            assert!(
                safe_join(root, path).is_err(),
                "accepted unsafe path: {path}"
            );
        }
    }
}
