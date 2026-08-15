use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{Emitter, Manager};
use windows::Graphics::Capture::{GraphicsCaptureAccess, GraphicsCaptureAccessKind};
use windows::Security::Authorization::AppCapabilityAccess::AppCapabilityAccessStatus;
use windows_capture::capture::{Context, GraphicsCaptureApiError, GraphicsCaptureApiHandler};
use windows_capture::encoder::{
    AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
    VideoSettingsSubType,
};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::{GraphicsCaptureApi, InternalCaptureControl};
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use super::targets::{
    resolve_target, CaptureTargetRequest, NativeCaptureTarget, ResolvedCaptureTarget,
};

const RECORDING_STARTED_EVENT: &str = "capture-test-recording-started";
const CAPTURE_DURATION: Duration = Duration::from_secs(5);

static CAPTURE_ACTIVE: AtomicBool = AtomicBool::new(false);

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureTestResult {
    success: bool,
    file_path: Option<String>,
    error_message: Option<String>,
    borderless_active: bool,
    borderless_status: String,
    bordered_capture_available: Option<bool>,
}

impl CaptureTestResult {
    fn success(path: &Path) -> Self {
        Self {
            success: true,
            file_path: Some(path.to_string_lossy().into_owned()),
            error_message: None,
            borderless_active: true,
            borderless_status: "active".to_string(),
            bordered_capture_available: None,
        }
    }

    fn failure(failure: CaptureFailure) -> Self {
        Self {
            success: false,
            file_path: None,
            error_message: Some(failure.message),
            borderless_active: false,
            borderless_status: failure.borderless_status,
            bordered_capture_available: failure.bordered_capture_available,
        }
    }
}

struct CaptureFailure {
    message: String,
    borderless_status: String,
    bordered_capture_available: Option<bool>,
}

impl CaptureFailure {
    fn before_permission(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            borderless_status: "not_attempted".to_string(),
            bordered_capture_available: None,
        }
    }

    fn after_permission(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            borderless_status: "permission_granted".to_string(),
            bordered_capture_available: None,
        }
    }

    fn borderless(failure: BorderlessFailure) -> Self {
        Self {
            message: failure.message,
            borderless_status: failure.status.to_string(),
            bordered_capture_available: Some(failure.bordered_capture_available),
        }
    }
}

struct ActiveCaptureGuard;

impl Drop for ActiveCaptureGuard {
    fn drop(&mut self) {
        CAPTURE_ACTIVE.store(false, Ordering::Release);
    }
}

struct CaptureFlags {
    app: tauri::AppHandle,
    output_path: PathBuf,
    width: u32,
    height: u32,
    permission_granted: Arc<AtomicBool>,
}

struct CaptureTestHandler {
    app: tauri::AppHandle,
    encoder: Option<VideoEncoder>,
    frame_count: u64,
    recording_started: bool,
    started_at: Instant,
}

impl CaptureTestHandler {
    fn finish(&mut self) -> Result<u64, CaptureHandlerError> {
        let encoder = self.encoder.take().ok_or_else(|| {
            CaptureHandlerError::Capture("The capture encoder was already finalized".to_string())
        })?;

        encoder.finish().map_err(|error| {
            CaptureHandlerError::Capture(format!(
                "The MP4 encoder could not finalize the test video: {error}"
            ))
        })?;
        Ok(self.frame_count)
    }
}

#[derive(Debug)]
struct BorderlessFailure {
    status: &'static str,
    message: String,
    bordered_capture_available: bool,
}

#[derive(Debug)]
enum CaptureHandlerError {
    Borderless(BorderlessFailure),
    Capture(String),
}

impl fmt::Display for CaptureHandlerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Borderless(failure) => formatter.write_str(&failure.message),
            Self::Capture(message) => formatter.write_str(message),
        }
    }
}

impl Error for CaptureHandlerError {}

impl GraphicsCaptureApiHandler for CaptureTestHandler {
    type Flags = CaptureFlags;
    type Error = CaptureHandlerError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        request_borderless_access().map_err(CaptureHandlerError::Borderless)?;
        ctx.flags.permission_granted.store(true, Ordering::Release);

        let video_settings = VideoSettingsBuilder::new(ctx.flags.width, ctx.flags.height)
            .sub_type(VideoSettingsSubType::H264)
            .frame_rate(60);

        let encoder = VideoEncoder::new(
            video_settings,
            AudioSettingsBuilder::new().disabled(true),
            ContainerSettingsBuilder::new(),
            &ctx.flags.output_path,
        )
        .map_err(|error| {
            CaptureHandlerError::Capture(format!("The MP4 encoder could not initialize: {error}"))
        })?;

        Ok(Self {
            app: ctx.flags.app,
            encoder: Some(encoder),
            frame_count: 0,
            recording_started: false,
            started_at: Instant::now(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if !self.recording_started {
            let _ = self.app.emit(RECORDING_STARTED_EVENT, ());
            self.recording_started = true;
        }

        if let Some(encoder) = self.encoder.as_mut() {
            encoder.send_frame(frame).map_err(|error| {
                CaptureHandlerError::Capture(format!(
                    "The MP4 encoder rejected a video frame: {error}"
                ))
            })?;
            self.frame_count += 1;
        }

        if self.started_at.elapsed() >= CAPTURE_DURATION {
            self.finish()?;
            capture_control.stop();
        }

        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        self.finish()?;
        Err(CaptureHandlerError::Capture(
            "The selected capture target closed before the five-second test completed.".to_string(),
        ))
    }
}

#[tauri::command]
pub async fn run_capture_test(
    app: tauri::AppHandle,
    target: CaptureTargetRequest,
) -> CaptureTestResult {
    if CAPTURE_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return CaptureTestResult::failure(CaptureFailure::before_permission(
            "A native capture test is already active.",
        ));
    }

    let active_guard = ActiveCaptureGuard;
    let videos_dir = match app.path().video_dir() {
        Ok(path) => path,
        Err(error) => {
            return CaptureTestResult::failure(CaptureFailure::before_permission(format!(
                "Could not locate the Windows Videos folder: {error}"
            )));
        }
    };

    let worker = tauri::async_runtime::spawn_blocking(move || {
        let _active_guard = active_guard;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            capture_five_second_test(&app, &videos_dir, &target)
        }))
    });

    match worker.await {
        Ok(Ok(Ok(path))) => CaptureTestResult::success(&path),
        Ok(Ok(Err(error))) => CaptureTestResult::failure(error),
        Ok(Err(_)) => CaptureTestResult::failure(CaptureFailure::before_permission(
            "The native capture worker panicked while recording or finalizing the MP4.",
        )),
        Err(error) => CaptureTestResult::failure(CaptureFailure::before_permission(format!(
            "The native capture worker could not complete: {error}"
        ))),
    }
}

fn capture_five_second_test(
    app: &tauri::AppHandle,
    videos_dir: &Path,
    target_request: &CaptureTargetRequest,
) -> Result<PathBuf, CaptureFailure> {
    let ResolvedCaptureTarget {
        target,
        width,
        height,
    } = resolve_target(target_request).map_err(CaptureFailure::before_permission)?;
    let width = even_dimension(width).map_err(CaptureFailure::before_permission)?;
    let height = even_dimension(height).map_err(CaptureFailure::before_permission)?;

    let output_dir = videos_dir.join("JustIn Replay").join("DevTests");
    fs::create_dir_all(&output_dir).map_err(|error| {
        CaptureFailure::before_permission(format!(
            "Could not create the capture test output directory '{}': {error}",
            output_dir.display()
        ))
    })?;

    let output_path =
        reserve_output_path(&output_dir).map_err(CaptureFailure::before_permission)?;
    let permission_granted = Arc::new(AtomicBool::new(false));
    let flags = CaptureFlags {
        app: app.clone(),
        output_path: output_path.clone(),
        width,
        height,
        permission_granted: permission_granted.clone(),
    };

    let capture_result = match target {
        NativeCaptureTarget::Monitor(monitor) => start_target_capture(monitor, flags),
        NativeCaptureTarget::Window(window) => start_target_capture(window, flags),
    };

    if let Err(error) = capture_result {
        let _ = fs::remove_file(&output_path);
        return Err(match error {
            GraphicsCaptureApiError::NewHandlerError(CaptureHandlerError::Borderless(failure)) => {
                CaptureFailure::borderless(failure)
            }
            error if permission_granted.load(Ordering::Acquire) => CaptureFailure::after_permission(format!(
                "Native capture could not start or complete after borderless permission was granted: {error}"
            )),
            error => CaptureFailure::before_permission(format!(
                "Native capture could not start or complete: {error}"
            )),
        });
    }

    let file_size = fs::metadata(&output_path)
        .map_err(|error| {
            CaptureFailure::after_permission(format!(
                "The saved MP4 could not be verified: {error}"
            ))
        })?
        .len();
    if file_size == 0 {
        return Err(CaptureFailure::after_permission(
            "The encoder created an empty MP4 file.",
        ));
    }

    Ok(output_path)
}

fn start_target_capture<T>(
    target: T,
    flags: CaptureFlags,
) -> Result<(), GraphicsCaptureApiError<CaptureHandlerError>>
where
    T: TryInto<GraphicsCaptureItemType>,
{
    let settings = Settings::new(
        target,
        CursorCaptureSettings::Default,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags,
    );

    CaptureTestHandler::start(settings)
}

fn even_dimension(value: u32) -> Result<u32, String> {
    if value == 0 {
        return Err("The selected capture target has a zero-sized dimension.".to_string());
    }

    Ok(if value % 2 == 0 { value } else { value + 1 })
}

fn request_borderless_access() -> Result<(), BorderlessFailure> {
    let bordered_capture_available = GraphicsCaptureApi::is_supported().unwrap_or(false);
    match GraphicsCaptureApi::is_border_settings_supported() {
        Ok(true) => {}
        Ok(false) => {
            let message = if bordered_capture_available {
                "This Windows version supports normal capture but does not support disabling the system capture border."
            } else {
                "Windows Graphics Capture is unavailable on this system, including borderless capture."
            };
            return Err(BorderlessFailure {
                status: "unsupported",
                message: message.to_string(),
                bordered_capture_available,
            });
        }
        Err(error) => {
            return Err(BorderlessFailure {
                status: "permission_check_failed",
                message: format!("Windows could not check borderless-capture support: {error}"),
                bordered_capture_available,
            });
        }
    }

    let access = GraphicsCaptureAccess::RequestAccessAsync(GraphicsCaptureAccessKind::Borderless)
        .and_then(|operation| operation.join())
        .map_err(|error| BorderlessFailure {
            status: "permission_request_failed",
            message: format!(
                "Windows could not request borderless-capture permission: {error}. Normal bordered capture may still be available."
            ),
            bordered_capture_available,
        })?;

    let (status, message) = if access == AppCapabilityAccessStatus::Allowed {
        return Ok(());
    } else if access == AppCapabilityAccessStatus::NotDeclaredByApp {
        (
            "capability_not_declared",
            "Windows reports that JustIn Replay has not declared the graphicsCaptureWithoutBorder package capability. Borderless capture cannot be claimed in this build; normal bordered capture may still work.",
        )
    } else if access == AppCapabilityAccessStatus::DeniedByUser {
        (
            "denied_by_user",
            "Windows borderless-capture permission was denied by the user. Normal bordered capture may still work.",
        )
    } else if access == AppCapabilityAccessStatus::DeniedBySystem {
        (
            "denied_by_system",
            "Windows denied borderless-capture permission. The app may be unpackaged, the required capability may be unavailable, or system policy may forbid it. Normal bordered capture may still work.",
        )
    } else if access == AppCapabilityAccessStatus::UserPromptRequired {
        (
            "user_prompt_required",
            "Windows still requires user consent for borderless capture. Normal bordered capture may still work.",
        )
    } else {
        (
            "unknown_permission_status",
            "Windows returned an unknown borderless-capture permission status. Normal bordered capture may still work.",
        )
    };

    Err(BorderlessFailure {
        status,
        message: message.to_string(),
        bordered_capture_available,
    })
}

fn reserve_output_path(output_dir: &Path) -> Result<PathBuf, String> {
    let timestamp = utc_file_timestamp()?;

    for suffix in 0..1000 {
        let file_name = if suffix == 0 {
            format!("capture-test-{timestamp}.mp4")
        } else {
            format!("capture-test-{timestamp}-{suffix}.mp4")
        };
        let path = output_dir.join(file_name);

        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not create the capture test file '{}': {error}",
                    path.display()
                ));
            }
        }
    }

    Err("Could not create a unique capture test filename.".to_string())
}

fn utc_file_timestamp() -> Result<String, String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("The system clock is invalid: {error}"))?
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let seconds_in_day = seconds.rem_euclid(86_400);

    // Convert days since the Unix epoch to a Gregorian date without another dependency.
    let shifted_days = days + 719_468;
    let era = shifted_days.div_euclid(146_097);
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }

    let hour = seconds_in_day / 3_600;
    let minute = seconds_in_day % 3_600 / 60;
    let second = seconds_in_day % 60;

    Ok(format!(
        "{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}"
    ))
}
