use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub nickname: String,
    pub selected_server_id: Option<String>,
    pub auto_game_path: bool,
    pub game_path_override: Option<String>,
    pub manifest_url: Option<String>,
    pub download_base_url: Option<String>,
    pub portproton_path: Option<String>,
    pub auto_install_portproton: bool,
    pub installer_sha256: Option<String>,
    pub download_concurrency: usize,
    pub hash_concurrency: usize,
    pub graphics_preset: String,
    pub wine_build: String,
    pub graphics_api: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            nickname: "Name_Surname".into(),
            selected_server_id: None,
            auto_game_path: true,
            game_path_override: None,
            manifest_url: None,
            download_base_url: None,
            portproton_path: None,
            auto_install_portproton: true,
            installer_sha256: None,
            download_concurrency: 4,
            hash_concurrency: 4,
            graphics_preset: "enhanced".into(),
            wine_build: "PROTON_LG_10-28".into(),
            graphics_api: "latest".into(),
        }
    }
}

impl Settings {
    pub fn normalized(mut self) -> Self {
        self.nickname = self.nickname.trim().to_string();
        normalize_option(&mut self.game_path_override);
        normalize_option(&mut self.manifest_url);
        normalize_option(&mut self.download_base_url);
        normalize_option(&mut self.portproton_path);
        normalize_option(&mut self.installer_sha256);
        normalize_option(&mut self.selected_server_id);
        self
    }
}

fn normalize_option(value: &mut Option<String>) {
    if let Some(text) = value {
        let trimmed = text.trim().to_string();
        if trimmed.is_empty() {
            *value = None;
        } else {
            *text = trimmed;
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathsInfo {
    pub game_dir: String,
    pub config_dir: String,
    pub cache_dir: String,
    pub state_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PortProtonKind {
    Native,
    Flatpak,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortProtonStatus {
    #[serde(default)]
    pub scripts_present: bool,
    #[serde(default)]
    pub root: Option<String>,
    pub installed: bool,
    pub kind: Option<PortProtonKind>,
    pub version: Option<String>,
    pub message: String,
    #[serde(skip)]
    pub executable: Option<std::path::PathBuf>,
}

impl PortProtonStatus {
    pub fn missing() -> Self {
        Self {
            installed: false,
            scripts_present: false,
            root: None,
            kind: None,
            version: None,
            message: "PortProton не найден. Можно запустить официальную фоновую установку.".into(),
            executable: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub online: Option<bool>,
    pub players: Option<u32>,
    pub capacity: Option<u32>,
    pub bonus: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewsItem {
    pub id: String,
    pub title: String,
    pub summary: String,
    pub published_at: Option<String>,
    pub target_url: Option<String>,
    pub accent: String,
    pub image_url: Option<String>,
    pub video_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameManifest {
    pub build: String,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ManifestFile {
    pub hash: String,
    pub path: String,
    #[serde(default = "default_archive")]
    pub archive: i32,
    pub size: u64,
    #[serde(default)]
    pub url: Option<String>,
}

const fn default_archive() -> i32 {
    -1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VerificationReport {
    pub build: String,
    pub ready: bool,
    pub total_files: usize,
    pub valid_files: usize,
    pub missing_files: usize,
    pub changed_files: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreparationReport {
    pub build: String,
    pub ready: bool,
    pub total_files: usize,
    pub valid_files: usize,
    pub missing_files: usize,
    pub changed_files: usize,
    pub downloaded_files: usize,
    pub message: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProgressPayload {
    pub phase: String,
    pub current_file: Option<String>,
    pub bytes_done: u64,
    pub bytes_total: u64,
    pub files_done: usize,
    pub files_total: usize,
    pub bytes_per_second: f64,
    pub percent: f64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReadyPayload {
    pub build: String,
    pub ready: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub message: String,
    pub retryable: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GameProcess {
    pub id: String,
    pub pid: u32,
    pub executable: String,
    pub server_id: Option<String>,
    pub status: String,
    pub exit_code: Option<i32>,
    pub log_path: String,
    pub started_at: String,
    pub ended_at: Option<String>,
    #[serde(default)]
    pub windows_pid: Option<u32>,
    #[serde(default)]
    pub identity: Option<crate::locks::ProcessIdentity>,
    #[serde(default)]
    pub token_port: Option<u16>,
    #[serde(default)]
    pub status_path: Option<String>,
    #[serde(default)]
    pub prefix: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bootstrap {
    pub settings: Settings,
    pub paths: PathsInfo,
    pub portproton: PortProtonStatus,
    pub servers: Vec<Server>,
    pub process: Option<GameProcess>,
    pub news: Vec<NewsItem>,
    pub environment: EnvironmentReport,
    pub preparation: PreparationStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnvironmentReport {
    pub runtime_ready: bool,
    pub launch_ready: bool,
    pub phase: String,
    pub root: Option<String>,
    pub prefix: Option<String>,
    pub wine_build: String,
    pub graphics_api: String,
    pub message: String,
    pub suggested_graphics_api: Option<String>,
    #[serde(default)]
    pub graphics_error: Option<serde_json::Value>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuntimeVersion {
    pub id: String,
    pub label: String,
    pub installed: bool,
    pub downloadable: bool,
    pub source: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GraphicsProfile {
    pub id: String,
    pub label: String,
    pub portproton_value: String,
    pub dxvk_version: Option<String>,
    pub vkd3d_version: Option<String>,
    pub minimum_vulkan: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DependencyPlan {
    pub id: String,
    pub packages: Vec<String>,
    pub enable_i386: bool,
    pub changes: String,
    pub blocked: Option<String>,
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PreparationStatus {
    pub event_sequence: u64,
    pub settings: Settings,
    pub operation_id: Option<String>,
    pub active: bool,
    pub ready: bool,
    pub files_ready: bool,
    pub environment: EnvironmentReport,
    pub process: Option<GameProcess>,
    pub events: Vec<crate::events::EventFrame>,
    pub progress: Option<ProgressPayload>,
    pub error: Option<serde_json::Value>,
    pub updates: UpdateStatus,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub status: String,
    pub installed_build: Option<String>,
    pub latest_build: Option<String>,
    pub message: String,
}
