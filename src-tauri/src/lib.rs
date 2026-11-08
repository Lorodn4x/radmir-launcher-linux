mod commands;
mod components;
mod db;
mod error;
mod events;
mod installation;
mod locks;
mod manifest;
mod models;
mod official;
mod paths;
mod portproton;
mod process;
mod runtime;
mod servers;
mod state;
#[cfg(test)]
mod test_support;
mod token_server;
mod updater;
mod worker;
use tauri::{Emitter, Manager};
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    if std::env::args().any(|a| a == "--worker") {
        if let Err(e) = worker::run() {
            eprintln!("Фоновый процесс: {e}");
            std::process::exit(1);
        }
        return;
    }
    let paths = paths::AppPaths::discover().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    paths.ensure_private_dirs().unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    let client = worker::WorkerClient::new(paths).unwrap_or_else(|e| {
        eprintln!("{e}");
        std::process::exit(1)
    });
    tauri::Builder::default().manage(client)
        .setup(|app|{
            let app=app.handle().clone();let client=app.state::<worker::WorkerClient>().inner().clone();
            tauri::async_runtime::spawn(async move{
                let mut sequence=0u64;
                loop{
                    match client.call("get_preparation_status",serde_json::json!({"since":sequence})).await{
                        Ok(value)=>{
                            sequence = value["event_sequence"].as_u64().unwrap_or(0);
                            if let Some(frames)=value["events"].as_array(){for frame in frames{if let(Some(seq),Some(event))=(frame["sequence"].as_u64(),frame["event"].as_str()){sequence=sequence.max(seq);let _=app.emit(event,frame["payload"].clone());}}}
                            let _=app.emit("worker://status",value);
                        },Err(error)=>{sequence=0;let _=app.emit("worker://error",serde_json::json!({"code":error.code(),"message":error.to_string()}));}
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                }
            });Ok(())
        })
        .invoke_handler(tauri::generate_handler![commands::get_bootstrap,commands::save_settings,commands::list_servers,commands::get_preparation_status,commands::check_portproton,commands::check_game_update,commands::install_portproton,commands::prepare_installation,commands::prepare_game,commands::verify_game,commands::get_game_status,commands::launch_game,commands::list_runtime_versions,commands::list_graphics_profiles,commands::get_dependency_plan,commands::install_system_dependencies,commands::cancel_operation,commands::open_external,commands::open_logs,commands::minimize_window,commands::toggle_maximize,commands::close_window])
        .run(tauri::generate_context!()).expect("Не удалось запустить окно лаунчера");
}
