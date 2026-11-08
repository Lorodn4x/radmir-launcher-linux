use crate::{db::Database, paths::AppPaths, state::AppState};
use std::{fs, path::PathBuf, sync::Arc};
pub struct Fixture {
    pub root: PathBuf,
    pub state: Arc<AppState>,
}
impl Fixture {
    pub fn new() -> Self {
        let root = std::env::temp_dir().join(format!("radmir-test-{}", uuid::Uuid::new_v4()));
        let paths = AppPaths {
            config_dir: root.join("config"),
            cache_dir: root.join("cache"),
            state_dir: root.join("state"),
            default_game_dir: root.join("game"),
        };
        paths.ensure_private_dirs().unwrap();
        let db = Database::open(&paths.config_dir.join("launcher.db")).unwrap();
        let state = Arc::new(AppState::new(paths, db));
        let mut settings = state.settings().unwrap();
        settings.portproton_path = Some(root.join("PortProton").to_string_lossy().into());
        state.db.lock().unwrap().save_settings(&settings).unwrap();
        Self { root, state }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}
