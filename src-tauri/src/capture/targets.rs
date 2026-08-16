use serde::{Deserialize, Serialize};
use windows_capture::monitor::Monitor;
use windows_capture::window::Window;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorTarget {
    id: String,
    display_index: usize,
    friendly_name: String,
    width: u32,
    height: u32,
    refresh_rate: Option<u32>,
    primary: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowTarget {
    id: String,
    title: String,
    process_name: Option<String>,
    process_id: u32,
    width: u32,
    height: u32,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TargetListResult<T> {
    success: bool,
    targets: Vec<T>,
    error_message: Option<String>,
}

impl<T> TargetListResult<T> {
    fn success(targets: Vec<T>) -> Self {
        Self {
            success: true,
            targets,
            error_message: None,
        }
    }

    fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            targets: Vec::new(),
            error_message: Some(error.into()),
        }
    }
}

#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "lowercase")]
pub enum CaptureTargetType {
    Monitor,
    Window,
}

#[derive(Deserialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct CaptureTargetRequest {
    pub target_type: CaptureTargetType,
    pub id: String,
}

pub enum NativeCaptureTarget {
    Monitor(Monitor),
    Window(Window),
}

pub struct ResolvedCaptureTarget {
    pub target: NativeCaptureTarget,
    pub label: String,
    pub width: u32,
    pub height: u32,
}

#[tauri::command]
pub async fn list_capture_monitors() -> TargetListResult<MonitorTarget> {
    match tauri::async_runtime::spawn_blocking(enumerate_monitors).await {
        Ok(Ok(targets)) => TargetListResult::success(targets),
        Ok(Err(error)) => TargetListResult::failure(error),
        Err(error) => TargetListResult::failure(format!(
            "The monitor enumeration worker could not complete: {error}"
        )),
    }
}

#[tauri::command]
pub async fn list_capture_windows() -> TargetListResult<WindowTarget> {
    match tauri::async_runtime::spawn_blocking(enumerate_windows).await {
        Ok(Ok(targets)) => TargetListResult::success(targets),
        Ok(Err(error)) => TargetListResult::failure(error),
        Err(error) => TargetListResult::failure(format!(
            "The window enumeration worker could not complete: {error}"
        )),
    }
}

pub fn resolve_target(request: &CaptureTargetRequest) -> Result<ResolvedCaptureTarget, String> {
    match request.target_type {
        CaptureTargetType::Monitor => resolve_monitor(&request.id),
        CaptureTargetType::Window => resolve_window(&request.id),
    }
}

fn enumerate_monitors() -> Result<Vec<MonitorTarget>, String> {
    let monitors = Monitor::enumerate()
        .map_err(|error| format!("Could not enumerate Windows displays: {error}"))?;
    let primary = Monitor::primary().ok();
    let mut targets = Vec::new();
    let mut last_error = None;

    for (position, monitor) in monitors.into_iter().enumerate() {
        match monitor_metadata(monitor, position + 1, primary) {
            Ok(target) => targets.push(target),
            Err(error) => last_error = Some(error),
        }
    }

    if targets.is_empty() {
        if let Some(error) = last_error {
            return Err(error);
        }
    }

    Ok(targets)
}

fn monitor_metadata(
    monitor: Monitor,
    fallback_index: usize,
    primary: Option<Monitor>,
) -> Result<MonitorTarget, String> {
    let device_name = monitor
        .device_name()
        .map_err(|error| format!("Could not identify display {fallback_index}: {error}"))?;
    let display_index = monitor.index().unwrap_or(fallback_index);
    let friendly_name = monitor.name().unwrap_or_else(|_| device_name.clone());
    let width = monitor
        .width()
        .map_err(|error| format!("Could not read display {display_index} width: {error}"))?;
    let height = monitor
        .height()
        .map_err(|error| format!("Could not read display {display_index} height: {error}"))?;

    Ok(MonitorTarget {
        id: monitor_id(&device_name),
        display_index,
        friendly_name,
        width,
        height,
        refresh_rate: monitor.refresh_rate().ok(),
        primary: primary == Some(monitor),
    })
}

fn enumerate_windows() -> Result<Vec<WindowTarget>, String> {
    let windows = Window::enumerate()
        .map_err(|error| format!("Could not enumerate capturable windows: {error}"))?;
    let mut targets = Vec::new();

    for window in windows {
        let title = match window.title() {
            Ok(title) if !title.trim().is_empty() => title,
            _ => continue,
        };
        let process_id = match window.process_id() {
            Ok(process_id) => process_id,
            Err(_) => continue,
        };
        let (width, height) = match (window.width(), window.height()) {
            (Ok(width), Ok(height)) if width > 1 && height > 1 => (width as u32, height as u32),
            _ => continue,
        };

        targets.push(WindowTarget {
            id: window_id(window, process_id),
            title,
            process_name: window.process_name().ok(),
            process_id,
            width,
            height,
        });
    }

    targets.sort_by(|left, right| {
        left.process_name
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .cmp(&right.process_name.as_deref().unwrap_or("").to_lowercase())
            .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
    });

    Ok(targets)
}

fn resolve_monitor(id: &str) -> Result<ResolvedCaptureTarget, String> {
    let monitors = Monitor::enumerate()
        .map_err(|error| format!("Could not refresh Windows displays: {error}"))?;

    for monitor in monitors {
        let Ok(device_name) = monitor.device_name() else {
            continue;
        };
        if monitor_id(&device_name) != id {
            continue;
        }

        let display_index = monitor.index().unwrap_or(1);
        let friendly_name = monitor.name().unwrap_or_else(|_| device_name.clone());
        let width = monitor
            .width()
            .map_err(|error| format!("Could not read the selected display width: {error}"))?;
        let height = monitor
            .height()
            .map_err(|error| format!("Could not read the selected display height: {error}"))?;
        return Ok(ResolvedCaptureTarget {
            target: NativeCaptureTarget::Monitor(monitor),
            label: format!("Display {display_index} - {friendly_name}"),
            width,
            height,
        });
    }

    Err(
        "The selected display is no longer available. Refresh the target list and try again."
            .to_string(),
    )
}

fn resolve_window(id: &str) -> Result<ResolvedCaptureTarget, String> {
    let windows = Window::enumerate()
        .map_err(|error| format!("Could not refresh capturable windows: {error}"))?;

    for window in windows {
        let Ok(process_id) = window.process_id() else {
            continue;
        };
        if window_id(window, process_id) != id {
            continue;
        }

        let title = window
            .title()
            .unwrap_or_else(|_| format!("Window owned by process {process_id}"));
        let process_name = window.process_name().ok();
        let width = window
            .width()
            .map_err(|error| format!("Could not read the selected window width: {error}"))?;
        let height = window
            .height()
            .map_err(|error| format!("Could not read the selected window height: {error}"))?;
        return Ok(ResolvedCaptureTarget {
            target: NativeCaptureTarget::Window(window),
            label: process_name
                .map(|name| format!("{name} - {title}"))
                .unwrap_or(title),
            width: positive_dimension(width, "width")?,
            height: positive_dimension(height, "height")?,
        });
    }

    Err(
        "The selected window is no longer available. Refresh the target list and try again."
            .to_string(),
    )
}

fn monitor_id(device_name: &str) -> String {
    format!("monitor:{device_name}")
}

fn window_id(window: Window, process_id: u32) -> String {
    format!("window:{:X}:{process_id}", window.as_raw_hwnd() as usize)
}

fn positive_dimension(value: i32, name: &str) -> Result<u32, String> {
    u32::try_from(value)
        .ok()
        .filter(|value| *value > 0)
        .ok_or_else(|| format!("The selected window {name} is invalid ({value})."))
}
