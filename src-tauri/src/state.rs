use crate::{
    db::Database,
    error::{AppError, AppResult},
    events::EventBus,
    locks::{resource_key, FileLock},
    models::{EnvironmentReport, GameProcess, PreparationStatus, Settings, UpdateStatus},
    paths::AppPaths,
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
pub struct AppState {
    pub paths: AppPaths,
    pub db: Mutex<Database>,
    pub ready: AtomicBool,
    pub files_ready: AtomicBool,
    pub ready_build: Mutex<Option<String>>,
    pub operation_running: AtomicBool,
    pub game_running: AtomicBool,
    pub cancelled: AtomicBool,
    pub process: Mutex<Option<GameProcess>>,
    pub environment: Mutex<EnvironmentReport>,
    pub operation_id: Mutex<Option<String>>,
    pub updates: Mutex<UpdateStatus>,
    pub update_check: tokio::sync::Mutex<()>,
    pub events: EventBus,
}
impl AppState {
    pub fn new(paths: AppPaths, db: Database) -> Self {
        Self {
            paths,
            db: Mutex::new(db),
            ready: AtomicBool::new(false),
            files_ready: AtomicBool::new(false),
            ready_build: Mutex::new(None),
            operation_running: AtomicBool::new(false),
            game_running: AtomicBool::new(false),
            cancelled: AtomicBool::new(false),
            process: Mutex::new(None),
            environment: Mutex::new(EnvironmentReport::default()),
            operation_id: Mutex::new(None),
            updates: Mutex::new(UpdateStatus::default()),
            update_check: tokio::sync::Mutex::new(()),
            events: EventBus::default(),
        }
    }
    pub fn settings(&self) -> AppResult<Settings> {
        self.db
            .lock()
            .map_err(|_| AppError::new("Повреждена блокировка БД."))?
            .load_settings()
    }
    pub fn set_ready(&self, build: Option<String>, ready: bool) {
        self.ready.store(ready, Ordering::SeqCst);
        if let Ok(mut value) = self.ready_build.lock() {
            *value = if ready { build } else { None };
        }
    }
    pub fn set_files_ready(&self, build: Option<String>, ready: bool) {
        self.files_ready.store(ready, Ordering::SeqCst);
        if let Ok(mut value) = self.ready_build.lock() {
            *value = if ready { build } else { None };
        }
    }
    pub fn process(&self) -> Option<GameProcess> {
        self.process.lock().ok()?.clone()
    }
    pub fn check_cancelled(&self) -> AppResult<()> {
        if self.cancelled.load(Ordering::SeqCst) {
            Err(AppError::coded(
                "cancelled",
                "Подготовка остановлена. Можно продолжить повторным запуском.",
            ))
        } else {
            Ok(())
        }
    }
    pub fn snapshot(&self, since: u64) -> PreparationStatus {
        let events = self.events.since(since);
        let latest = self.events.since(0);
        let progress = latest
            .iter()
            .find(|e| e.event == "launcher://progress")
            .and_then(|e| serde_json::from_value(e.payload.clone()).ok());
        let error = latest
            .iter()
            .find(|e| e.event == "launcher://error")
            .map(|e| e.payload.clone());
        PreparationStatus {
            event_sequence: self.events.sequence(),
            settings: self.settings().unwrap_or_default(),
            operation_id: self.operation_id.lock().ok().and_then(|v| v.clone()),
            active: self.operation_running.load(Ordering::SeqCst),
            ready: self.ready.load(Ordering::SeqCst),
            files_ready: self.files_ready.load(Ordering::SeqCst),
            environment: self
                .environment
                .lock()
                .map(|v| v.clone())
                .unwrap_or_default(),
            process: self.process(),
            events,
            progress,
            error,
            updates: self.updates.lock().map(|v| v.clone()).unwrap_or_default(),
        }
    }
    pub fn resource_locks(&self) -> AppResult<Vec<FileLock>> {
        let settings = self.settings()?;
        let root = crate::portproton::native_root(&settings);
        let game = self.paths.game_dir(&settings)?;
        let mut names = vec![
            format!("portproton-{}", resource_key(&root)),
            format!(
                "prefix-{}",
                resource_key(&crate::runtime::prefix(&settings))
            ),
            format!("game-{}", resource_key(&game)),
        ];
        names.sort();
        names.dedup();
        names.iter().map(|n| FileLock::acquire(n)).collect()
    }
}
pub struct OperationGuard<'a> {
    flag: &'a AtomicBool,
    _locks: Vec<FileLock>,
}
impl<'a> OperationGuard<'a> {
    pub fn begin(state: &'a Arc<AppState>) -> AppResult<Self> {
        if state.game_running.load(Ordering::SeqCst) {
            return Err(AppError::new(
                "Закройте игру перед изменением среды или файлов.",
            ));
        }
        state
            .operation_running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map_err(|_| AppError::new("Подготовка уже выполняется."))?;
        let locks = match state.resource_locks() {
            Ok(v) => v,
            Err(e) => {
                state.operation_running.store(false, Ordering::SeqCst);
                return Err(e);
            }
        };
        if state.game_running.load(Ordering::SeqCst) {
            state.operation_running.store(false, Ordering::SeqCst);
            return Err(AppError::new("Игра уже запущена."));
        }
        state.cancelled.store(false, Ordering::SeqCst);
        Ok(Self {
            flag: &state.operation_running,
            _locks: locks,
        })
    }
}
impl Drop for OperationGuard<'_> {
    fn drop(&mut self) {
        self.flag.store(false, Ordering::SeqCst);
    }
}
