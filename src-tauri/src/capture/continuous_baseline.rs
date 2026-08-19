use std::error::Error;
use std::fmt;
use std::fs::{self, OpenOptions};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{Manager, State};
use windows_capture::capture::{Context, GraphicsCaptureApiError, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use super::capture_test::ensure_borderless_capture_access;
use super::encoder::{
    resolve_encoder, EncoderChoice, EncoderCodec, EncoderFrameTelemetry, VideoEncoderBackend,
    WindowsCaptureFileBackend,
};
use super::targets::{
    resolve_target, CaptureTargetRequest, NativeCaptureTarget, ResolvedCaptureTarget,
};
use super::WGC_FRAME_POOL_BUFFER_COUNT;
use crate::replay::ReplayBufferManager;

const BASELINE_DURATION: Duration = Duration::from_secs(20);
const MATERIAL_GAP_INTERVALS: f64 = 2.0;
static BASELINE_ACTIVE: AtomicBool = AtomicBool::new(false);

pub(crate) fn is_continuous_baseline_active() -> bool {
    BASELINE_ACTIVE.load(Ordering::Acquire)
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ContinuousBaselineResult {
    success: bool,
    error_message: Option<String>,
    file_path: Option<String>,
    requested_encoder: String,
    actual_encoder: Option<String>,
    frame_rate: u32,
    width: u32,
    height: u32,
    expected_frame_interval_ms: f64,
    total_wall_duration_ms: f64,
    frames_observed: u64,
    first_source_timestamp_100ns: Option<i64>,
    last_source_timestamp_100ns: Option<i64>,
    average_consecutive_delta_ms: Option<f64>,
    worst_consecutive_delta_ms: Option<f64>,
    intervals_over_two_expected: u64,
    estimated_frames_missed: u64,
    average_callback_duration_ms: Option<f64>,
    worst_callback_duration_ms: Option<f64>,
    average_send_frame_duration_ms: Option<f64>,
    worst_send_frame_duration_ms: Option<f64>,
    #[serde(rename = "sendFrameOver16_67Ms")]
    send_frame_over_16_67_ms: u64,
    #[serde(rename = "sendFrameOver33_33Ms")]
    send_frame_over_33_33_ms: u64,
    send_frame_over_50_ms: u64,
    send_frame_over_100_ms: u64,
    owned_frame_copies: u64,
    average_gpu_copy_duration_ms: Option<f64>,
    worst_gpu_copy_duration_ms: Option<f64>,
    encoder_queue_depth: u64,
    maximum_encoder_queue_depth: u64,
    encoder_queue_capacity: usize,
    encoder_queue_full_events: u64,
    deliberately_dropped_frames: u64,
    frame_pool_creation_method: String,
    frame_pool_buffer_count: u32,
    finalization_duration_ms: Option<f64>,
}

impl ContinuousBaselineResult {
    fn failure(
        requested_encoder: EncoderChoice,
        frame_rate: u32,
        message: impl Into<String>,
    ) -> Self {
        Self {
            success: false,
            error_message: Some(message.into()),
            file_path: None,
            requested_encoder: requested_encoder.result_name().to_string(),
            actual_encoder: None,
            frame_rate,
            width: 0,
            height: 0,
            expected_frame_interval_ms: 1_000.0 / f64::from(frame_rate.max(1)),
            total_wall_duration_ms: 0.0,
            frames_observed: 0,
            first_source_timestamp_100ns: None,
            last_source_timestamp_100ns: None,
            average_consecutive_delta_ms: None,
            worst_consecutive_delta_ms: None,
            intervals_over_two_expected: 0,
            estimated_frames_missed: 0,
            average_callback_duration_ms: None,
            worst_callback_duration_ms: None,
            average_send_frame_duration_ms: None,
            worst_send_frame_duration_ms: None,
            send_frame_over_16_67_ms: 0,
            send_frame_over_33_33_ms: 0,
            send_frame_over_50_ms: 0,
            send_frame_over_100_ms: 0,
            owned_frame_copies: 0,
            average_gpu_copy_duration_ms: None,
            worst_gpu_copy_duration_ms: None,
            encoder_queue_depth: 0,
            maximum_encoder_queue_depth: 0,
            encoder_queue_capacity: 0,
            encoder_queue_full_events: 0,
            deliberately_dropped_frames: 0,
            frame_pool_creation_method: "CreateFreeThreaded".to_string(),
            frame_pool_buffer_count: WGC_FRAME_POOL_BUFFER_COUNT,
            finalization_duration_ms: None,
        }
    }
}

#[derive(Default)]
struct DurationStats {
    count: u64,
    total_ms: f64,
    worst_ms: f64,
}

impl DurationStats {
    fn record(&mut self, duration: Duration) {
        let duration_ms = duration.as_secs_f64() * 1_000.0;
        self.count += 1;
        self.total_ms += duration_ms;
        self.worst_ms = self.worst_ms.max(duration_ms);
    }

    fn average_ms(&self) -> Option<f64> {
        (self.count > 0).then(|| self.total_ms / self.count as f64)
    }

    fn worst_ms(&self) -> Option<f64> {
        (self.count > 0).then_some(self.worst_ms)
    }
}

struct BaselineMeasurements {
    capture_started: Instant,
    capture_stopped: Option<Instant>,
    frames_observed: u64,
    first_source_timestamp_100ns: Option<i64>,
    last_source_timestamp_100ns: Option<i64>,
    consecutive_delta_count: u64,
    total_consecutive_delta_ms: f64,
    worst_consecutive_delta_ms: f64,
    intervals_over_two_expected: u64,
    estimated_frames_missed: u64,
    callback: DurationStats,
    send_frame: DurationStats,
    send_frame_over_16_67_ms: u64,
    send_frame_over_33_33_ms: u64,
    send_frame_over_50_ms: u64,
    send_frame_over_100_ms: u64,
    owned_frame_copies: u64,
    gpu_copy: DurationStats,
    encoder_queue_depth: u64,
    maximum_encoder_queue_depth: u64,
    encoder_queue_capacity: usize,
    encoder_queue_full_events: u64,
    deliberately_dropped_frames: u64,
    finalization_duration_ms: Option<f64>,
    finalization_error: Option<String>,
}

impl BaselineMeasurements {
    fn new() -> Self {
        Self {
            capture_started: Instant::now(),
            capture_stopped: None,
            frames_observed: 0,
            first_source_timestamp_100ns: None,
            last_source_timestamp_100ns: None,
            consecutive_delta_count: 0,
            total_consecutive_delta_ms: 0.0,
            worst_consecutive_delta_ms: 0.0,
            intervals_over_two_expected: 0,
            estimated_frames_missed: 0,
            callback: DurationStats::default(),
            send_frame: DurationStats::default(),
            send_frame_over_16_67_ms: 0,
            send_frame_over_33_33_ms: 0,
            send_frame_over_50_ms: 0,
            send_frame_over_100_ms: 0,
            owned_frame_copies: 0,
            gpu_copy: DurationStats::default(),
            encoder_queue_depth: 0,
            maximum_encoder_queue_depth: 0,
            encoder_queue_capacity: 0,
            encoder_queue_full_events: 0,
            deliberately_dropped_frames: 0,
            finalization_duration_ms: None,
            finalization_error: None,
        }
    }

    fn record_frame(
        &mut self,
        timestamp_100ns: i64,
        frame_rate: u32,
        send_frame_duration: Duration,
        encoder: EncoderFrameTelemetry,
    ) {
        let expected_ms = 1_000.0 / f64::from(frame_rate.max(1));
        if let Some(previous) = self.last_source_timestamp_100ns {
            let delta_ms = (i128::from(timestamp_100ns) - i128::from(previous)) as f64 / 10_000.0;
            self.consecutive_delta_count += 1;
            self.total_consecutive_delta_ms += delta_ms;
            self.worst_consecutive_delta_ms = self.worst_consecutive_delta_ms.max(delta_ms);
            if delta_ms > expected_ms * MATERIAL_GAP_INTERVALS {
                self.intervals_over_two_expected += 1;
            }
            let observed_intervals = if delta_ms > 0.0 {
                (delta_ms / expected_ms).round() as u64
            } else {
                0
            };
            self.estimated_frames_missed = self
                .estimated_frames_missed
                .saturating_add(observed_intervals.saturating_sub(1));
        }

        self.frames_observed += 1;
        self.first_source_timestamp_100ns
            .get_or_insert(timestamp_100ns);
        self.last_source_timestamp_100ns = Some(timestamp_100ns);
        self.send_frame.record(send_frame_duration);

        let send_ms = send_frame_duration.as_secs_f64() * 1_000.0;
        self.send_frame_over_16_67_ms += u64::from(send_ms > 16.67);
        self.send_frame_over_33_33_ms += u64::from(send_ms > 33.33);
        self.send_frame_over_50_ms += u64::from(send_ms > 50.0);
        self.send_frame_over_100_ms += u64::from(send_ms > 100.0);

        self.encoder_queue_depth = encoder.queue_depth;
        self.maximum_encoder_queue_depth =
            self.maximum_encoder_queue_depth.max(encoder.queue_depth);
        self.encoder_queue_capacity = encoder.queue_capacity;
        if encoder.queued {
            if let Some(copy_duration) = encoder.gpu_copy_duration {
                self.owned_frame_copies += 1;
                self.gpu_copy.record(copy_duration);
            }
        } else {
            self.encoder_queue_full_events += 1;
            self.deliberately_dropped_frames += 1;
        }
    }
}

struct BaselineFlags {
    output_path: PathBuf,
    codec: EncoderCodec,
    width: u32,
    height: u32,
    frame_rate: u32,
    measurements: Arc<Mutex<BaselineMeasurements>>,
}

struct ContinuousBaselineHandler {
    encoder: Option<Box<dyn VideoEncoderBackend>>,
    frame_rate: u32,
    measurements: Arc<Mutex<BaselineMeasurements>>,
}

impl ContinuousBaselineHandler {
    fn finalize(&mut self) {
        let Some(encoder) = self.encoder.take() else {
            return;
        };
        let started = Instant::now();
        let result = encoder.finish();
        let mut measurements = lock_measurements(&self.measurements);
        measurements.finalization_duration_ms = Some(started.elapsed().as_secs_f64() * 1_000.0);
        if let Err(error) = result {
            measurements.finalization_error = Some(error.to_string());
        }
    }
}

impl Drop for ContinuousBaselineHandler {
    fn drop(&mut self) {
        self.finalize();
    }
}

#[derive(Debug)]
struct BaselineHandlerError(String);

impl fmt::Display for BaselineHandlerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for BaselineHandlerError {}

impl GraphicsCaptureApiHandler for ContinuousBaselineHandler {
    type Flags = BaselineFlags;
    type Error = BaselineHandlerError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        ensure_borderless_capture_access().map_err(BaselineHandlerError)?;
        let encoder = WindowsCaptureFileBackend::create(
            &ctx.flags.output_path,
            ctx.flags.codec,
            ctx.flags.width,
            ctx.flags.height,
            ctx.flags.frame_rate,
        )
        .map_err(|error| {
            BaselineHandlerError(format!("Baseline encoder initialization failed: {error}"))
        })?;
        lock_measurements(&ctx.flags.measurements).capture_started = Instant::now();

        Ok(Self {
            encoder: Some(encoder),
            frame_rate: ctx.flags.frame_rate,
            measurements: ctx.flags.measurements,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let callback_started = Instant::now();
        let timestamp = frame
            .timestamp()
            .map_err(|error| {
                BaselineHandlerError(format!("Could not read baseline frame timestamp: {error}"))
            })?
            .Duration;
        let send_started = Instant::now();
        let encoded = self
            .encoder
            .as_mut()
            .ok_or_else(|| {
                BaselineHandlerError("The baseline encoder was already finalized.".to_string())
            })?
            .encode_frame(frame)
            .map_err(|error| {
                BaselineHandlerError(format!("Baseline encoder rejected a frame: {error}"))
            })?;
        let send_duration = send_started.elapsed();
        let mut measurements = lock_measurements(&self.measurements);
        measurements.record_frame(timestamp, self.frame_rate, send_duration, encoded.telemetry);
        measurements.callback.record(callback_started.elapsed());
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        Err(BaselineHandlerError(
            "The selected capture target closed before the continuous baseline completed."
                .to_string(),
        ))
    }
}

struct BaselineActiveGuard;

impl Drop for BaselineActiveGuard {
    fn drop(&mut self) {
        BASELINE_ACTIVE.store(false, Ordering::Release);
    }
}

#[tauri::command]
pub async fn run_continuous_baseline(
    app: tauri::AppHandle,
    replay_manager: State<'_, ReplayBufferManager>,
    target: CaptureTargetRequest,
    encoder: EncoderChoice,
    frame_rate: u32,
) -> Result<ContinuousBaselineResult, String> {
    let replay_manager = replay_manager.inner().clone();
    if replay_manager.status().state.is_active() {
        return Ok(ContinuousBaselineResult::failure(
            encoder,
            frame_rate,
            "Stop the Replay Buffer before running the isolated continuous baseline.",
        ));
    }
    if !matches!(frame_rate, 30 | 60) {
        return Ok(ContinuousBaselineResult::failure(
            encoder,
            frame_rate,
            format!("Baseline frame rate must be 30 or 60 FPS; received {frame_rate}."),
        ));
    }
    if matches!(encoder, EncoderChoice::Av1) {
        return Ok(ContinuousBaselineResult::failure(
            encoder,
            frame_rate,
            "AV1 is not available through windows-capture 2.0.1.",
        ));
    }
    if BASELINE_ACTIVE
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return Ok(ContinuousBaselineResult::failure(
            encoder,
            frame_rate,
            "A continuous-capture baseline is already running.",
        ));
    }
    let active_guard = BaselineActiveGuard;

    let videos_dir = match app.path().video_dir() {
        Ok(path) => path,
        Err(error) => {
            return Ok(ContinuousBaselineResult::failure(
                encoder,
                frame_rate,
                format!("Could not locate the Windows Videos folder: {error}"),
            ));
        }
    };

    let worker = tauri::async_runtime::spawn_blocking(move || {
        let _active_guard = active_guard;
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run_baseline(&videos_dir, &target, encoder, frame_rate)
        }))
    });

    Ok(match worker.await {
        Ok(Ok(result)) => result,
        Ok(Err(_)) => ContinuousBaselineResult::failure(
            encoder,
            frame_rate,
            "The continuous-capture baseline worker panicked.",
        ),
        Err(error) => ContinuousBaselineResult::failure(
            encoder,
            frame_rate,
            format!("The continuous-capture baseline worker could not complete: {error}"),
        ),
    })
}

fn run_baseline(
    videos_dir: &Path,
    target_request: &CaptureTargetRequest,
    encoder_choice: EncoderChoice,
    frame_rate: u32,
) -> ContinuousBaselineResult {
    let resolved_encoder = match resolve_encoder(encoder_choice) {
        Ok(encoder) => encoder,
        Err(error) => return ContinuousBaselineResult::failure(encoder_choice, frame_rate, error),
    };
    let ResolvedCaptureTarget {
        target,
        width,
        height,
        ..
    } = match resolve_target(target_request) {
        Ok(target) => target,
        Err(error) => return ContinuousBaselineResult::failure(encoder_choice, frame_rate, error),
    };
    let width = match even_dimension(width) {
        Ok(width) => width,
        Err(error) => return ContinuousBaselineResult::failure(encoder_choice, frame_rate, error),
    };
    let height = match even_dimension(height) {
        Ok(height) => height,
        Err(error) => return ContinuousBaselineResult::failure(encoder_choice, frame_rate, error),
    };

    let output_dir = videos_dir.join("JustIn Replay").join("DevTests");
    if let Err(error) = fs::create_dir_all(&output_dir) {
        return ContinuousBaselineResult::failure(
            encoder_choice,
            frame_rate,
            format!(
                "Could not create baseline output directory '{}': {error}",
                output_dir.display()
            ),
        );
    }
    let output_path = match reserve_output_path(&output_dir) {
        Ok(path) => path,
        Err(error) => return ContinuousBaselineResult::failure(encoder_choice, frame_rate, error),
    };
    let measurements = Arc::new(Mutex::new(BaselineMeasurements::new()));
    let flags = BaselineFlags {
        output_path: output_path.clone(),
        codec: resolved_encoder.actual,
        width,
        height,
        frame_rate,
        measurements: Arc::clone(&measurements),
    };

    let capture_result = match target {
        NativeCaptureTarget::Monitor(monitor) => start_baseline_capture(monitor, flags),
        NativeCaptureTarget::Window(window) => start_baseline_capture(window, flags),
    };
    if let Err(error) = capture_result {
        let _ = fs::remove_file(&output_path);
        return ContinuousBaselineResult::failure(encoder_choice, frame_rate, error);
    }

    let telemetry = lock_measurements(&measurements);
    if let Some(error) = telemetry.finalization_error.as_ref() {
        let _ = fs::remove_file(&output_path);
        return ContinuousBaselineResult::failure(
            encoder_choice,
            frame_rate,
            format!("The baseline MP4 could not be finalized: {error}"),
        );
    }
    if telemetry.frames_observed == 0 {
        let _ = fs::remove_file(&output_path);
        return ContinuousBaselineResult::failure(
            encoder_choice,
            frame_rate,
            "The continuous baseline completed without receiving a source frame.",
        );
    }

    ContinuousBaselineResult {
        success: true,
        error_message: None,
        file_path: Some(output_path.to_string_lossy().into_owned()),
        requested_encoder: encoder_choice.result_name().to_string(),
        actual_encoder: Some(resolved_encoder.actual.display_name().to_string()),
        frame_rate,
        width,
        height,
        expected_frame_interval_ms: 1_000.0 / f64::from(frame_rate),
        total_wall_duration_ms: telemetry
            .capture_stopped
            .unwrap_or_else(Instant::now)
            .duration_since(telemetry.capture_started)
            .as_secs_f64()
            * 1_000.0,
        frames_observed: telemetry.frames_observed,
        first_source_timestamp_100ns: telemetry.first_source_timestamp_100ns,
        last_source_timestamp_100ns: telemetry.last_source_timestamp_100ns,
        average_consecutive_delta_ms: (telemetry.consecutive_delta_count > 0).then(|| {
            telemetry.total_consecutive_delta_ms / telemetry.consecutive_delta_count as f64
        }),
        worst_consecutive_delta_ms: (telemetry.consecutive_delta_count > 0)
            .then_some(telemetry.worst_consecutive_delta_ms),
        intervals_over_two_expected: telemetry.intervals_over_two_expected,
        estimated_frames_missed: telemetry.estimated_frames_missed,
        average_callback_duration_ms: telemetry.callback.average_ms(),
        worst_callback_duration_ms: telemetry.callback.worst_ms(),
        average_send_frame_duration_ms: telemetry.send_frame.average_ms(),
        worst_send_frame_duration_ms: telemetry.send_frame.worst_ms(),
        send_frame_over_16_67_ms: telemetry.send_frame_over_16_67_ms,
        send_frame_over_33_33_ms: telemetry.send_frame_over_33_33_ms,
        send_frame_over_50_ms: telemetry.send_frame_over_50_ms,
        send_frame_over_100_ms: telemetry.send_frame_over_100_ms,
        owned_frame_copies: telemetry.owned_frame_copies,
        average_gpu_copy_duration_ms: telemetry.gpu_copy.average_ms(),
        worst_gpu_copy_duration_ms: telemetry.gpu_copy.worst_ms(),
        encoder_queue_depth: telemetry.encoder_queue_depth,
        maximum_encoder_queue_depth: telemetry.maximum_encoder_queue_depth,
        encoder_queue_capacity: telemetry.encoder_queue_capacity,
        encoder_queue_full_events: telemetry.encoder_queue_full_events,
        deliberately_dropped_frames: telemetry.deliberately_dropped_frames,
        frame_pool_creation_method: "CreateFreeThreaded".to_string(),
        frame_pool_buffer_count: WGC_FRAME_POOL_BUFFER_COUNT,
        finalization_duration_ms: telemetry.finalization_duration_ms,
    }
}

fn start_baseline_capture<T>(target: T, flags: BaselineFlags) -> Result<(), String>
where
    T: TryInto<GraphicsCaptureItemType> + Send + 'static,
{
    let measurements = Arc::clone(&flags.measurements);
    let settings = Settings::new(
        target,
        CursorCaptureSettings::Default,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags,
    )
    .frame_pool_buffer_count(WGC_FRAME_POOL_BUFFER_COUNT);
    let control =
        ContinuousBaselineHandler::start_free_threaded(settings).map_err(map_capture_error)?;
    let capture_started = lock_measurements(&measurements).capture_started;

    loop {
        if control.is_finished() {
            return control.wait().map_err(|error| {
                format!("Continuous baseline capture ended unexpectedly: {error}")
            });
        }
        if capture_started.elapsed() >= BASELINE_DURATION {
            lock_measurements(&measurements).capture_stopped = Some(Instant::now());
            return control.stop().map_err(|error| {
                format!("Continuous baseline capture could not stop cleanly: {error}")
            });
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn map_capture_error(error: GraphicsCaptureApiError<BaselineHandlerError>) -> String {
    match error {
        GraphicsCaptureApiError::NewHandlerError(error)
        | GraphicsCaptureApiError::FrameHandlerError(error) => error.to_string(),
        error => format!("Continuous baseline capture failed: {error}"),
    }
}

fn lock_measurements(
    measurements: &Mutex<BaselineMeasurements>,
) -> std::sync::MutexGuard<'_, BaselineMeasurements> {
    measurements
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn even_dimension(value: u32) -> Result<u32, String> {
    if value == 0 {
        return Err("The selected capture target has a zero-sized dimension.".to_string());
    }
    Ok(if value % 2 == 0 { value } else { value + 1 })
}

fn reserve_output_path(output_dir: &Path) -> Result<PathBuf, String> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("The system clock is invalid: {error}"))?
        .as_secs();
    for suffix in 0..1_000 {
        let suffix = if suffix == 0 {
            String::new()
        } else {
            format!("-{suffix}")
        };
        let path = output_dir.join(format!("continuous-baseline-{timestamp}{suffix}.mp4"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(_) => return Ok(path),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "Could not reserve baseline file '{}': {error}",
                    path.display()
                ))
            }
        }
    }
    Err("Could not reserve a unique continuous-baseline filename.".to_string())
}

#[cfg(test)]
mod tests {
    use super::BaselineMeasurements;
    use crate::capture::encoder::EncoderFrameTelemetry;
    use std::time::Duration;

    #[test]
    fn baseline_source_gap_telemetry_counts_missing_intervals() {
        let mut telemetry = BaselineMeasurements::new();
        telemetry.record_frame(
            1_000_000,
            60,
            Duration::from_micros(500),
            EncoderFrameTelemetry {
                queued: true,
                gpu_copy_duration: Some(Duration::from_micros(250)),
                queue_depth: 1,
                queue_capacity: 8,
            },
        );
        telemetry.record_frame(
            1_833_333,
            60,
            Duration::from_millis(40),
            EncoderFrameTelemetry {
                queued: false,
                gpu_copy_duration: None,
                queue_depth: 8,
                queue_capacity: 8,
            },
        );

        assert_eq!(telemetry.frames_observed, 2);
        assert_eq!(telemetry.consecutive_delta_count, 1);
        assert_eq!(telemetry.intervals_over_two_expected, 1);
        assert_eq!(telemetry.estimated_frames_missed, 4);
        assert_eq!(telemetry.send_frame_over_33_33_ms, 1);
        assert_eq!(telemetry.send_frame_over_50_ms, 0);
        assert_eq!(telemetry.owned_frame_copies, 1);
        assert_eq!(telemetry.maximum_encoder_queue_depth, 8);
        assert_eq!(telemetry.encoder_queue_full_events, 1);
        assert_eq!(telemetry.deliberately_dropped_frames, 1);
    }
}
