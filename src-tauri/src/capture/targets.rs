use serde::{Deserialize, Serialize};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Dxgi::{CreateDXGIFactory1, IDXGIFactory1};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, HMONITOR, MONITORINFO};
use windows_capture::monitor::Monitor;
use windows_capture::window::Window;

use crate::audio::resolve_process_metadata;
use crate::capture::compatibility::PresentationClass;

#[derive(Clone, Serialize)]
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
    pub(crate) id: String,
    pub(crate) title: String,
    pub(crate) process_name: Option<String>,
    pub(crate) process_id: u32,
    pub(crate) width: u32,
    pub(crate) height: u32,
    pub(crate) foreground: bool,
    #[serde(skip_serializing)]
    pub(crate) executable_path: Option<String>,
    #[serde(skip_serializing)]
    pub(crate) monitor_width: Option<u32>,
    #[serde(skip_serializing)]
    pub(crate) monitor_height: Option<u32>,
    #[serde(skip_serializing)]
    pub(crate) title_bar_height: Option<u32>,
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

#[derive(Clone, Copy)]
pub enum NativeCaptureTarget {
    Monitor(Monitor),
    Window(Window),
}

pub struct ResolvedCaptureTarget {
    pub target: NativeCaptureTarget,
    pub label: String,
    pub width: u32,
    pub height: u32,
    pub process_id: Option<u32>,
    pub presentation: Option<PresentationClass>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct DesktopRect {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

impl DesktopRect {
    fn overlap_area(self, other: Self) -> i64 {
        let width = i64::from(self.right.min(other.right) - self.left.max(other.left)).max(0);
        let height = i64::from(self.bottom.min(other.bottom) - self.top.max(other.top)).max(0);
        width.saturating_mul(height)
    }

    fn width(self) -> i64 {
        i64::from(self.right - self.left).max(0)
    }

    fn height(self) -> i64 {
        i64::from(self.bottom - self.top).max(0)
    }

    fn area(self) -> i64 {
        self.width().saturating_mul(self.height())
    }
}

pub fn dxgi_monitor_for_target(target: &NativeCaptureTarget) -> Result<Monitor, String> {
    match target {
        NativeCaptureTarget::Monitor(monitor) => Ok(*monitor),
        NativeCaptureTarget::Window(window) => {
            monitor_and_rect_for_window(*window).map(|(monitor, _, _)| monitor)
        }
    }
}

pub struct FfmpegDisplayOutput {
    pub adapter_index: u32,
    pub output_index: u32,
    pub identity: String,
    pub desktop_x: i32,
    pub desktop_y: i32,
    pub width: u32,
    pub height: u32,
}

pub fn ffmpeg_output_for_target(
    target: &NativeCaptureTarget,
) -> Result<FfmpegDisplayOutput, String> {
    let monitor = dxgi_monitor_for_target(target)?;
    let (adapter_index, output_index) = dxgi_adapter_output_for_monitor(monitor)?;
    let identity = monitor
        .device_name()
        .map_err(|error| format!("Could not identify the selected physical display: {error}"))?;
    let (desktop_x, desktop_y) = monitor_desktop_origin(monitor)?;
    let width = monitor
        .width()
        .map_err(|error| format!("Could not read the selected display width: {error}"))?;
    let height = monitor
        .height()
        .map_err(|error| format!("Could not read the selected display height: {error}"))?;
    Ok(FfmpegDisplayOutput {
        adapter_index,
        output_index,
        identity,
        desktop_x,
        desktop_y,
        width,
        height,
    })
}

fn dxgi_adapter_output_for_monitor(monitor: Monitor) -> Result<(u32, u32), String> {
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1() }
        .map_err(|error| format!("Could not enumerate DXGI adapters for FFmpeg: {error}"))?;
    let selected = HMONITOR(monitor.as_raw_hmonitor());
    let mut candidates = Vec::new();
    for adapter_index in 0..64u32 {
        let Ok(adapter) = (unsafe { factory.EnumAdapters1(adapter_index) }) else {
            break;
        };
        for output_index in 0..64u32 {
            let Ok(output) = (unsafe { adapter.EnumOutputs(output_index) }) else {
                break;
            };
            let description = unsafe { output.GetDesc() }.map_err(|error| {
                format!("Could not inspect DXGI output {adapter_index}:{output_index}: {error}")
            })?;
            candidates.push((adapter_index, output_index, description.Monitor.0 as isize));
        }
    }
    select_dxgi_output_handle(selected.0 as isize, &candidates).ok_or_else(|| {
        "Could not map the selected physical monitor to a DXGI adapter/output pair for FFmpeg ddagrab."
            .to_string()
    })
}

fn select_dxgi_output_handle(
    selected_monitor: isize,
    candidates: &[(u32, u32, isize)],
) -> Option<(u32, u32)> {
    candidates
        .iter()
        .find(|(_, _, monitor)| *monitor == selected_monitor)
        .map(|(adapter, output, _)| (*adapter, *output))
}

fn monitor_and_rect_for_window(
    window: Window,
) -> Result<(Monitor, DesktopRect, DesktopRect), String> {
    let rect = window.rect().map_err(|error| {
        format!("Could not determine the selected window's desktop position: {error}")
    })?;
    let window_rect = DesktopRect::from(rect);
    Monitor::enumerate()
        .map_err(|error| format!("Could not enumerate displays for capture targeting: {error}"))?
        .into_iter()
        .filter_map(|monitor| monitor_rect(monitor).ok().map(|rect| (monitor, rect)))
        .max_by_key(|(_, rect)| window_rect.overlap_area(*rect))
        .filter(|(_, rect)| window_rect.overlap_area(*rect) > 0)
        .map(|(monitor, monitor_rect)| (monitor, window_rect, monitor_rect))
        .ok_or_else(|| "Could not match the selected window to a display.".to_string())
}

fn classify_presentation_geometry(
    window: DesktopRect,
    monitor: DesktopRect,
    title_bar_height: Option<u32>,
) -> PresentationClass {
    if title_bar_height.is_some_and(|height| height > 0) || monitor.area() == 0 {
        return PresentationClass::Windowed;
    }
    let overlap = window.overlap_area(monitor);
    let covers_area = overlap.saturating_mul(100) >= monitor.area().saturating_mul(95);
    let covers_width = window.width().saturating_mul(100) >= monitor.width().saturating_mul(95);
    let covers_height = window.height().saturating_mul(100) >= monitor.height().saturating_mul(95);
    if covers_area && covers_width && covers_height {
        PresentationClass::FullscreenLike
    } else {
        PresentationClass::Windowed
    }
}

#[allow(dead_code)]
pub fn monitor_desktop_origin(monitor: Monitor) -> Result<(i32, i32), String> {
    let rect = monitor_rect(monitor)?;
    Ok((rect.left, rect.top))
}

impl From<RECT> for DesktopRect {
    fn from(rect: RECT) -> Self {
        Self {
            left: rect.left,
            top: rect.top,
            right: rect.right,
            bottom: rect.bottom,
        }
    }
}

fn monitor_rect(monitor: Monitor) -> Result<DesktopRect, String> {
    let mut info = MONITORINFO {
        cbSize: std::mem::size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetMonitorInfoW(HMONITOR(monitor.as_raw_hmonitor()), &mut info) }
        .as_bool()
        .then(|| DesktopRect::from(info.rcMonitor))
        .ok_or_else(|| "Windows could not read display geometry.".to_string())
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

pub(crate) fn enumerate_windows() -> Result<Vec<WindowTarget>, String> {
    let windows = Window::enumerate()
        .map_err(|error| format!("Could not enumerate capturable windows: {error}"))?;
    let mut targets = Vec::new();
    let foreground = Window::foreground().ok();

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

        let process_metadata = resolve_process_metadata(process_id);
        let process_name = window.process_name().ok().or_else(|| {
            process_metadata
                .as_ref()
                .map(|metadata| metadata.process_name.clone())
        });
        let executable_path = process_metadata.and_then(|metadata| metadata.executable_path);
        let monitor = window.monitor();

        targets.push(WindowTarget {
            id: window_id(window, process_id),
            title,
            process_name,
            process_id,
            width,
            height,
            foreground: foreground == Some(window),
            executable_path,
            monitor_width: monitor.and_then(|monitor| monitor.width().ok()),
            monitor_height: monitor.and_then(|monitor| monitor.height().ok()),
            title_bar_height: window.title_bar_height().ok(),
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
            process_id: None,
            presentation: None,
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
        let (_, window_rect, monitor_rect) = monitor_and_rect_for_window(window)?;
        let presentation = classify_presentation_geometry(
            window_rect,
            monitor_rect,
            window.title_bar_height().ok(),
        );
        let width = window
            .width()
            .map_err(|error| format!("Could not read the selected window width: {error}"))?;
        let height = window
            .height()
            .map_err(|error| format!("Could not read the selected window height: {error}"))?;
        return Ok(ResolvedCaptureTarget {
            target: NativeCaptureTarget::Window(window),
            label: process_name
                .as_ref()
                .map(|name| format!("{name} - {title}"))
                .unwrap_or(title),
            width: positive_dimension(width, "width")?,
            height: positive_dimension(height, "height")?,
            process_id: Some(process_id),
            presentation: Some(presentation),
        });
    }

    Err(
        "The selected window is no longer available. Refresh the target list and try again."
            .to_string(),
    )
}

pub(crate) fn resolve_discord_window(id: &str) -> Result<ResolvedCaptureTarget, String> {
    let metadata = enumerate_windows()?
        .into_iter()
        .find(|window| window.id == id)
        .ok_or_else(|| {
            "The selected Discord reaction window is no longer available. Refresh and try again."
                .to_string()
        })?;
    let is_discord = metadata.process_name.as_deref().is_some_and(|name| {
        let normalized = name.trim_end_matches(".exe");
        normalized.eq_ignore_ascii_case("discord")
            || normalized.to_ascii_lowercase().starts_with("discord")
    });
    if !is_discord {
        return Err(
            "Watch Party reactions must use one whole window owned by the Discord desktop app."
                .to_string(),
        );
    }
    resolve_window(id)
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

#[cfg(test)]
mod compatibility_tests {
    use super::{classify_presentation_geometry, select_dxgi_output_handle, DesktopRect};
    use crate::capture::compatibility::PresentationClass;

    #[test]
    fn overlap_selection_handles_secondary_monitor_and_negative_coordinates() {
        let window = DesktopRect {
            left: -1700,
            top: 100,
            right: 300,
            bottom: 1000,
        };
        let left_monitor = DesktopRect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let primary = DesktopRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert!(window.overlap_area(left_monitor) > window.overlap_area(primary));
    }

    #[test]
    fn selected_hmonitor_maps_to_its_exact_adapter_local_dxgi_output() {
        let candidates = [(0, 0, 100), (1, 0, 200), (1, 1, 300)];
        assert_eq!(select_dxgi_output_handle(300, &candidates), Some((1, 1)));
        assert_eq!(select_dxgi_output_handle(999, &candidates), None);
    }

    #[test]
    fn non_overlapping_rectangles_have_zero_area() {
        let left = DesktopRect {
            left: -1920,
            top: 0,
            right: 0,
            bottom: 1080,
        };
        let right = DesktopRect {
            left: 100,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        assert_eq!(left.overlap_area(right), 0);
    }

    #[test]
    fn normal_window_is_classified_as_windowed() {
        let monitor = DesktopRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let window = DesktopRect {
            left: 200,
            top: 120,
            right: 1500,
            bottom: 900,
        };
        assert_eq!(
            classify_presentation_geometry(window, monitor, Some(30)),
            PresentationClass::Windowed
        );
    }

    #[test]
    fn borderless_monitor_sized_window_is_fullscreen_like() {
        let monitor = DesktopRect {
            left: -2560,
            top: 0,
            right: 0,
            bottom: 1440,
        };
        let window = DesktopRect {
            left: -2560,
            top: 0,
            right: 0,
            bottom: 1440,
        };
        assert_eq!(
            classify_presentation_geometry(window, monitor, None),
            PresentationClass::FullscreenLike
        );
    }

    #[test]
    fn maximized_bordered_window_remains_windowed() {
        let monitor = DesktopRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1080,
        };
        let window = DesktopRect {
            left: 0,
            top: 0,
            right: 1920,
            bottom: 1040,
        };
        assert_eq!(
            classify_presentation_geometry(window, monitor, Some(31)),
            PresentationClass::Windowed
        );
    }
}
