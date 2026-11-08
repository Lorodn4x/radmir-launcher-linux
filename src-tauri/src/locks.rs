//! Locks and process identity survive individual GUI instances.
use crate::error::{AppError, AppResult};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    os::unix::{
        fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
        io::AsRawFd,
    },
    path::{Path, PathBuf},
};

pub fn private_runtime_dir() -> AppResult<PathBuf> {
    let uid = unsafe { libc::geteuid() };
    let run = PathBuf::from(format!("/run/user/{uid}"));
    let path = if run.is_dir() {
        run.join("radmir-standalone")
    } else {
        std::env::temp_dir().join(format!("radmir-standalone-{uid}"))
    };
    fs::create_dir_all(&path)?;
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.file_type().is_symlink() {
        return Err(AppError::new("Небезопасный служебный каталог runtime."));
    }
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700))?;
    Ok(path)
}

pub fn resource_key(path: &Path) -> String {
    let mut existing = path.to_path_buf();
    let mut tail = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            break;
        };
        tail.push(name.to_os_string());
        if !existing.pop() {
            break;
        }
    }
    let mut normalized = existing.canonicalize().unwrap_or(existing);
    for name in tail.into_iter().rev() {
        normalized.push(name);
    }
    hex::encode(Sha256::digest(normalized.as_os_str().as_encoded_bytes()))
}

pub struct FileLock(File);
impl FileLock {
    pub fn acquire(name: &str) -> AppResult<Self> {
        if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(AppError::new("Некорректное имя блокировки."));
        }
        let path = private_runtime_dir()?.join(format!("{name}.lock"));
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        if file.metadata()?.uid() != unsafe { libc::geteuid() } {
            return Err(AppError::new(
                "Владелец блокировки не совпадает с пользователем.",
            ));
        }
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
            return Err(AppError::new(
                "Другая операция уже использует этот PortProton или префикс.",
            ));
        }
        Ok(Self(file))
    }
}
impl Drop for FileLock {
    fn drop(&mut self) {
        unsafe {
            libc::flock(self.0.as_raw_fd(), libc::LOCK_UN);
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_ticks: u64,
    pub boot_id: String,
}
impl ProcessIdentity {
    pub fn read(pid: u32) -> std::io::Result<Self> {
        let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
        let fields = stat
            .rsplit_once(')')
            .ok_or_else(|| std::io::Error::other("proc stat"))?
            .1;
        let start_ticks = fields
            .split_whitespace()
            .nth(19)
            .ok_or_else(|| std::io::Error::other("proc start"))?
            .parse()
            .map_err(|_| std::io::Error::other("proc start"))?;
        Ok(Self {
            pid,
            start_ticks,
            boot_id: fs::read_to_string("/proc/sys/kernel/random/boot_id")?
                .trim()
                .into(),
        })
    }
    pub fn alive(&self) -> bool {
        Self::read(self.pid)
            .is_ok_and(|v| v.start_ticks == self.start_ticks && v.boot_id == self.boot_id)
    }
}

pub fn available_bytes(path: &Path) -> AppResult<u64> {
    let mut parent = path;
    while !parent.exists() {
        parent = parent
            .parent()
            .ok_or_else(|| AppError::new("Нет родителя для проверки места."))?;
    }
    let name = std::ffi::CString::new(parent.as_os_str().as_encoded_bytes())
        .map_err(|_| AppError::new("Некорректный путь."))?;
    let mut info = std::mem::MaybeUninit::<libc::statvfs>::uninit();
    if unsafe { libc::statvfs(name.as_ptr(), info.as_mut_ptr()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let info = unsafe { info.assume_init() };
    Ok(info.f_bavail.saturating_mul(info.f_frsize))
}

pub fn require_space(path: &Path, bytes: u64) -> AppResult<()> {
    let available = available_bytes(path)?;
    let required = bytes.saturating_add(1024 * 1024 * 1024);
    if available < required {
        return Err(AppError::coded(
            "insufficient_space",
            format!(
                "Недостаточно места: нужно {:.1} ГиБ, свободно {:.1} ГиБ ({})",
                required as f64 / 1073741824.0,
                available as f64 / 1073741824.0,
                path.display()
            ),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn lock_excludes_other_instances_until_owner_drops() {
        let name = format!("test-{}", uuid::Uuid::new_v4());
        let lock = FileLock::acquire(&name).unwrap();
        assert!(FileLock::acquire(&name).is_err());
        drop(lock);
        assert!(FileLock::acquire(&name).is_ok());
    }
    #[test]
    fn recycled_pid_or_boot_is_not_a_live_session() {
        let mut id = ProcessIdentity::read(std::process::id()).unwrap();
        assert!(id.alive());
        id.start_ticks += 1;
        assert!(!id.alive());
        id = ProcessIdentity::read(std::process::id()).unwrap();
        id.boot_id.push('x');
        assert!(!id.alive());
    }
    #[test]
    fn impossible_disk_requirement_is_rejected_before_download() {
        assert!(require_space(std::path::Path::new("/tmp"), u64::MAX).is_err());
    }
}
