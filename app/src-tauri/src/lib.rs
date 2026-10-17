//! Tauri shell: hosts the web UI and forwards its commands to the Rust
//! service. All app logic lives in `cse-service`.

use std::sync::Arc;

use cse_service::AppService;
use serde_json::Value;
use tauri::Manager;

/// The UI's single entry point; see `AppService::dispatch` for commands.
#[tauri::command]
async fn call(
    service: tauri::State<'_, Arc<AppService>>,
    cmd: String,
    args: Value,
) -> Result<Value, String> {
    service.dispatch(&cmd, args).await.map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let service = Arc::new(AppService::open(dir.join("cse-trader.sqlite"))?);
            tauri::async_runtime::spawn(service.clone().run_poller());
            app.manage(service);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![call])
        .run(tauri::generate_context!())
        .expect("error while running CSE Trader");
}
