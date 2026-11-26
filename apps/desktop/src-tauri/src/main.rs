#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use codex_mobile_bridge::{
    config::ServiceConfig, host::Host, paths::Paths, updates, BridgeError, Result,
};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tauri::{Manager, State};
use tauri_plugin_autostart::ManagerExt as AutostartExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::Mutex;

struct AppState {
    host: Arc<Host>,
    binaries: PathBuf,
    update_operation: Mutex<()>,
}
#[tauri::command]
async fn status(state: State<'_, AppState>) -> Result<Value> {
    state.host.status().await
}
#[tauri::command]
fn get_autostart(app: tauri::AppHandle) -> Result<bool> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| BridgeError::unavailable(e.to_string()))
}
#[tauri::command]
fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<bool> {
    let manager = app.autolaunch();
    if enabled {
        manager.enable()
    } else {
        manager.disable()
    }
    .map_err(|e| BridgeError::unavailable(e.to_string()))?;
    manager
        .is_enabled()
        .map_err(|e| BridgeError::unavailable(e.to_string()))
}
#[tauri::command]
async fn start_service(state: State<'_, AppState>) -> Result<Value> {
    state.host.start().await?;
    state.host.connect_desktop(state.binaries.clone()).await;
    state.host.status().await
}
#[tauri::command]
async fn stop_service(state: State<'_, AppState>) -> Result<Value> {
    state.host.stop().await?;
    state.host.status().await
}
#[tauri::command]
async fn save_config(state: State<'_, AppState>, config: ServiceConfig) -> Result<Value> {
    state.host.save_config(config).await?;
    state.host.status().await
}
#[tauri::command]
async fn pairing(state: State<'_, AppState>) -> Result<Value> {
    state.host.pairing().await
}
#[tauri::command]
async fn rotate_key(state: State<'_, AppState>) -> Result<Value> {
    state.host.rotate_key().await
}
#[tauri::command]
async fn configure_relay(
    state: State<'_, AppState>,
    url: &str,
    name: &str,
    token: &str,
) -> Result<Value> {
    state.host.configure_relay(url, name, token).await?;
    state.host.status().await
}
#[tauri::command]
async fn relay_approval(state: State<'_, AppState>, login_key: Option<&str>) -> Result<Value> {
    state.host.relay_approval(login_key).await
}
#[tauri::command]
async fn link_relay_desktop(state: State<'_, AppState>) -> Result<Value> {
    state.host.link_relay_desktop().await
}
#[tauri::command]
async fn decide_relay_approval(
    state: State<'_, AppState>,
    request: &str,
    approve: bool,
) -> Result<Value> {
    state.host.decide_relay_approval(request, approve).await?;
    state.host.status().await
}
#[tauri::command]
async fn revoke_relay_phone(state: State<'_, AppState>, phone: &str) -> Result<Value> {
    state.host.revoke_relay_phone(phone).await?;
    state.host.status().await
}
#[tauri::command]
async fn retry_sync(state: State<'_, AppState>) -> Result<Value> {
    state.host.connect_desktop(state.binaries.clone()).await;
    state.host.status().await
}
#[tauri::command]
async fn diagnostics(state: State<'_, AppState>) -> Result<String> {
    state.host.diagnostics().await
}
#[tauri::command]
async fn open_codex(state: State<'_, AppState>) -> Result<Value> {
    state.host.connect_desktop(state.binaries.clone()).await;
    state.host.status().await
}
#[tauri::command]
fn open_external(app: tauri::AppHandle, destination: &str) -> Result<()> {
    let url = match destination {
        "repository" => "https://github.com/J-Smile/codex-mobile",
        "releases" => "https://github.com/J-Smile/codex-mobile/releases",
        _ => return Err(BridgeError::invalid("链接不存在。")),
    };
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| BridgeError::unavailable(e.to_string()))
}
#[tauri::command]
async fn check_update(state: State<'_, AppState>) -> Result<Value> {
    let _operation = state.update_operation.lock().await;
    state.host.prepare_update().await?;
    state.host.status().await
}

async fn apply(app: &tauri::AppHandle, background: bool) -> Result<()> {
    let state = app.state::<AppState>();
    if state.host.update_state.read().await["phase"] == "applying" {
        return Ok(());
    }
    let candidate = state
        .host
        .candidate
        .lock()
        .await
        .clone()
        .ok_or_else(|| BridgeError::invalid("请先检查并下载更新。"))?;
    let resume_service = state.host.app().await.is_some();
    *state.host.update_state.write().await =
        json!({"phase":"applying","message":"正在完成当前请求并切换版本…"});
    let result = state.host.stop().await.and_then(|()| {
        updates::spawn_handoff(
            &state.host.paths.data_home,
            &state.binaries,
            candidate,
            updates::LaunchOptions {
                background,
                resume_service,
            },
        )
    });
    if let Err(error) = result {
        if resume_service {
            let _ = state.host.start().await;
        }
        *state.host.update_state.write().await = json!({"phase":"error","message":error.message});
        return Err(error);
    }
    app.exit(0);
    Ok(())
}
#[tauri::command]
async fn apply_update(app: tauri::AppHandle) -> Result<()> {
    let state = app.state::<AppState>();
    let _operation = state.update_operation.lock().await;
    apply(&app, false).await
}

fn show(app: &tauri::AppHandle) {
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.unminimize();
        let _ = window.set_focus();
    }
}
fn open_mobile(app: &tauri::AppHandle) {
    show(app);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(state) = app.try_state::<AppState>() {
            if state.host.auto_start().await {
                state.host.connect_desktop(state.binaries.clone()).await;
            }
        }
    });
}
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if !cfg!(debug_assertions) {
        let redirect = Paths::from_env(PathBuf::new()).and_then(|paths| {
            tauri::async_runtime::block_on(updates::redirect_installed(&paths.data_home, &args))
        });
        if matches!(redirect, Ok(true)) {
            return;
        }
    }
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, args, _cwd| {
            if !args.iter().any(|arg| arg == "--autostart") {
                if args.iter().any(|arg| arg == "--update-handoff") {
                    show(app);
                } else {
                    open_mobile(app);
                }
            }
            let app = app.clone();
            tauri::async_runtime::spawn(async move {
                if let Some(state) = app.try_state::<AppState>() {
                    let ready = if args.iter().any(|arg| arg == "--resume-service") {
                        state.host.start().await.is_ok()
                    } else {
                        !state.host.auto_start().await || state.host.app().await.is_some()
                    };
                    if ready {
                        let _ = updates::acknowledge(&args);
                    }
                }
            });
        }))
        .plugin(
            tauri_plugin_autostart::Builder::new()
                .app_name("Codex Mobile")
                .arg("--autostart")
                .build(),
        )
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            status,
            get_autostart,
            set_autostart,
            start_service,
            stop_service,
            save_config,
            pairing,
            rotate_key,
            configure_relay,
            relay_approval,
            link_relay_desktop,
            decide_relay_approval,
            revoke_relay_phone,
            retry_sync,
            diagnostics,
            open_codex,
            open_external,
            check_update,
            apply_update
        ])
        .setup(|app| {
            // Keep an enabled login entry pointing at the current installed version.
            if !cfg!(debug_assertions) && app.autolaunch().is_enabled().unwrap_or(false) {
                let _ = app.autolaunch().enable();
            }
            let resources = app.path().resource_dir()?;
            let mut paths = Paths::from_env(PathBuf::new())?;
            #[cfg(windows)]
            let resources = if cfg!(debug_assertions) {
                resources
            } else {
                updates::windows_resource_root(&paths.data_home, &resources)?
            };
            let web = if cfg!(debug_assertions) {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../dist/mobile")
            } else {
                resources.join("mobile")
            };
            let binaries = if cfg!(debug_assertions) {
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("binaries")
            } else {
                resources.join("bin")
            };
            paths.web_root = web;
            let host = Host::open(paths, !cfg!(debug_assertions))?;
            app.manage(AppState {
                host: host.clone(),
                binaries,
                update_operation: Mutex::new(()),
            });
            let show_item = tauri::menu::MenuItem::with_id(
                app,
                "show",
                "打开 Codex Mobile",
                true,
                None::<&str>,
            )?;
            let quit_item =
                tauri::menu::MenuItem::with_id(app, "quit", "退出连接服务", true, None::<&str>)?;
            let menu = tauri::menu::Menu::with_items(app, &[&show_item, &quit_item])?;
            tauri::tray::TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .tooltip("Codex Mobile")
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => open_mobile(app),
                    "quit" => {
                        let app = app.clone();
                        tauri::async_runtime::spawn(async move {
                            let _ = app.state::<AppState>().host.stop().await;
                            app.exit(0);
                        });
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if matches!(
                        event,
                        tauri::tray::TrayIconEvent::Click {
                            button: tauri::tray::MouseButton::Left,
                            button_state: tauri::tray::MouseButtonState::Up,
                            ..
                        }
                    ) {
                        open_mobile(tray.app_handle());
                    }
                })
                .build(app)?;
            if !std::env::args().any(|arg| arg == "--autostart") {
                if std::env::args().any(|arg| arg == "--update-handoff") {
                    show(app.handle());
                } else {
                    open_mobile(app.handle());
                }
            }
            let handle = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                let args: Vec<String> = std::env::args().collect();
                let ready = if host.auto_start().await
                    || args.iter().any(|arg| arg == "--resume-service")
                {
                    host.start().await.is_ok()
                } else {
                    true
                };
                if ready {
                    let _ = updates::acknowledge(&args);
                }
                let mut requests = host.mobile_update_requests();
                loop {
                    tokio::select! {
                        biased;
                        _ = requests.wait_for(|pending| *pending) => {}
                        _ = tokio::time::sleep(Duration::from_secs(15 * 60)) => {}
                    }
                    let state = handle.state::<AppState>();
                    let _operation = state.update_operation.lock().await;
                    if *requests.borrow() || host.updates.status()["mode"] == "automatic" {
                        if host.prepare_update().await.unwrap_or(false) {
                            let _ = apply(&handle, true).await;
                        }
                        host.finish_mobile_update();
                    }
                }
            });
            Ok(())
        })
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                let _ = window.hide();
            }
        })
        .run(tauri::generate_context!())
        .expect("无法启动 Codex Mobile 桌面程序");
}
