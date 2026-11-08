//! Same executable, separate lifetime: UI requests never own installation tasks.
use crate::{
    db::{validate_settings, Database},
    error::{AppError, AppResult},
    locks::{private_runtime_dir, resource_key, FileLock},
    models::Settings,
    paths::AppPaths,
    state::AppState,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    os::unix::{fs::PermissionsExt, process::CommandExt},
    path::PathBuf,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
};
const LIMIT: u64 = 1024 * 1024;
#[derive(Serialize, Deserialize)]
struct Request {
    command: String,
    #[serde(default)]
    args: Value,
}
#[derive(Clone)]
pub struct WorkerClient {
    pub paths: AppPaths,
    socket: PathBuf,
    program: PathBuf,
}

fn socket_path(paths: &AppPaths) -> AppResult<PathBuf> {
    Ok(private_runtime_dir()?.join(format!("{}.sock", resource_key(&paths.state_dir))))
}
impl WorkerClient {
    pub fn new(paths: AppPaths) -> AppResult<Self> {
        let socket = socket_path(&paths)?;
        let program = if let Some(image) = std::env::var_os("APPIMAGE") {
            let source = PathBuf::from(image);
            let hash = crate::manifest::hash_path(&source, crate::manifest::HashAlgorithm::Sha256)?;
            let folder = paths
                .cache_dir
                .join("worker-images")
                .join(env!("CARGO_PKG_VERSION"));
            fs::create_dir_all(&folder)?;
            let dest = folder.join(format!("{hash}.AppImage"));
            if !dest.is_file()
                || crate::manifest::hash_path(&dest, crate::manifest::HashAlgorithm::Sha256)?
                    != hash
            {
                crate::locks::require_space(&folder, fs::metadata(&source)?.len())?;
                let part = folder.join(format!(".image-{}", uuid::Uuid::new_v4()));
                fs::copy(source, &part)?;
                fs::set_permissions(&part, fs::Permissions::from_mode(0o700))?;
                fs::rename(part, &dest)?;
            }
            dest
        } else {
            std::env::current_exe()?
        };
        Ok(Self {
            paths,
            socket,
            program,
        })
    }
    fn spawn(&self) -> AppResult<()> {
        let log = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.paths.state_dir.join("worker.log"))?;
        let mut cmd = std::process::Command::new(&self.program);
        cmd.arg("--worker")
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::from(log.try_clone()?))
            .stderr(std::process::Stdio::from(log));
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        cmd.spawn()?;
        Ok(())
    }
    pub async fn call(&self, command: &str, args: Value) -> AppResult<Value> {
        let mut stream = match UnixStream::connect(&self.socket).await {
            Ok(s) => s,
            Err(_) => {
                self.spawn()?;
                let start = Instant::now();
                loop {
                    match UnixStream::connect(&self.socket).await {
                        Ok(s) => break s,
                        Err(_) => {
                            if start.elapsed() > Duration::from_secs(20) {
                                return Err(AppError::coded(
                                    "worker_unavailable",
                                    "Фоновый процесс не запустился. Откройте worker.log.",
                                ));
                            }
                            tokio::time::sleep(Duration::from_millis(100)).await;
                        }
                    }
                }
            }
        };
        if stream.peer_cred()?.uid() != unsafe { libc::geteuid() } {
            return Err(AppError::new(
                "Владелец фонового процесса не совпадает с пользователем.",
            ));
        }
        let request = serde_json::to_vec(&Request {
            command: command.into(),
            args,
        })?;
        if request.len() as u64 >= LIMIT {
            return Err(AppError::new("Слишком большой запрос worker."));
        }
        stream.write_all(&request).await?;
        stream.write_all(b"\n").await?;
        let mut reader = BufReader::new(stream);
        let mut response = String::new();
        // Runtime requests can finish after the GUI closes; no GUI deadline
        // cancels the server task. The bounded frame limits only message size.
        tokio::io::AsyncReadExt::take(&mut reader, LIMIT)
            .read_line(&mut response)
            .await?;
        let value: Value = serde_json::from_str(&response)?;
        if let Some(error) = value.get("error") {
            if let Some(path) = error["log_path"].as_str() {
                return Err(AppError::with_log(
                    error["code"].as_str().unwrap_or("worker_error"),
                    error["message"]
                        .as_str()
                        .unwrap_or("Ошибка фонового процесса."),
                    std::path::Path::new(path),
                ));
            }
            return Err(AppError::coded(
                error["code"].as_str().unwrap_or("worker_error"),
                error["message"]
                    .as_str()
                    .unwrap_or("Ошибка фонового процесса."),
            ));
        }
        Ok(value["result"].clone())
    }
}

pub fn run() -> AppResult<()> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(server())
}
async fn server() -> AppResult<()> {
    let paths = AppPaths::discover()?;
    paths.ensure_private_dirs()?;
    let _owner = FileLock::acquire(&format!("worker-{}", resource_key(&paths.state_dir)))?;
    let socket = socket_path(&paths)?;
    let _ = fs::remove_file(&socket);
    let listener = UnixListener::bind(&socket)?;
    fs::set_permissions(&socket, fs::Permissions::from_mode(0o600))?;
    let db = Database::open(&paths.config_dir.join("launcher.db"))?;
    db.interrupt_preparations()?;
    let state = Arc::new(AppState::new(paths, db));
    crate::runtime::migrate_preferences(&state)?;
    crate::runtime::restore_graphics_diagnostic(&state)?;
    crate::installation::restore(&state)?;
    crate::process::recover(state.clone()).await?;
    let start = Instant::now();
    let last = Arc::new(AtomicU64::new(0));
    let mut idle = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            accept=listener.accept()=>{
                let (stream,_)=accept?;if stream.peer_cred()?.uid()!=unsafe{libc::geteuid()}{continue;}
                last.store(start.elapsed().as_secs(),Ordering::SeqCst);
                let state=state.clone();let last=last.clone();let began=start;
                tokio::spawn(async move{
                    let mut reader=BufReader::new(stream);let mut text=String::new();
                    let input=tokio::time::timeout(Duration::from_secs(10),tokio::io::AsyncReadExt::take(&mut reader,LIMIT).read_line(&mut text)).await;
                    if !matches!(input,Ok(Ok(n)) if n>0)||text.len() as u64>=LIMIT{return;}
                    let result=match serde_json::from_str::<Request>(&text){Ok(req)=>dispatch(&state,&req.command,req.args).await,Err(e)=>Err(e.into())};
                    last.store(began.elapsed().as_secs(),Ordering::SeqCst);
                    let value=match result{Ok(value)=>json!({"result":value}),Err(error)=>json!({"error":error})};
                    if let Ok(mut bytes)=serde_json::to_vec(&value){bytes.push(b'\n');let _=reader.get_mut().write_all(&bytes).await;}
                });
            },
            _=idle.tick()=>{
                if !state.operation_running.load(Ordering::SeqCst)&&!state.game_running.load(Ordering::SeqCst)&&start.elapsed().as_secs().saturating_sub(last.load(Ordering::SeqCst))>30{break;}
            }
        }
    }
    let _ = fs::remove_file(socket);
    Ok(())
}

async fn dispatch(state: &Arc<AppState>, command: &str, args: Value) -> AppResult<Value> {
    match command {
        "get_bootstrap" => {
            let settings = state.settings()?;
            let game = state.paths.game_dir(&settings)?;
            let (servers, news) = tokio::join!(
                crate::official::servers(&state.paths, &game),
                crate::official::news(&state.paths)
            );
            let servers = servers?;
            state
                .db
                .lock()
                .map_err(|_| AppError::new("Блокировка БД"))?
                .cache_servers(&servers)?;
            Ok(
                json!({"settings":settings,"paths":state.paths.info(&settings)?,"portproton":crate::portproton::detect(&settings),"servers":servers,"news":news,"process":state.process(),"environment":state.snapshot(0).environment,"preparation":state.snapshot(0)}),
            )
        }
        "save_settings" => {
            let settings: Settings = serde_json::from_value(args["settings"].clone())?;
            let settings = settings.normalized();
            validate_settings(&settings)?;
            let old = state.settings()?;
            let runtime_change = old.wine_build != settings.wine_build
                || old.graphics_api != settings.graphics_api
                || old.portproton_path != settings.portproton_path;
            let files_change = old.auto_game_path != settings.auto_game_path
                || old.game_path_override != settings.game_path_override
                || old.manifest_url != settings.manifest_url
                || old.download_base_url != settings.download_base_url;
            let _guard = if runtime_change || files_change {
                Some(crate::state::OperationGuard::begin(state)?)
            } else {
                None
            };
            {
                let db = state
                    .db
                    .lock()
                    .map_err(|_| AppError::new("Блокировка БД"))?;
                if runtime_change {
                    db.save_runtime_selection(&settings)?;
                } else {
                    db.save_settings(&settings)?;
                }
            }
            if runtime_change || files_change {
                state.set_ready(None, false);
                let build = crate::manifest::load_local_manifest(&state.paths.game_dir(&settings)?)
                    .ok()
                    .map(|m| m.build);
                *state
                    .updates
                    .lock()
                    .map_err(|_| AppError::new("Блокировка версии"))? =
                    crate::models::UpdateStatus {
                        status: "checking".into(),
                        installed_build: build,
                        latest_build: None,
                        message: "Проверяем выбранную установку…".into(),
                    };
                if let Ok(mut env) = state.environment.lock() {
                    env.launch_ready = false;
                }
            }
            if files_change {
                state.set_files_ready(None, false);
            }
            Ok(serde_json::to_value(settings)?)
        }
        "list_servers" => {
            let settings = state.settings()?;
            Ok(serde_json::to_value(
                crate::official::servers(&state.paths, &state.paths.game_dir(&settings)?).await?,
            )?)
        }
        "get_preparation_status" => Ok(serde_json::to_value(
            state.snapshot(args["since"].as_u64().unwrap_or(0)),
        )?),
        "check_portproton" => Ok(serde_json::to_value(crate::portproton::detect(
            &state.settings()?,
        ))?),
        "check_game_update" => Ok(serde_json::to_value(
            crate::installation::check_updates(state).await?,
        )?),
        "check_runtime_tools" => {
            crate::runtime::validate_bundled_tools(state).await?;
            Ok(json!({"ready":true}))
        }
        "prepare_installation" | "prepare_game" => Ok(serde_json::to_value(
            crate::runtime::prepare_installation(state.clone(), false).await?,
        )?),
        "verify_game" => Ok(serde_json::to_value(
            crate::runtime::prepare_installation(state.clone(), true).await?,
        )?),
        "install_portproton" => crate::portproton::install(
            state.events.clone(),
            state.clone(),
            crate::updater::http_client()?,
        )
        .await
        .map(|s| serde_json::to_value(s).unwrap()),
        "get_game_status" => Ok(serde_json::to_value(state.process())?),
        "launch_game" => {
            let server = args["serverId"]
                .as_str()
                .or_else(|| args["server_id"].as_str())
                .ok_or_else(|| AppError::new("Не выбран сервер."))?;
            let nick = args["nickname"]
                .as_str()
                .ok_or_else(|| AppError::new("Не указан никнейм."))?;
            Ok(serde_json::to_value(
                crate::process::launch_game(
                    state.events.clone(),
                    state.clone(),
                    server.into(),
                    nick.into(),
                )
                .await?,
            )?)
        }
        "list_runtime_versions" => Ok(serde_json::to_value(
            crate::components::versions(state).await?,
        )?),
        "list_graphics_profiles" => Ok(serde_json::to_value(crate::runtime::graphics_profiles())?),
        "get_dependency_plan" => Ok(serde_json::to_value(
            crate::runtime::dependency_plan(state).await?,
        )?),
        "install_system_dependencies" => Ok(serde_json::to_value(
            crate::runtime::install_dependencies(
                state,
                args["planId"]
                    .as_str()
                    .or_else(|| args["plan_id"].as_str())
                    .unwrap_or(""),
            )
            .await?,
        )?),
        "cancel_operation" => {
            state.cancelled.store(true, Ordering::SeqCst);
            Ok(Value::Null)
        }
        _ => Err(AppError::new("Неизвестная команда фонового процесса.")),
    }
}
