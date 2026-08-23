use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Mutex,
};
use std::time::Duration;

use serde::Serialize;
use tauri::menu::{MenuBuilder, MenuItem, MenuItemBuilder};
use tauri::tray::{MouseButton, TrayIcon, TrayIconBuilder, TrayIconEvent};
use tauri::{App, AppHandle, Emitter, Manager, PhysicalPosition, State, Wry};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND, ERROR_SUCCESS};
use windows::Win32::System::Registry::{
    RegCloseKey, RegCreateKeyExW, RegDeleteValueW, RegSetValueExW, HKEY, HKEY_CURRENT_USER,
    KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ,
};

use crate::clips::ClipSaveManager;
use crate::preferences::{UiPreferencesManager, UiPreferencesPatch, UiPreferencesResponse};
use crate::replay::{ReplayBufferManager, ReplayLifecycleState};

const AUTOSTART_SUBKEY: &str = "Software\\Microsoft\\Windows\\CurrentVersion\\Run";
const AUTOSTART_VALUE: &str = "SlickClip";
const TRAY_OPEN: &str = "slickclip-open";
const TRAY_SAVE: &str = "slickclip-save";
const TRAY_QUIT: &str = "slickclip-quit";
const OVERLAY_DURATION: Duration = Duration::from_millis(3_200);

pub struct DesktopIntegration {
    tray: TrayIcon<Wry>,
    tray_status: MenuItem<Wry>,
    tray_save: MenuItem<Wry>,
    exiting: AtomicBool,
    overlay_generation: AtomicU64,
    startup_error: Mutex<Option<String>>,
}

impl DesktopIntegration {
    fn new(
        tray: TrayIcon<Wry>,
        tray_status: MenuItem<Wry>,
        tray_save: MenuItem<Wry>,
        startup_error: Option<String>,
    ) -> Self {
        Self {
            tray,
            tray_status,
            tray_save,
            exiting: AtomicBool::new(false),
            overlay_generation: AtomicU64::new(0),
            startup_error: Mutex::new(startup_error),
        }
    }

    pub fn begin_exit(&self) {
        self.exiting.store(true, Ordering::SeqCst);
    }

    pub fn is_exiting(&self) -> bool {
        self.exiting.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveOverlayPayload {
    title: String,
    detail: String,
}

pub fn setup(app: &mut App) -> tauri::Result<()> {
    let tray_status = MenuItemBuilder::new("Replay Buffer: Stopped")
        .enabled(false)
        .build(app)?;
    let tray_save = MenuItemBuilder::with_id(TRAY_SAVE, "Save Replay")
        .enabled(false)
        .build(app)?;
    let menu = MenuBuilder::new(app)
        .text(TRAY_OPEN, "Open SlickClip")
        .item(&tray_status)
        .separator()
        .item(&tray_save)
        .separator()
        .text(TRAY_QUIT, "Quit SlickClip")
        .build()?;
    let mut tray_builder = TrayIconBuilder::with_id("slickclip")
        .menu(&menu)
        .tooltip("SlickClip — Replay Buffer stopped")
        .show_menu_on_left_click(true)
        .on_menu_event(handle_tray_menu)
        .on_tray_icon_event(|tray, event| {
            if matches!(
                event,
                TrayIconEvent::DoubleClick {
                    button: MouseButton::Left,
                    ..
                }
            ) {
                let _ = show_main_window(tray.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon().cloned() {
        tray_builder = tray_builder.icon(icon);
    }
    let tray = tray_builder.build(app)?;

    let preferences = app.state::<UiPreferencesManager>().get().preferences;
    let startup_error = if preferences.start_with_windows {
        std::env::current_exe()
            .map_err(|error| format!("Could not locate SlickClip for Windows startup: {error}"))
            .and_then(|path| configure_start_with_windows(&path, true))
            .err()
    } else {
        None
    };
    if let Some(error) = startup_error.as_ref() {
        eprintln!("SlickClip Windows startup warning: {error}");
    }

    app.manage(DesktopIntegration::new(
        tray,
        tray_status,
        tray_save,
        startup_error,
    ));
    refresh_tray_status(app.handle());

    let poll_app = app.handle().clone();
    std::thread::Builder::new()
        .name("slickclip-tray-status".to_string())
        .spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            let Some(desktop) = poll_app.try_state::<DesktopIntegration>() else {
                break;
            };
            if desktop.is_exiting() {
                break;
            }
            refresh_tray_status(&poll_app);
        })
        .map_err(tauri::Error::Io)?;
    Ok(())
}

fn handle_tray_menu(app: &AppHandle, event: tauri::menu::MenuEvent) {
    match event.id().0.as_str() {
        TRAY_OPEN => {
            let _ = show_main_window(app);
        }
        TRAY_SAVE => {
            crate::hotkey::request_save_with_feedback(app);
            refresh_tray_status(app);
        }
        TRAY_QUIT => {
            if let Some(desktop) = app.try_state::<DesktopIntegration>() {
                desktop.begin_exit();
            }
            app.exit(0);
        }
        _ => {}
    }
}

pub fn show_main_window(app: &AppHandle) -> Result<(), String> {
    let main = app
        .get_webview_window("main")
        .ok_or_else(|| "The SlickClip main window is unavailable.".to_string())?;
    main.unminimize().map_err(|error| error.to_string())?;
    main.show().map_err(|error| error.to_string())?;
    main.set_focus().map_err(|error| error.to_string())
}

pub fn should_background(app: &AppHandle) -> bool {
    if app
        .try_state::<DesktopIntegration>()
        .is_some_and(|desktop| desktop.is_exiting())
    {
        return false;
    }
    app.try_state::<UiPreferencesManager>()
        .is_some_and(|manager| manager.get().preferences.close_to_tray)
}

pub fn refresh_tray_status(app: &AppHandle) {
    let (Some(desktop), Some(replay), Some(save)) = (
        app.try_state::<DesktopIntegration>(),
        app.try_state::<ReplayBufferManager>(),
        app.try_state::<ClipSaveManager>(),
    ) else {
        return;
    };
    let replay = replay.status();
    let save = save.status();
    let target = replay.target_label.as_deref().unwrap_or("No target");
    let (status, tooltip) = match replay.state {
        ReplayLifecycleState::Stopped => (
            "Replay Buffer: Stopped".to_string(),
            "SlickClip — Replay Buffer stopped".to_string(),
        ),
        ReplayLifecycleState::Starting => (
            "Replay Buffer: Starting…".to_string(),
            format!("SlickClip — Starting {target}"),
        ),
        ReplayLifecycleState::Running => (
            format!("Replay Buffer: Running — {target}"),
            format!("SlickClip — Capturing {target}"),
        ),
        ReplayLifecycleState::Stopping => (
            "Replay Buffer: Stopping…".to_string(),
            "SlickClip — Stopping Replay Buffer".to_string(),
        ),
        ReplayLifecycleState::Error => (
            "Replay Buffer: Needs attention".to_string(),
            "SlickClip — Replay Buffer error".to_string(),
        ),
    };
    let can_save = replay.state == ReplayLifecycleState::Running
        && replay.completed_segment_count > 0
        && !save.state.is_active();
    let _ = desktop.tray_status.set_text(status);
    let _ = desktop.tray_save.set_enabled(can_save);
    let _ = desktop.tray.set_tooltip(Some(tooltip));
}

pub fn show_save_overlay(app: &AppHandle, duration_seconds: f64) {
    let enabled = app
        .try_state::<UiPreferencesManager>()
        .is_none_or(|manager| manager.get().preferences.save_overlay_enabled);
    if !enabled {
        return;
    }
    show_notification_overlay(
        app,
        "Replay Saved",
        &format!("{duration_seconds:.1}s clip added to your Library"),
    );
}

pub fn show_notification_overlay(app: &AppHandle, title: &str, detail: &str) {
    let (Some(desktop), Some(overlay)) = (
        app.try_state::<DesktopIntegration>(),
        app.get_webview_window("save-overlay"),
    ) else {
        return;
    };

    let generation = desktop.overlay_generation.fetch_add(1, Ordering::SeqCst) + 1;
    let monitor = overlay
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| app.primary_monitor().ok().flatten());
    if let Some(monitor) = monitor {
        if let Ok(size) = overlay.outer_size() {
            let work_area = monitor.work_area();
            let (x, y) = overlay_position(
                work_area.position.x,
                work_area.position.y,
                work_area.size.width,
                work_area.size.height,
                size.width,
                size.height,
                18,
            );
            let _ = overlay.set_position(PhysicalPosition::new(x, y));
        }
    }
    let payload = SaveOverlayPayload {
        title: title.to_string(),
        detail: detail.to_string(),
    };
    let _ = overlay.emit("replay-saved-overlay", payload);
    let _ = overlay.set_focusable(false);
    let _ = overlay.show();

    let hide_app = app.clone();
    let _ = std::thread::Builder::new()
        .name("slickclip-save-overlay".to_string())
        .spawn(move || {
            std::thread::sleep(OVERLAY_DURATION);
            let Some(desktop) = hide_app.try_state::<DesktopIntegration>() else {
                return;
            };
            if desktop.overlay_generation.load(Ordering::SeqCst) == generation {
                if let Some(overlay) = hide_app.get_webview_window("save-overlay") {
                    let _ = overlay.hide();
                }
            }
        });
}

#[tauri::command]
pub fn set_start_with_windows(
    manager: State<'_, UiPreferencesManager>,
    desktop: State<'_, DesktopIntegration>,
    enabled: bool,
) -> UiPreferencesResponse {
    let result = std::env::current_exe()
        .map_err(|error| format!("Could not locate SlickClip for Windows startup: {error}"))
        .and_then(|path| configure_start_with_windows(&path, enabled));
    if let Err(error) = result {
        *desktop
            .startup_error
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(error.clone());
        return UiPreferencesResponse {
            success: false,
            preferences: manager.get().preferences,
            error_message: Some(error),
        };
    }
    *desktop
        .startup_error
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
    let response = manager.update(UiPreferencesPatch {
        start_with_windows: Some(enabled),
        ..Default::default()
    });
    if !response.success {
        if let Ok(path) = std::env::current_exe() {
            let _ = configure_start_with_windows(&path, !enabled);
        }
    }
    response
}

fn configure_start_with_windows(executable: &Path, enabled: bool) -> Result<(), String> {
    let subkey = wide_null(OsStr::new(AUTOSTART_SUBKEY));
    let value_name = wide_null(OsStr::new(AUTOSTART_VALUE));
    let mut key = HKEY::default();
    let create_status = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(subkey.as_ptr()),
            None,
            PCWSTR::null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            None,
            &mut key,
            None,
        )
    };
    if create_status != ERROR_SUCCESS {
        return Err(format!(
            "Could not open the Windows startup registry key (error {}).",
            create_status.0
        ));
    }
    let key = RegistryKey(key);

    if enabled {
        let command = startup_command(executable);
        let command_wide = wide_null(OsStr::new(&command));
        let command_bytes = unsafe {
            std::slice::from_raw_parts(
                command_wide.as_ptr().cast::<u8>(),
                command_wide.len() * std::mem::size_of::<u16>(),
            )
        };
        let status = unsafe {
            RegSetValueExW(
                key.0,
                PCWSTR(value_name.as_ptr()),
                None,
                REG_SZ,
                Some(command_bytes),
            )
        };
        if status != ERROR_SUCCESS {
            return Err(format!(
                "Could not enable SlickClip Windows startup (error {}).",
                status.0
            ));
        }
    } else {
        let status = unsafe { RegDeleteValueW(key.0, PCWSTR(value_name.as_ptr())) };
        if status != ERROR_SUCCESS
            && status != ERROR_FILE_NOT_FOUND
            && status != ERROR_PATH_NOT_FOUND
        {
            return Err(format!(
                "Could not disable SlickClip Windows startup (error {}).",
                status.0
            ));
        }
    }
    Ok(())
}

fn startup_command(executable: &Path) -> String {
    format!("\"{}\" --background", executable.display())
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

struct RegistryKey(HKEY);

impl Drop for RegistryKey {
    fn drop(&mut self) {
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn overlay_position(
    work_x: i32,
    work_y: i32,
    work_width: u32,
    work_height: u32,
    overlay_width: u32,
    overlay_height: u32,
    margin: i32,
) -> (i32, i32) {
    let x =
        i64::from(work_x) + i64::from(work_width) - i64::from(overlay_width) - i64::from(margin);
    let y =
        i64::from(work_y) + i64::from(work_height) - i64::from(overlay_height) - i64::from(margin);
    (
        x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
    )
}

#[cfg(test)]
mod tests {
    use super::{overlay_position, startup_command};
    use std::path::Path;

    #[test]
    fn startup_command_quotes_the_executable_and_requests_background_mode() {
        assert_eq!(
            startup_command(Path::new(r"C:\Program Files\SlickClip\SlickClip.exe")),
            r#""C:\Program Files\SlickClip\SlickClip.exe" --background"#
        );
    }

    #[test]
    fn overlay_uses_the_monitor_work_area_and_margin() {
        assert_eq!(
            overlay_position(-1920, 0, 1920, 1040, 340, 86, 18),
            (-358, 936)
        );
    }
}
