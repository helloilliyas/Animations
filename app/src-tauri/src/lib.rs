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
    let builder = tauri::Builder::default();
    // A second launch shows the running window instead of starting another
    // poller on the same database.
    #[cfg(desktop)]
    let builder = builder
        .plugin(tauri_plugin_single_instance::init(|app, _, _| {
            desktop::show(app);
        }))
        .plugin(tauri_plugin_notification::init());
    builder
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&dir)?;
            let service = service_at(&dir.join("cse-trader.sqlite"))?;
            tauri::async_runtime::spawn(service.clone().run_poller());
            #[cfg(desktop)]
            desktop::setup(app, service.clone())?;
            app.manage(service);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![call])
        .run(tauri::generate_context!())
        .expect("error while running CSE Trader");
}

/// Desktop extras: a tray icon that keeps the app checking when the window
/// is closed, and system notifications for alerts written to the outbox
/// (on Android the Kotlin side does both).
#[cfg(desktop)]
mod desktop {
    use std::sync::Arc;
    use std::time::Duration;

    use cse_service::AppService;
    use serde_json::{json, Value};
    use tauri::menu::{Menu, MenuItem};
    use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
    use tauri::{App, AppHandle, Manager, WindowEvent};
    use tauri_plugin_notification::NotificationExt;

    pub fn show(app: &AppHandle) {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.unminimize();
            let _ = w.show();
            let _ = w.set_focus();
        }
    }

    pub fn setup(app: &mut App, service: Arc<AppService>) -> tauri::Result<()> {
        let open = MenuItem::with_id(app, "open", "Open CSE Trader", true, None::<&str>)?;
        let quit = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
        let menu = Menu::with_items(app, &[&open, &quit])?;
        let mut tray = TrayIconBuilder::with_id("main")
            .tooltip("CSE Trader")
            .menu(&menu)
            .show_menu_on_left_click(false)
            .on_menu_event(|app, e| match e.id().as_ref() {
                "open" => show(app),
                "quit" => app.exit(0),
                _ => {}
            })
            .on_tray_icon_event(|tray, e| {
                if let TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                } = e
                {
                    show(tray.app_handle());
                }
            });
        if let Some(icon) = app.default_window_icon() {
            tray = tray.icon(icon.clone());
        }
        // Closing the window hides it; checks and alerts carry on until
        // Quit in the tray menu. Without a tray (some Linux desktops) closing
        // quits as usual, so the window is never lost.
        // (On Linux a missing tray library panics rather than erroring.)
        let built = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| tray.build(app)));
        let has_tray = matches!(built, Ok(Ok(_)));
        if let Some(w) = app.get_webview_window("main").filter(|_| has_tray) {
            let hide = w.clone();
            w.on_window_event(move |e| {
                if let WindowEvent::CloseRequested { api, .. } = e {
                    api.prevent_close();
                    let _ = hide.hide();
                }
            });
        }

        let handle = app.handle().clone();
        tauri::async_runtime::spawn(async move {
            let path = service
                .dispatch("outbox_path", json!({}))
                .await
                .ok()
                .and_then(|v| v.as_str().map(std::path::PathBuf::from));
            let Some(path) = path else { return };
            loop {
                for n in drain(&path) {
                    let text = |k: &str| n.get(k).and_then(Value::as_str).unwrap_or("").to_string();
                    let _ = handle
                        .notification()
                        .builder()
                        .title(text("title"))
                        .body(text("body"))
                        .show();
                }
                tokio::time::sleep(Duration::from_secs(5)).await;
            }
        });
        Ok(())
    }

    /// Takes every waiting notification. The file is renamed first so an
    /// alert written meanwhile goes to a fresh outbox, not lost.
    fn drain(path: &std::path::Path) -> Vec<Value> {
        let taken = path.with_extension("draining");
        if std::fs::rename(path, &taken).is_err() {
            return Vec::new();
        }
        let text = std::fs::read_to_string(&taken).unwrap_or_default();
        let _ = std::fs::remove_file(&taken);
        text.lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }
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
