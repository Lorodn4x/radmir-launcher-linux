use std::{
    collections::HashSet,
    fs::{self, File},
    io::{BufReader, Read},
    path::{Path, PathBuf},
    time::Duration,
};

use md5::Md5;
use sha2::{Digest, Sha256};

use crate::{
    db::validate_https_url,
    error::{AppError, AppResult},
    models::{GameManifest, ManifestFile, Settings},
    paths::safe_join,
};

const MAX_MANIFEST_BYTES: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HashAlgorithm {
    Md5,
    Sha256,
}

impl HashAlgorithm {
    pub fn from_hash(hash: &str) -> AppResult<Self> {
        if !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(AppError::new("Hash в manifest содержит не hex-символы."));
        }
        match hash.len() {
            32 => Ok(Self::Md5),
            64 => Ok(Self::Sha256),
            length => Err(AppError::new(format!(
                "Неподдерживаемая длина hash ({length}); ожидаются MD5/SHA256."
            ))),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileCheck {
    Valid,
    Missing,
    Changed,
}

pub async fn resolve_manifest(
    settings: &Settings,
    game_dir: &Path,
    client: &reqwest::Client,
) -> AppResult<ResolvedManifest> {
    if let Some(url) = &settings.manifest_url {
        match fetch_manifest(url, client).await {
            Ok(manifest) => {
                validate_manifest(&manifest)?;
                return Ok(ResolvedManifest {
                    manifest,
                    warning: None,
                    base_url: settings.download_base_url.clone(),
                    archives: Default::default(),
                });
            }
            Err(remote_error) => {
                let local_path = game_dir.join("manifest.json");
                if local_path.is_file() {
                    let manifest = load_local_manifest(game_dir)?;
                    return Ok(ResolvedManifest {
                        manifest,
                        warning: Some(format!(
                            "Сеть недоступна; использован локальный manifest. {remote_error}"
                        )),
                        base_url: None,
                        archives: Default::default(),
                    });
                }
                return Err(remote_error);
            }
        }
    }
    match crate::official::release(client).await {
        Ok(release) => Ok(ResolvedManifest {
            manifest: release.manifest,
            warning: None,
            base_url: Some(release.base_url),
            archives: release.archives,
        }),
        Err(error) => Ok(ResolvedManifest {
            manifest: load_local_manifest(game_dir)?,
            warning: Some(format!(
                "Проверена локальная сборка; обновления недоступны: {error}"
            )),
            base_url: None,
            archives: Default::default(),
        }),
    }
}

pub struct ResolvedManifest {
    pub manifest: GameManifest,
    pub warning: Option<String>,
    pub base_url: Option<String>,
    pub archives: std::collections::HashMap<i32, ManifestFile>,
}

pub fn load_local_manifest(game_dir: &Path) -> AppResult<GameManifest> {
    let path = game_dir.join("manifest.json");
    let bytes = fs::read(&path).map_err(|error| {
        AppError::new(format!(
            "Не удалось прочитать {}: {error}. Укажите официальный manifest URL в настройках.",
            path.display()
        ))
    })?;
    let manifest: GameManifest = serde_json::from_slice(&bytes)?;
    validate_manifest(&manifest)?;
    Ok(manifest)
}

pub(crate) async fn fetch_manifest(url: &str, client: &reqwest::Client) -> AppResult<GameManifest> {
    let url = validate_https_url(url)?;
    let response = client
        .get(url)
        .timeout(Duration::from_secs(45))
        .send()
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_MANIFEST_BYTES as u64)
    {
        return Err(AppError::new("Manifest превышает лимит 64 MiB."));
    }
    let bytes = response.bytes().await?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Err(AppError::new("Manifest превышает лимит 64 MiB."));
    }
    Ok(serde_json::from_slice(&bytes)?)
}

pub fn validate_manifest(manifest: &GameManifest) -> AppResult<()> {
    if manifest.build.trim().is_empty() {
        return Err(AppError::new("Manifest не содержит build."));
    }
    if manifest.files.is_empty() {
        return Err(AppError::new("Manifest не содержит файлов."));
    }
    let mut paths = HashSet::with_capacity(manifest.files.len());
    for file in &manifest.files {
        safe_join(Path::new("/"), &file.path)?;
        HashAlgorithm::from_hash(&file.hash)?;
        if !paths.insert(file.path.replace('\\', "/")) {
            return Err(AppError::new(format!(
                "Путь повторяется в manifest: {}",
                file.path
            )));
        }
        if let Some(url) = &file.url {
            validate_https_url(url)?;
        }
    }
    Ok(())
}

pub fn check_game_file(game_dir: &Path, file: &ManifestFile) -> AppResult<FileCheck> {
    let path = safe_join(game_dir, &file.path)?;
    check_file_at(&path, file)
}

pub fn check_file_at(path: &Path, file: &ManifestFile) -> AppResult<FileCheck> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(FileCheck::Missing)
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Ok(FileCheck::Changed);
    }
    if metadata.len() != file.size {
        return Ok(FileCheck::Changed);
    }
    let actual = hash_path(path, HashAlgorithm::from_hash(&file.hash)?)?;
    if actual.eq_ignore_ascii_case(&file.hash) {
        Ok(FileCheck::Valid)
    } else {
        Ok(FileCheck::Changed)
    }
}

pub fn hash_path(path: &Path, algorithm: HashAlgorithm) -> AppResult<String> {
    match algorithm {
        HashAlgorithm::Md5 => hash_with::<Md5>(path),
        HashAlgorithm::Sha256 => hash_with::<Sha256>(path),
    }
}

fn hash_with<D: Digest>(path: &Path) -> AppResult<String> {
    let file = File::open(path)?;
    let mut reader = BufReader::with_capacity(1024 * 1024, file);
    let mut hasher = D::new();
    let mut buffer = vec![0_u8; 1024 * 1024];
    loop {
        let read = reader.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex::encode(hasher.finalize()))
}

pub fn secure_destination(game_dir: &Path, file: &ManifestFile) -> AppResult<PathBuf> {
    fs::create_dir_all(game_dir)?;
    let canonical_root = game_dir.canonicalize()?;
    let destination = safe_join(&canonical_root, &file.path)?;
    let parent = destination
        .parent()
        .ok_or_else(|| AppError::new("Файл не имеет родительской директории."))?;
    // Walk existing parents before creating children: do not even mkdir
    // through a symlink that leads outside the installation.
    let mut current = canonical_root.clone();
    for component in parent
        .strip_prefix(&canonical_root)
        .map_err(|_| AppError::new("Некорректный родительский путь."))?
        .components()
    {
        current.push(component);
        if current.exists() {
            if !current.canonicalize()?.starts_with(&canonical_root) {
                return Err(AppError::new("Symlink ведёт за пределы каталога игры."));
            }
        } else {
            match fs::create_dir(&current) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    if !current.canonicalize()?.starts_with(&canonical_root) {
                        return Err(AppError::new("Symlink ведёт за пределы каталога игры."));
                    }
                }
                Err(error) => return Err(error.into()),
            }
        }
    }
    let canonical_parent = parent.canonicalize()?;
    if !canonical_parent.starts_with(&canonical_root) {
        return Err(AppError::new("Symlink ведёт за пределы каталога игры."));
    }
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if metadata.file_type().is_symlink() {
            return Err(AppError::new(format!(
                "Перезапись symlink запрещена: {}",
                file.path
            )));
        }
    }
    Ok(destination)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore = "read-only verification of an installed game; requires RADMIR_GAME_DIR"]
    async fn installed_game_integrity() {
        use futures_util::StreamExt;
        let root = PathBuf::from(std::env::var_os("RADMIR_GAME_DIR").expect("RADMIR_GAME_DIR"));
        let manifest = load_local_manifest(&root).unwrap();
        let mut tasks = futures_util::stream::iter(manifest.files.into_iter().map(|file| {
            let root = root.clone();
            tokio::task::spawn_blocking(move || (file.path.clone(), check_game_file(&root, &file)))
        }))
        .buffer_unordered(4);
        let mut valid = 0;
        let mut invalid = Vec::new();
        while let Some(result) = tasks.next().await {
            let (path, result) = result.unwrap();
            if result.unwrap() == FileCheck::Valid {
                valid += 1;
            } else {
                invalid.push(path);
            }
        }
        println!(
            "build={} valid={} invalid={} examples={:?}",
            manifest.build,
            valid,
            invalid.len(),
            &invalid[..invalid.len().min(10)]
        );
        assert!(invalid.is_empty(), "Installed game differs from manifest");
    }

    fn sample_file(hash: &str) -> ManifestFile {
        ManifestFile {
            hash: hash.into(),
            path: "data/test.bin".into(),
            archive: -1,
            size: 5,
            url: None,
        }
    }

    #[test]
    fn parses_observed_radmir_manifest_contract() {
        let manifest: GameManifest = serde_json::from_str(
            r#"{"build":"8.9.2","files":[{"hash":"5d41402abc4b2a76b9719d911017c592","path":"SAMP/SAMP.img","archive":-1,"size":5}]}"#,
        )
        .unwrap();
        validate_manifest(&manifest).unwrap();
        assert_eq!(manifest.files[0].archive, -1);
        assert_eq!(
            HashAlgorithm::from_hash(&manifest.files[0].hash).unwrap(),
            HashAlgorithm::Md5
        );
    }

    #[test]
    fn rejects_duplicate_paths_and_bad_hashes() {
        let file = sample_file("5d41402abc4b2a76b9719d911017c592");
        let mut manifest = GameManifest {
            build: "test".into(),
            files: vec![file.clone(), file],
        };
        assert!(validate_manifest(&manifest).is_err());
        manifest.files.truncate(1);
        manifest.files[0].hash = "not-a-hash".into();
        assert!(validate_manifest(&manifest).is_err());
    }

    #[test]
    fn verifies_md5_and_sha256_without_loading_whole_file() {
        let directory =
            std::env::temp_dir().join(format!("radmir-hash-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("hello.bin");
        fs::write(&path, b"hello").unwrap();
        assert_eq!(
            hash_path(&path, HashAlgorithm::Md5).unwrap(),
            "5d41402abc4b2a76b9719d911017c592"
        );
        assert_eq!(
            hash_path(&path, HashAlgorithm::Sha256).unwrap(),
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
        fs::remove_dir_all(directory).unwrap();
    }
}
