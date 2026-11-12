pub mod brightness;
pub mod camera;
pub mod engine;
pub mod power;
pub mod settings;
pub mod state;

use engine::EngineHandle;
use settings as settings_store;
use state::{EngineState, Settings, WorkerCommand};
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Manager, WindowEvent};

pub struct AppRuntime {
    pub engine: EngineHandle,
    pub shared: Arc<Mutex<EngineState>>,
}

#[tauri::command]
fn get_state(rt: tauri::State<'_, AppRuntime>) -> EngineState {
    rt.shared.lock().map(|s| s.clone()).unwrap_or_default()
}

#[tauri::command]
fn get_settings(rt: tauri::State<'_, AppRuntime>) -> Settings {
    rt.shared
        .lock()
        .map(|s| s.settings.clone())
        .unwrap_or_else(|_| settings_store::load_settings())
}

#[tauri::command]
fn set_paused(paused: bool, rt: tauri::State<'_, AppRuntime>) -> Result<(), String> {
    rt.engine
        .tx
        .send(WorkerCommand::SetPaused(paused))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn calibrate(rt: tauri::State<'_, AppRuntime>) -> Result<(), String> {
    rt.engine
        .tx
        .send(WorkerCommand::Calibrate)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn refresh_displays(rt: tauri::State<'_, AppRuntime>) -> Result<(), String> {
    rt.engine
        .tx
        .send(WorkerCommand::RefreshDisplays)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn apply_now(rt: tauri::State<'_, AppRuntime>) -> Result<(), String> {
    rt.engine
        .tx
        .send(WorkerCommand::ApplyNow)
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn update_settings(mut settings: Settings, app: tauri::AppHandle, rt: tauri::State<'_, AppRuntime>) -> Result<(), String> {
    settings_store::sanitize_settings(&mut settings);
    // Handle autostart side effect
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        let autolaunch = app.autolaunch();
        if settings.autostart {
            let _ = autolaunch.enable();
        } else {
            let _ = autolaunch.disable();
        }
    }
    settings_store::enable_persist();
    rt.engine
        .tx
        .send(WorkerCommand::SetSettings(settings))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn set_autostart(enabled: bool, app: tauri::AppHandle, rt: tauri::State<'_, AppRuntime>) -> Result<(), String> {
    #[cfg(desktop)]
    {
        use tauri_plugin_autostart::ManagerExt;
        let autolaunch = app.autolaunch();
        if enabled {
            autolaunch.enable().map_err(|e| e.to_string())?;
        } else {
            autolaunch.disable().map_err(|e| e.to_string())?;
        }
    }
    let mut settings = rt.shared.lock().map(|s| s.settings.clone()).unwrap_or_default();
    settings.autostart = enabled;
    settings_store::sanitize_settings(&mut settings);
    settings_store::enable_persist();
    rt.engine
        .tx
        .send(WorkerCommand::SetSettings(settings))
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn quit_app(app: tauri::AppHandle, rt: tauri::State<'_, AppRuntime>) {
    let _ = rt.engine.tx.send(WorkerCommand::Quit);
    app.exit(0);
}

fn setup_tray(app: &tauri::AppHandle) -> tauri::Result<()> {
    let show_i = MenuItem::with_id(app, "show", "Show", true, None::<&str>)?;
    let pause_i = MenuItem::with_id(app, "pause", "Pause/Resume", true, None::<&str>)?;
    let quit_i = MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show_i, &pause_i, &quit_i])?;

    let _tray = TrayIconBuilder::with_id("main")
        .menu(&menu)
        .tooltip("AutoBrightness")
        .on_menu_event(|app, event| match event.id().as_ref() {
            "show" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
            }
            "pause" => {
                let rt = app.state::<AppRuntime>();
                let paused = rt.shared.lock().map(|s| s.paused).unwrap_or(false);
                let _ = rt.engine.tx.send(WorkerCommand::SetPaused(!paused));
            }
            "quit" => {
                let rt = app.state::<AppRuntime>();
                let _ = rt.engine.tx.send(WorkerCommand::Quit);
                app.exit(0);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::DoubleClick {
                button: MouseButton::Left,
                ..
            } = event
            {
                let app = tray.app_handle();
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.show();
                    let _ = w.unminimize();
                    let _ = w.set_focus();
                }
            }
        })
        .icon(app.default_window_icon().unwrap().clone())
        .build(app)?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let (initial_settings, _persist_allowed) = settings_store::load_settings_with_persist();
    let shared = Arc::new(Mutex::new(engine::initial_state(&initial_settings)));

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.unminimize();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--minimized"]),
        ))
        .setup(move |app| {
            let handle = app.handle().clone();
            let shared_state = shared.clone();
            let settings_for_engine = initial_settings.clone();
            // Ensure shared state reflects loaded/sanitized settings at startup
            if let Ok(mut st) = shared_state.lock() {
                st.settings = settings_for_engine.clone();
                st.paused = settings_for_engine.paused;
            }
            let engine_handle = engine::spawn_engine(handle.clone(), settings_for_engine, shared_state.clone());
            app.manage(AppRuntime {
                engine: engine_handle,
                shared: shared_state.clone(),
            });

            if let Err(e) = setup_tray(&handle) {
                eprintln!("Tray setup failed: {e}");
            }

            // Hide on start if --minimized
            let args: Vec<String> = std::env::args().collect();
            if args.iter().any(|a| a == "--minimized") {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.hide();
                }
            }

            // Tooltip updater
            let app_for_tooltip = handle.clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_secs(2));
                let (paused, target) = {
                    if let Some(rt) = app_for_tooltip.try_state::<AppRuntime>() {
                        if let Ok(st) = rt.shared.lock() {
                            (st.paused, st.target_brightness)
                        } else {
                            (false, None)
                        }
                    } else {
                        (false, None)
                    }
                };
                if let Some(tray) = app_for_tooltip.tray_by_id("main") {
                    let tip = match target {
                        Some(t) => format!("AutoBrightness · {}%{}", t, if paused { " (paused)" } else { "" }),
                        None => format!("AutoBrightness{}", if paused { " (paused)" } else { "" }),
                    };
                    let _ = tray.set_tooltip(Some(tip));
                }
            });

            Ok(())
        })
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_state,
            get_settings,
            set_paused,
            calibrate,
            refresh_displays,
            apply_now,
            update_settings,
            set_autostart,
            quit_app
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

pub fn main_stub() {
    run();
}
