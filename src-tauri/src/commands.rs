use crate::{
    db::validate_https_url,
    error::{AppError, AppResult},
    worker::WorkerClient,
};
use serde_json::{json, Value};
use tauri::State;
#[tauri::command]
pub async fn get_bootstrap(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("get_bootstrap", json!({})).await
}
#[tauri::command]
pub async fn save_settings(
    state: State<'_, WorkerClient>,
    settings: crate::models::Settings,
) -> AppResult<Value> {
    state
        .call("save_settings", json!({"settings":settings}))
        .await
}
#[tauri::command]
pub async fn list_servers(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("list_servers", json!({})).await
}
#[tauri::command]
pub async fn get_preparation_status(
    state: State<'_, WorkerClient>,
    since: Option<u64>,
) -> AppResult<Value> {
    state
        .call(
            "get_preparation_status",
            json!({"since":since.unwrap_or(0)}),
        )
        .await
}
#[tauri::command]
pub async fn check_portproton(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("check_portproton", json!({})).await
}
#[tauri::command]
pub async fn check_game_update(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("check_game_update", json!({})).await
}
#[tauri::command]
pub async fn install_portproton(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("install_portproton", json!({})).await
}
#[tauri::command]
pub async fn prepare_installation(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("prepare_installation", json!({})).await
}
#[tauri::command]
pub async fn prepare_game(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("prepare_game", json!({})).await
}
#[tauri::command]
pub async fn verify_game(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("verify_game", json!({})).await
}
#[tauri::command]
pub async fn get_game_status(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("get_game_status", json!({})).await
}
#[tauri::command]
pub async fn launch_game(
    state: State<'_, WorkerClient>,
    server_id: String,
    nickname: String,
) -> AppResult<Value> {
    state
        .call(
            "launch_game",
            json!({"serverId":server_id,"nickname":nickname}),
        )
        .await
}
#[tauri::command]
pub async fn list_runtime_versions(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("list_runtime_versions", json!({})).await
}
#[tauri::command]
pub async fn list_graphics_profiles(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("list_graphics_profiles", json!({})).await
}
#[tauri::command]
pub async fn get_dependency_plan(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("get_dependency_plan", json!({})).await
}
#[tauri::command]
pub async fn install_system_dependencies(
    state: State<'_, WorkerClient>,
    plan_id: String,
) -> AppResult<Value> {
    state
        .call("install_system_dependencies", json!({"planId":plan_id}))
        .await
}
#[tauri::command]
pub async fn cancel_operation(state: State<'_, WorkerClient>) -> AppResult<Value> {
    state.call("cancel_operation", json!({})).await
}
#[tauri::command]
pub fn open_external(url: String) -> AppResult<()> {
    validate_https_url(&url)?;
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map_err(AppError::from)?;
    Ok(())
}
#[tauri::command]
pub fn open_logs(state: State<'_, WorkerClient>) -> AppResult<()> {
    std::process::Command::new("xdg-open")
        .arg(&state.paths.state_dir)
        .spawn()?;
    Ok(())
}
#[tauri::command]
pub fn minimize_window(window: tauri::WebviewWindow) -> AppResult<()> {
    window.minimize().map_err(|e| AppError::new(e.to_string()))
}
#[tauri::command]
pub fn toggle_maximize(window: tauri::WebviewWindow) -> AppResult<()> {
    if window
        .is_maximized()
        .map_err(|e| AppError::new(e.to_string()))?
    {
        window.unmaximize()
    } else {
        window.maximize()
    }
    .map_err(|e| AppError::new(e.to_string()))
}
#[tauri::command]
pub fn close_window(window: tauri::WebviewWindow) -> AppResult<()> {
    window.close().map_err(|e| AppError::new(e.to_string()))
}
