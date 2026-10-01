use crate::core::service::app_project_dirs;
use crate::core::types::{
    DataSourceKind, ModelPriceEntry, SyncResult, UsageRange, UsageSnapshot, WeeklyUsage,
};
use crate::core::UsageService;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, WebviewUrl, WebviewWindowBuilder, WindowEvent};

const USAGE_UPDATED_EVENT: &str = "usage-updated";
const DASHBOARD_LABEL: &str = "main";
const FLOATING_LABEL: &str = "floating";

struct AppState {
    service: Arc<UsageService>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct FloatingPosition {
    x: i32,
    y: i32,
}

pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            Some(vec!["--autostart"]),
        ))
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let service = Arc::new(UsageService::open_default().map_err(std::io::Error::other)?);
            app.manage(AppState {
                service: Arc::clone(&service),
            });
            let source_kind = service.primary_source_kind();
            create_dashboard_window(app.handle(), source_kind.window_title())?;
            create_floating_window(app.handle(), source_kind.window_title())?;
            create_tray(app, source_kind.label())?;

            let app_handle = app.handle().clone();
            let initial_service = Arc::clone(&service);
            std::thread::spawn(move || {
                let usage_ready = initial_service.sync().is_ok();
                let prices_ready = initial_service.refresh_models_dev_prices().is_ok();
                if usage_ready || prices_ready {
                    let _ = app_handle.emit(USAGE_UPDATED_EVENT, ());
                }
            });

            let app_handle = app.handle().clone();
            service
                .start_background_sync(move || {
                    let _ = app_handle.emit(USAGE_UPDATED_EVENT, ());
                })
                .map_err(std::io::Error::other)?;
            Ok(())
        })
        .on_window_event(|window, event| match event {
            WindowEvent::CloseRequested { api, .. } => {
                api.prevent_close();
                let _ = window.hide();
            }
            WindowEvent::Moved(position) if window.label() == FLOATING_LABEL => {
                let _ = save_position(FloatingPosition {
                    x: position.x,
                    y: position.y,
                });
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            get_usage_snapshot,
            get_account_weekly_usage,
            sync_usage,
            reset_usage_cache,
            show_dashboard_window,
            show_floating_window,
            hide_floating_window,
            get_source_homes,
            add_source_home,
            remove_source_home,
            get_used_model_prices,
            refresh_models_dev_prices,
            update_model_price,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run QuotaLoom");
}

#[tauri::command]
async fn get_usage_snapshot(
    state: tauri::State<'_, AppState>,
    range: UsageRange,
    source: Option<DataSourceKind>,
) -> Result<UsageSnapshot, String> {
    let service = Arc::clone(&state.service);
    let filter = source.unwrap_or(DataSourceKind::All);
    tauri::async_runtime::spawn_blocking(move || service.snapshot(range, filter))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
async fn get_account_weekly_usage(
    state: tauri::State<'_, AppState>,
) -> Result<Option<WeeklyUsage>, String> {
    let service = Arc::clone(&state.service);
    tauri::async_runtime::spawn_blocking(move || service.account_weekly_usage())
        .await
        .map_err(|error| error.to_string())
}

#[tauri::command]
async fn sync_usage(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<SyncResult, String> {
    let service = Arc::clone(&state.service);
    let result = tauri::async_runtime::spawn_blocking(move || service.sync())
        .await
        .map_err(|error| error.to_string())??;
    if result.imported_events > 0 || result.rebuilt_files > 0 {
        let _ = app.emit(USAGE_UPDATED_EVENT, ());
    }
    Ok(result)
}

#[tauri::command]
async fn reset_usage_cache(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<SyncResult, String> {
    let service = Arc::clone(&state.service);
    let result = tauri::async_runtime::spawn_blocking(move || service.reset_usage_cache())
        .await
        .map_err(|error| error.to_string())??;
    let _ = app.emit(USAGE_UPDATED_EVENT, ());
    Ok(result)
}

#[tauri::command]
fn show_dashboard_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(DASHBOARD_LABEL)
        .ok_or_else(|| "统计窗口不可用".to_string())?;
    window.show().map_err(|error| error.to_string())?;
    window.set_focus().map_err(|error| error.to_string())
}

#[tauri::command]
fn show_floating_window(app: tauri::AppHandle) -> Result<(), String> {
    let window = app
        .get_webview_window(FLOATING_LABEL)
        .ok_or_else(|| "悬浮窗口不可用".to_string())?;
    window
        .set_always_on_top(true)
        .map_err(|error| error.to_string())?;
    window.show().map_err(|error| error.to_string())
}

#[tauri::command]
fn hide_floating_window(app: tauri::AppHandle) -> Result<(), String> {
    app.get_webview_window(FLOATING_LABEL)
        .ok_or_else(|| "悬浮窗口不可用".to_string())?
        .hide()
        .map_err(|error| error.to_string())
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceHomeInfo {
    path: String,
    kind: DataSourceKind,
    label: String,
}

#[tauri::command]
fn get_source_homes(state: tauri::State<'_, AppState>) -> Vec<SourceHomeInfo> {
    state
        .service
        .source_homes()
        .iter()
        .map(|home| {
            let kind = detect_home_kind(home);
            SourceHomeInfo {
                path: home.to_string_lossy().to_string(),
                kind,
                label: kind.label().to_string(),
            }
        })
        .collect()
}

fn detect_home_kind(home: &std::path::Path) -> DataSourceKind {
    crate::core::service::detect_data_source(home)
}

#[tauri::command]
fn add_source_home(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<SourceHomeInfo, String> {
    let selected_path = PathBuf::from(path);
    if !selected_path.is_dir() {
        return Err(format!("所选目录不存在: {}", selected_path.display()));
    }
    let display_path = selected_path.to_string_lossy().to_string();
    let kind = detect_home_kind(&selected_path);
    let service = Arc::clone(&state.service);
    let app_handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || match service.add_source_home(selected_path) {
        Ok(_) => {
            set_window_titles(&app_handle, service.primary_source_kind());
            let _ = app_handle.emit(USAGE_UPDATED_EVENT, ());
        }
        Err(error) => {
            let _ = app_handle.emit("usage-sync-error", error);
        }
    });
    Ok(SourceHomeInfo {
        path: display_path,
        kind,
        label: kind.label().to_string(),
    })
}

#[tauri::command]
fn remove_source_home(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    path: String,
) -> Result<(), String> {
    let service = Arc::clone(&state.service);
    let app_handle = app.clone();
    let removed = PathBuf::from(&path);
    tauri::async_runtime::spawn_blocking(move || match service.remove_source_home(&removed) {
        Ok(_) => {
            set_window_titles(&app_handle, service.primary_source_kind());
            let _ = app_handle.emit(USAGE_UPDATED_EVENT, ());
        }
        Err(error) => {
            let _ = app_handle.emit("usage-sync-error", error);
        }
    });
    Ok(())
}

#[tauri::command]
fn get_used_model_prices(
    state: tauri::State<'_, AppState>,
) -> Result<Vec<ModelPriceEntry>, String> {
    state.service.used_model_prices()
}

#[tauri::command]
fn update_model_price(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
    model: String,
    input_per_million: String,
    cached_input_per_million: String,
    output_per_million: String,
    multiplier: String,
) -> Result<(), String> {
    state.service.update_model_price(
        &model,
        &input_per_million,
        &cached_input_per_million,
        &output_per_million,
        &multiplier,
    )?;
    let _ = app.emit(USAGE_UPDATED_EVENT, ());
    Ok(())
}

#[tauri::command]
async fn refresh_models_dev_prices(
    app: tauri::AppHandle,
    state: tauri::State<'_, AppState>,
) -> Result<u64, String> {
    let service = Arc::clone(&state.service);
    let imported =
        tauri::async_runtime::spawn_blocking(move || service.refresh_models_dev_prices())
            .await
            .map_err(|error| error.to_string())??;
    let _ = app.emit(USAGE_UPDATED_EVENT, ());
    Ok(imported)
}

fn create_dashboard_window(app: &tauri::AppHandle, title: &str) -> tauri::Result<()> {
    WebviewWindowBuilder::new(
        app,
        DASHBOARD_LABEL,
        WebviewUrl::App("/index.html?surface=dashboard".into()),
    )
    .title(title)
    .inner_size(1160.0, 780.0)
    .min_inner_size(920.0, 640.0)
    .center()
    .visible(false)
    .build()?;
    Ok(())
}

fn create_floating_window(app: &tauri::AppHandle, title: &str) -> tauri::Result<()> {
    let mut builder = WebviewWindowBuilder::new(
        app,
        FLOATING_LABEL,
        WebviewUrl::App("/index.html?surface=floating".into()),
    )
    .title(title)
    .inner_size(420.0, 150.0)
    .resizable(false)
    .decorations(false)
    .transparent(true)
    .shadow(false)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false);

    #[cfg(not(target_os = "windows"))]
    {
        builder = builder.visible_on_all_workspaces(true);
    }
    if let Some(position) = load_position().filter(|position| position_is_visible(app, *position)) {
        builder = builder.position(position.x as f64, position.y as f64);
    } else {
        builder = builder.position(48.0, 72.0);
    }
    builder.build()?;
    Ok(())
}

fn create_tray(app: &tauri::App, label: &str) -> tauri::Result<()> {
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&quit])?;
    TrayIconBuilder::with_id("quota-loom")
        .icon(tray_icon())
        .tooltip(format!("QuotaLoom · {label}"))
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::Click {
                    button: MouseButton::Left,
                    button_state: MouseButtonState::Up,
                    ..
                }
            ) {
                let _ = show_floating_window(tray.app_handle().clone());
            }
        })
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "quit" {
                app.exit(0);
            }
        })
        .build(app)?;
    Ok(())
}

fn set_window_titles(app: &tauri::AppHandle, source_kind: DataSourceKind) {
    let title = source_kind.window_title();
    if let Some(window) = app.get_webview_window(DASHBOARD_LABEL) {
        let _ = window.set_title(title);
    }
    if let Some(window) = app.get_webview_window(FLOATING_LABEL) {
        let _ = window.set_title(title);
    }
}

fn tray_icon() -> Image<'static> {
    const SIZE: u32 = 32;
    let mut rgba = vec![0_u8; (SIZE * SIZE * 4) as usize];
    for y in 2..30 {
        for x in 2..30 {
            let index = ((y * SIZE + x) * 4) as usize;
            let border = !(5..27).contains(&x) || !(5..27).contains(&y);
            let color = if border {
                [17, 17, 17, 255]
            } else if x < 16 {
                [142, 12, 36, 255]
            } else {
                [22, 51, 95, 255]
            };
            rgba[index..index + 4].copy_from_slice(&color);
        }
    }
    Image::new_owned(rgba, SIZE, SIZE)
}

fn position_path() -> Option<PathBuf> {
    app_project_dirs()
        .ok()
        .map(|dirs| dirs.config_local_dir().join("floating-position.json"))
}

fn load_position() -> Option<FloatingPosition> {
    let data = std::fs::read(position_path()?).ok()?;
    serde_json::from_slice(&data).ok()
}

fn save_position(position: FloatingPosition) -> Result<(), String> {
    let path = position_path().ok_or_else(|| "无法确定设置目录".to_string())?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    std::fs::write(
        path,
        serde_json::to_vec(&position).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())
}

fn position_is_visible(app: &tauri::AppHandle, position: FloatingPosition) -> bool {
    app.available_monitors().is_ok_and(|monitors| {
        monitors.iter().any(|monitor| {
            let area = monitor.work_area();
            let left = area.position.x;
            let top = area.position.y;
            let right = left.saturating_add(area.size.width as i32);
            let bottom = top.saturating_add(area.size.height as i32);
            position.x >= left && position.x < right && position.y >= top && position.y < bottom
        })
    })
}
