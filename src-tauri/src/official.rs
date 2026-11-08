//! Observed public RADMIR transport contracts, checked against live responses.
use crate::{
    error::{AppError, AppResult},
    manifest::validate_manifest,
    models::{GameManifest, ManifestFile, NewsItem, Server},
    paths::AppPaths,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashMap, path::Path, time::Duration};

pub const PROJECT_URL: &str =
    "http://launcher.hassle-games.com:3000/projects/crmp/v3/main/info.json";
const ONLINE_URL: &str = "http://launcher.hassle-games.com:3000/online.json";
const NEWS_URL: &str = "https://api.radmir.online/post/get?count=6&offset=0";
pub const CDNS: [&str; 2] = [
    "https://rdmr-cdn.samp-dl.com/game/main",
    "https://rdmr-cdn-ru.samp-dl.com/game/main",
];

pub async fn json<T: DeserializeOwned>(client: &reqwest::Client, url: &str) -> AppResult<T> {
    use futures_util::StreamExt;
    let response = client
        .get(url)
        .header("Referer", "https://radmir.online")
        .timeout(Duration::from_secs(25))
        .send()
        .await?
        .error_for_status()?;
    let mut bytes = Vec::new();
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk?;
        if bytes.len() + chunk.len() > 64 * 1024 * 1024 {
            return Err(AppError::new("JSON превышает лимит 64 MiB."));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(serde_json::from_slice(&bytes)?)
}

#[derive(Serialize, Deserialize)]
pub struct Project {
    pub cdn: Vec<String>,
    pub servers: Vec<ProjectServer>,
}
#[derive(Serialize, Deserialize)]
pub struct ProjectServer {
    pub id: u32,
    pub name: String,
    pub ip: String,
    pub port: u16,
}

async fn cached<T: DeserializeOwned + Serialize>(
    paths: &AppPaths,
    name: &str,
    url: &str,
) -> AppResult<T> {
    let client = crate::updater::http_client()?;
    let cache = paths.cache_dir.join(name);
    match json::<T>(&client, url).await {
        Ok(value) => {
            let part = cache.with_extension("part");
            if tokio::fs::write(&part, serde_json::to_vec(&value)?)
                .await
                .is_ok()
            {
                let _ = tokio::fs::rename(part, cache).await;
            }
            Ok(value)
        }
        Err(error) => {
            eprintln!("{name}: {error}; trying cache");
            match tokio::fs::read(cache).await {
                Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
                Err(_) => Err(error),
            }
        }
    }
}

pub async fn servers(paths: &AppPaths, game: &Path) -> AppResult<Vec<Server>> {
    let project = cached::<Project>(paths, "project.json", PROJECT_URL).await;
    let mut servers = match project {
        Ok(project) => project
            .servers
            .into_iter()
            .filter(|s| s.port > 0 && s.ip.parse::<std::net::IpAddr>().is_ok())
            .map(|s| Server {
                id: s.id.to_string(),
                name: s.name,
                host: s.ip,
                port: s.port,
                players: None,
                capacity: None,
                online: None,
                bonus: None,
            })
            .collect(),
        Err(_) => crate::servers::load_servers(game)?,
    };
    // Never show cached player counts as live. A failed poll clears them.
    if let Ok(value) = json::<Value>(&crate::updater::http_client()?, ONLINE_URL).await {
        for s in &mut servers {
            let record = &value["crmp_new"][&s.id];
            s.players = record["players"].as_u64().and_then(|n| n.try_into().ok());
            s.capacity = record["maxPlayers"]
                .as_u64()
                .filter(|n| *n > 0)
                .and_then(|n| n.try_into().ok());
            s.bonus = record["bonus"].as_u64().and_then(|n| n.try_into().ok());
            s.online = s.players.map(|_| s.capacity.unwrap_or(0) > 0);
        }
    }
    servers.sort_by_key(|s| {
        (
            s.players.unwrap_or(u32::MAX),
            s.id.parse::<u32>().unwrap_or(0),
        )
    });
    Ok(servers)
}

fn https(value: &Value) -> Option<String> {
    value
        .as_str()
        .filter(|s| crate::db::validate_https_url(s).is_ok())
        .map(str::to_owned)
}

pub async fn news(paths: &AppPaths) -> Vec<NewsItem> {
    let Ok(posts) = cached::<Vec<Value>>(paths, "news.json", NEWS_URL).await else {
        return Vec::new();
    };
    posts
        .into_iter()
        .take(6)
        .filter_map(|post| {
            Some(NewsItem {
                id: post["postId"].to_string(),
                title: post["title"].as_str()?.to_string(),
                summary: String::new(),
                published_at: post["createdAtHuman"].as_str().map(str::to_owned),
                target_url: https(&post["moreUrl"]),
                accent: "gold".into(),
                image_url: https(&post["images"][0]["url"]),
                video_url: https(&post["videoPreview"]),
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct BuildInfo {
    version: String,
    #[serde(rename = "basePath")]
    base_path: String,
}
#[derive(Deserialize)]
struct Files {
    list: Vec<ManifestFile>,
}

// Warm startup fetches only the small version document, not the file catalog.
pub async fn latest_version(client: &reqwest::Client) -> AppResult<String> {
    latest_version_at(client, &CDNS).await
}
async fn latest_version_at(client: &reqwest::Client, hosts: &[&str]) -> AppResult<String> {
    let mut last = AppError::new("CDN обновлений недоступны");
    for host in hosts {
        match json::<BuildInfo>(client, &format!("{host}/build-info.json")).await {
            Ok(build) => {
                crate::paths::safe_join(Path::new("/"), &build.base_path)?;
                if build.version.trim().is_empty() {
                    return Err(AppError::new("Пустая версия игры"));
                }
                return Ok(build.version);
            }
            Err(e) => last = e,
        }
    }
    Err(last)
}

pub struct Release {
    pub manifest: GameManifest,
    pub archives: HashMap<i32, ManifestFile>,
    pub base_url: String,
}

pub async fn release(client: &reqwest::Client) -> AppResult<Release> {
    let mut last = AppError::new("CDN недоступны.");
    for host in CDNS {
        match release_from(client, host).await {
            Ok(r) => return Ok(r),
            Err(e) => last = e,
        }
    }
    Err(last)
}

async fn release_from(client: &reqwest::Client, host: &str) -> AppResult<Release> {
    let build: BuildInfo = json(client, &format!("{host}/build-info.json")).await?;
    crate::paths::safe_join(Path::new("/"), &build.base_path)?;
    let base = format!("{host}/{}", build.base_path.trim_matches('/'));
    let files_url = format!("{base}/files.json");
    let archives_url = format!("{base}/archives.json");
    let (files, archives) = tokio::try_join!(
        json::<Files>(client, &files_url),
        json::<HashMap<i32, ManifestFile>>(client, &archives_url)
    )?;
    let manifest = GameManifest {
        build: build.version,
        files: files.list,
    };
    validate_manifest(&manifest)?;
    for (id, archive) in &archives {
        if *id < 0 {
            return Err(AppError::new("Некорректный номер архива."));
        }
        validate_manifest(&GameManifest {
            build: "archive".into(),
            files: vec![archive.clone()],
        })?;
    }
    for file in &manifest.files {
        if file.archive < -1 || (file.archive >= 0 && !archives.contains_key(&file.archive)) {
            return Err(AppError::new(format!(
                "Не найден архив {} для {}",
                file.archive, file.path
            )));
        }
    }
    Ok(Release {
        manifest,
        archives,
        base_url: format!("{base}/files/"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn warm_version_check_fetches_only_build_info_and_uses_the_mirror() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let fixture = tokio::spawn(async move {
            let mut paths = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().await.unwrap();
                let mut request = [0; 2048];
                let n = stream.read(&mut request).await.unwrap();
                let text = String::from_utf8_lossy(&request[..n]);
                let path = text
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_string();
                let (status, body) = if path.starts_with("/primary/") {
                    ("503 Service Unavailable", "offline")
                } else {
                    ("200 OK", r#"{"version":"8.9.4","basePath":"builds/8.9.4"}"#)
                };
                stream.write_all(format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
                paths.push(path);
            }
            paths
        });
        let primary = format!("http://{address}/primary");
        let mirror = format!("http://{address}/mirror");
        let client = crate::updater::http_client().unwrap();
        assert_eq!(
            latest_version_at(&client, &[&primary, &mirror])
                .await
                .unwrap(),
            "8.9.4"
        );
        assert_eq!(
            fixture.await.unwrap(),
            vec!["/primary/build-info.json", "/mirror/build-info.json"]
        );
    }
    #[tokio::test]
    #[ignore = "live official service smoke, explicitly invoked"]
    async fn live_release_contract() {
        let client = crate::updater::http_client().unwrap();
        let release = release(&client).await.unwrap();
        assert!(!release.archives.is_empty());
        assert!(release.manifest.files.len() > 1000);
        println!(
            "build={}, files={}, archives={}, base={}",
            release.manifest.build,
            release.manifest.files.len(),
            release.archives.len(),
            release.base_url
        );
        let small = release
            .manifest
            .files
            .iter()
            .filter(|f| f.archive == -1 && f.size > 0)
            .min_by_key(|f| f.size)
            .unwrap();
        let response = client
            .get(format!("{}{}", release.base_url, small.path))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let bytes = response.bytes().await.unwrap();
        use sha2::Digest;
        assert_eq!(
            hex::encode(md5::Md5::digest(&bytes)),
            small.hash.to_lowercase()
        );
        assert_eq!(bytes.len() as u64, small.size);
        println!("Direct-file hash verified: {}", small.path);
        let (id, archive) = release.archives.iter().min_by_key(|(_, a)| a.size).unwrap();
        let response = client
            .get(format!("{}{}", release.base_url, archive.path))
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap();
        let bytes = response.bytes().await.unwrap();
        assert_eq!(bytes.len() as u64, archive.size);
        assert_eq!(
            hex::encode(md5::Md5::digest(&bytes)),
            archive.hash.to_lowercase()
        );
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
        let member = release
            .manifest
            .files
            .iter()
            .filter(|f| f.archive == *id)
            .min_by_key(|f| f.size)
            .unwrap();
        use std::io::Read;
        let mut data = Vec::new();
        zip.by_name(&member.path)
            .unwrap()
            .read_to_end(&mut data)
            .unwrap();
        assert_eq!(
            hex::encode(md5::Md5::digest(&data)),
            member.hash.to_lowercase()
        );
        println!(
            "Archive and member hashes verified: {} / {}",
            archive.path, member.path
        );
    }
}
