//! Tauri shell: hosts the web UI and forwards its commands to the Rust
//! service. All app logic lives in `cse-service`.

use std::path::Path;
use std::sync::{Arc, Mutex};

use cse_service::AppService;
use serde_json::Value;
use tauri::Manager;

/// One service per process, shared by the UI and Android's scheduled
/// background checks (which may start the process without the UI).
static SERVICE: Mutex<Option<Arc<AppService>>> = Mutex::new(None);

fn service_at(db: &Path) -> Result<Arc<AppService>, String> {
    let mut slot = SERVICE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(s) = slot.as_ref() {
        return Ok(s.clone());
    }
    let s = Arc::new(AppService::open(db).map_err(|e| e.to_string())?);
    *slot = Some(s.clone());
    Ok(s)
}

/// The UI's single entry point; see `AppService::dispatch` for commands.
#[tauri::command]
async fn call(
    service: tauri::State<'_, Arc<AppService>>,
    cmd: String,
    args: Value,
) -> Result<Value, String> {
    service
        .dispatch(&cmd, args)
        .await
        .map_err(|e| e.to_string())
}

/// Saves Rust panics where Android's CrashCatcher shows them on the next start.
#[cfg(target_os = "android")]
fn record_panics() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let default = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = std::fs::write(
                "/data/data/lk.personal.csetrader/files/rust-panic.txt",
                format!(
                    "Rust panic: {info}\n{}",
                    std::backtrace::Backtrace::force_capture()
                ),
            );
            default(info);
        }));
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    #[cfg(target_os = "android")]
    record_panics();
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let service = service_at(&dir.join("cse-trader.sqlite"))?;
            tauri::async_runtime::spawn(service.clone().run_poller());
            app.manage(service);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![call])
        .run(tauri::generate_context!())
        .expect("error while running CSE Trader");
}

/// Android scheduled check (`NativeCheck.runCheck` in CheckWorker.kt):
/// refreshes prices, updates every favourite and writes new alerts to the
/// outbox. Returns the number of new alerts, or -1 on failure.
#[cfg(target_os = "android")]
#[no_mangle]
pub extern "system" fn Java_lk_personal_csetrader_NativeCheck_runCheck(
    mut env: jni::JNIEnv,
    _class: jni::objects::JClass,
    db: jni::objects::JString,
) -> jni::sys::jint {
    record_panics();
    let Ok(db) = env.get_string(&db).map(String::from) else {
        return -1;
    };
    std::panic::catch_unwind(|| {
        let service = service_at(Path::new(&db)).ok()?;
        tauri::async_runtime::block_on(service.run_check()).ok()
    })
    .ok()
    .flatten()
    .map_or(-1, |n| n as jni::sys::jint)
}
