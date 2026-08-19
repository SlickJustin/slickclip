use std::cell::Cell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use windows_capture::capture::{Context, GraphicsCaptureApiError, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use crate::capture::capture_test::ensure_borderless_capture_access;
use crate::capture::continuous_baseline::is_continuous_baseline_active;
use crate::capture::encoder::{
    resolve_encoder, EncoderChoice, EncoderCodec, EncoderFrameTelemetry, VideoEncoderBackend,
    WindowsCaptureFileBackend,
};
use crate::capture::targets::{
    resolve_target, CaptureTargetRequest, NativeCaptureTarget, ResolvedCaptureTarget,
};
use crate::capture::WGC_FRAME_POOL_BUFFER_COUNT;

use super::audio::{
    AudioReplayConfiguration, AudioReplaySession, AudioReplayShared, AudioSnapshotPinGuard,
    AudioSnapshotPlan, ReplaySessionClock,
};
use super::segment::{CompletedSegment, SegmentRing, VideoFrameTimingPoint};
use super::state::{
    ReplayBufferStatus, ReplayCommandResult, ReplayLifecycleState, RotationLifecycleTrace,
};
use super::timeline::SavedReplayTimeline;

pub const SEGMENT_DURATION: Duration = Duration::from_secs(2);
const RECENT_SEGMENT_LIMIT: usize = 5;
// Development telemetry heuristic only; this is not a production failure policy.
const DIAGNOSTIC_MATERIAL_GAP_INTERVALS: f64 = 2.0;
const ALLOWED_REPLAY_DURATIONS: [u32; 5] = [30, 60, 120, 180, 300];
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

thread_local! {
    static IN_REPLAY_FRAME_CALLBACK: Cell<bool> = const { Cell::new(false) };
}

#[derive(Default)]
struct AtomicDurationStats {
    count: AtomicU64,
    total_ns: AtomicU64,
    worst_ns: AtomicU64,
}

impl AtomicDurationStats {
    fn record(&self, duration: Duration) {
        let nanos = duration.as_nanos().min(u128::from(u64::MAX)) as u64;
        self.count.fetch_add(1, Ordering::Relaxed);
        self.total_ns.fetch_add(nanos, Ordering::Relaxed);
        self.worst_ns.fetch_max(nanos, Ordering::Relaxed);
    }

    fn reset(&self) {
        self.count.store(0, Ordering::Relaxed);
        self.total_ns.store(0, Ordering::Relaxed);
        self.worst_ns.store(0, Ordering::Relaxed);
    }

    fn snapshot(&self) -> (Option<f64>, Option<f64>) {
        let count = self.count.load(Ordering::Relaxed);
        if count == 0 {
            return (None, None);
        }
        let total_ns = self.total_ns.load(Ordering::Relaxed);
        let worst_ns = self.worst_ns.load(Ordering::Relaxed);
        (
            Some(total_ns as f64 / count as f64 / 1_000_000.0),
            Some(worst_ns as f64 / 1_000_000.0),
        )
    }
}

#[derive(Default)]
struct ReplayCallbackTelemetry {
    callback: AtomicDurationStats,
    send_frame: AtomicDurationStats,
    lock_wait: AtomicDurationStats,
    rotation_evaluation: AtomicDurationStats,
    swap: AtomicDurationStats,
    state_update: AtomicDurationStats,
    filesystem: AtomicDurationStats,
    filesystem_operation_count: AtomicU64,
    send_over_16_67_ms: AtomicU64,
    send_over_33_33_ms: AtomicU64,
    send_over_50_ms: AtomicU64,
    send_over_100_ms: AtomicU64,
    owned_frame_copies: AtomicU64,
    gpu_copy: AtomicDurationStats,
    encoder_queue_depth: AtomicU64,
    maximum_encoder_queue_depth: AtomicU64,
    encoder_queue_capacity: AtomicU64,
    encoder_queue_full_events: AtomicU64,
    deliberately_dropped_frames: AtomicU64,
}

impl ReplayCallbackTelemetry {
    fn reset(&self) {
        self.callback.reset();
        self.send_frame.reset();
        self.lock_wait.reset();
        self.rotation_evaluation.reset();
        self.swap.reset();
        self.state_update.reset();
        self.filesystem.reset();
        self.filesystem_operation_count.store(0, Ordering::Relaxed);
        self.send_over_16_67_ms.store(0, Ordering::Relaxed);
        self.send_over_33_33_ms.store(0, Ordering::Relaxed);
        self.send_over_50_ms.store(0, Ordering::Relaxed);
        self.send_over_100_ms.store(0, Ordering::Relaxed);
        self.owned_frame_copies.store(0, Ordering::Relaxed);
        self.gpu_copy.reset();
        self.encoder_queue_depth.store(0, Ordering::Relaxed);
        self.maximum_encoder_queue_depth.store(0, Ordering::Relaxed);
        self.encoder_queue_capacity.store(0, Ordering::Relaxed);
        self.encoder_queue_full_events.store(0, Ordering::Relaxed);
        self.deliberately_dropped_frames.store(0, Ordering::Relaxed);
    }

    fn record_send_frame(&self, duration: Duration) {
        self.send_frame.record(duration);
        let duration_ms = duration.as_secs_f64() * 1_000.0;
        self.send_over_16_67_ms
            .fetch_add(u64::from(duration_ms > 16.67), Ordering::Relaxed);
        self.send_over_33_33_ms
            .fetch_add(u64::from(duration_ms > 33.33), Ordering::Relaxed);
        self.send_over_50_ms
            .fetch_add(u64::from(duration_ms > 50.0), Ordering::Relaxed);
        self.send_over_100_ms
            .fetch_add(u64::from(duration_ms > 100.0), Ordering::Relaxed);
    }

    fn record_encoder_frame(&self, telemetry: EncoderFrameTelemetry) {
        self.encoder_queue_depth
            .store(telemetry.queue_depth, Ordering::Relaxed);
        self.maximum_encoder_queue_depth
            .fetch_max(telemetry.queue_depth, Ordering::Relaxed);
        self.encoder_queue_capacity
            .store(telemetry.queue_capacity as u64, Ordering::Relaxed);
        if telemetry.queued {
            if let Some(copy_duration) = telemetry.gpu_copy_duration {
                self.owned_frame_copies.fetch_add(1, Ordering::Relaxed);
                self.gpu_copy.record(copy_duration);
            }
        } else {
            self.encoder_queue_full_events
                .fetch_add(1, Ordering::Relaxed);
            self.deliberately_dropped_frames
                .fetch_add(1, Ordering::Relaxed);
        }
    }
}

#[derive(Default)]
struct CallbackPhaseDurations {
    rotation_evaluation: Duration,
    swap: Duration,
    state_update: Duration,
    filesystem: Duration,
    filesystem_operation_count: u64,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayBufferStartRequest {
    pub target: CaptureTargetRequest,
    pub encoder: EncoderChoice,
    pub replay_duration_seconds: u32,
    pub frame_rate: u32,
    #[serde(default)]
    pub audio: AudioReplayConfiguration,
}

pub struct ReplaySaveSnapshot {
    pub save_request_timestamp_ms: u64,
    pub requested_duration_seconds: u32,
    pub segments: Vec<CompletedSegment>,
    pub video_timeline: SavedReplayTimeline,
    pub audio_snapshot_plans: Vec<AudioSnapshotPlan>,
    _pins: SegmentPinGuard,
    _audio_pins: AudioSnapshotPinGuard,
}

struct SegmentPinGuard {
    shared: Arc<SharedReplay>,
    sequence_numbers: Vec<u64>,
}

impl Drop for SegmentPinGuard {
    fn drop(&mut self) {
        self.shared.release_pins(&self.sequence_numbers);
    }
}

struct ReplayInner {
    state: ReplayLifecycleState,
    error_message: Option<String>,
    target_id: Option<String>,
    target_label: Option<String>,
    requested_encoder: Option<String>,
    actual_encoder: Option<String>,
    replay_duration_seconds: u32,
    frame_rate: u32,
    width: u32,
    height: u32,
    session_id: Option<String>,
    session_directory: Option<PathBuf>,
    ring: SegmentRing,
    pending_finalizations: usize,
    dropped_segments: u64,
    last_segment_duration_ms: Option<u64>,
    last_rotation_gap_ms: Option<f64>,
    last_finalize_time_ms: Option<f64>,
    last_source_frame_gap_ms: Option<f64>,
    worst_source_frame_gap_ms: Option<f64>,
    total_source_frame_gap_ms: f64,
    last_encoder_creation_ms: Option<f64>,
    worst_encoder_creation_ms: Option<f64>,
    total_encoder_creation_ms: f64,
    rotation_count: u64,
    last_estimated_frames_missed: Option<u64>,
    estimated_frames_missed_total: u64,
    material_source_gap_count: u64,
    encoder_preparation_in_flight: bool,
    prepared_encoder_ready: bool,
    next_encoder_state: String,
    rotation_lifecycle: RotationLifecycleTrace,
    pins: HashMap<u64, usize>,
    deferred_deletions: HashMap<u64, PathBuf>,
    next_rotation_request_id: u64,
    pending_rotation_request_id: Option<u64>,
    acknowledged_rotation: Option<(u64, u64)>,
}

impl ReplayInner {
    fn stopped() -> Self {
        Self {
            state: ReplayLifecycleState::Stopped,
            error_message: None,
            target_id: None,
            target_label: None,
            requested_encoder: None,
            actual_encoder: None,
            replay_duration_seconds: 0,
            frame_rate: 0,
            width: 0,
            height: 0,
            session_id: None,
            session_directory: None,
            ring: SegmentRing::new(0),
            pending_finalizations: 0,
            dropped_segments: 0,
            last_segment_duration_ms: None,
            last_rotation_gap_ms: None,
            last_finalize_time_ms: None,
            last_source_frame_gap_ms: None,
            worst_source_frame_gap_ms: None,
            total_source_frame_gap_ms: 0.0,
            last_encoder_creation_ms: None,
            worst_encoder_creation_ms: None,
            total_encoder_creation_ms: 0.0,
            rotation_count: 0,
            last_estimated_frames_missed: None,
            estimated_frames_missed_total: 0,
            material_source_gap_count: 0,
            encoder_preparation_in_flight: false,
            prepared_encoder_ready: false,
            next_encoder_state: "not_active".to_string(),
            rotation_lifecycle: RotationLifecycleTrace::default(),
            pins: HashMap::new(),
            deferred_deletions: HashMap::new(),
            next_rotation_request_id: 0,
            pending_rotation_request_id: None,
            acknowledged_rotation: None,
        }
    }

    fn snapshot(
        &self,
        frames_observed: u64,
        callback_telemetry: &ReplayCallbackTelemetry,
        audio: super::audio::AudioReplayStatus,
    ) -> ReplayBufferStatus {
        let (average_callback_duration_ms, worst_callback_duration_ms) =
            callback_telemetry.callback.snapshot();
        let (average_send_frame_duration_ms, worst_send_frame_duration_ms) =
            callback_telemetry.send_frame.snapshot();
        let (average_callback_lock_wait_ms, worst_callback_lock_wait_ms) =
            callback_telemetry.lock_wait.snapshot();
        let (average_rotation_evaluation_ms, worst_rotation_evaluation_ms) =
            callback_telemetry.rotation_evaluation.snapshot();
        let (average_swap_duration_ms, worst_swap_duration_ms) = callback_telemetry.swap.snapshot();
        let (average_callback_state_update_ms, worst_callback_state_update_ms) =
            callback_telemetry.state_update.snapshot();
        let (average_callback_filesystem_ms, worst_callback_filesystem_ms) =
            callback_telemetry.filesystem.snapshot();
        let (average_gpu_copy_duration_ms, worst_gpu_copy_duration_ms) =
            callback_telemetry.gpu_copy.snapshot();
        ReplayBufferStatus {
            state: self.state,
            error_message: self.error_message.clone(),
            target_id: self.target_id.clone(),
            target_label: self.target_label.clone(),
            requested_encoder: self.requested_encoder.clone(),
            actual_encoder: self.actual_encoder.clone(),
            replay_duration_seconds: self.replay_duration_seconds,
            expected_segment_duration_seconds: SEGMENT_DURATION.as_secs_f64(),
            frame_rate: self.frame_rate,
            width: self.width,
            height: self.height,
            session_id: self.session_id.clone(),
            session_directory: self
                .session_directory
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            completed_segment_count: self.ring.len(),
            retained_duration_seconds: self.ring.total_duration_ms() as f64 / 1_000.0,
            retained_bytes: self.ring.total_bytes(),
            pending_finalizations: self.pending_finalizations,
            dropped_segments: self.dropped_segments,
            last_segment_duration_seconds: self
                .last_segment_duration_ms
                .map(|duration| duration as f64 / 1_000.0),
            last_rotation_gap_ms: self.last_rotation_gap_ms,
            last_finalize_time_ms: self.last_finalize_time_ms,
            normal_frame_interval_ms: (self.frame_rate > 0)
                .then(|| 1_000.0 / f64::from(self.frame_rate)),
            last_source_frame_gap_ms: self.last_source_frame_gap_ms,
            worst_source_frame_gap_ms: self.worst_source_frame_gap_ms,
            average_source_frame_gap_ms: (self.rotation_count > 0)
                .then(|| self.total_source_frame_gap_ms / self.rotation_count as f64),
            last_encoder_creation_ms: self.last_encoder_creation_ms,
            worst_encoder_creation_ms: self.worst_encoder_creation_ms,
            average_encoder_creation_ms: (self.rotation_count > 0)
                .then(|| self.total_encoder_creation_ms / self.rotation_count as f64),
            rotation_count: self.rotation_count,
            frames_observed,
            last_estimated_frames_missed: self.last_estimated_frames_missed,
            estimated_frames_missed_total: self.estimated_frames_missed_total,
            material_source_gap_count: self.material_source_gap_count,
            encoder_preparation_in_flight: self.encoder_preparation_in_flight,
            prepared_encoder_ready: self.prepared_encoder_ready,
            next_encoder_state: self.next_encoder_state.clone(),
            average_callback_duration_ms,
            worst_callback_duration_ms,
            average_send_frame_duration_ms,
            worst_send_frame_duration_ms,
            send_frame_over_16_67_ms: callback_telemetry
                .send_over_16_67_ms
                .load(Ordering::Relaxed),
            send_frame_over_33_33_ms: callback_telemetry
                .send_over_33_33_ms
                .load(Ordering::Relaxed),
            send_frame_over_50_ms: callback_telemetry.send_over_50_ms.load(Ordering::Relaxed),
            send_frame_over_100_ms: callback_telemetry.send_over_100_ms.load(Ordering::Relaxed),
            average_callback_lock_wait_ms,
            worst_callback_lock_wait_ms,
            average_rotation_evaluation_ms,
            worst_rotation_evaluation_ms,
            average_swap_duration_ms,
            worst_swap_duration_ms,
            average_callback_state_update_ms,
            worst_callback_state_update_ms,
            average_callback_filesystem_ms,
            worst_callback_filesystem_ms,
            callback_filesystem_operation_count: callback_telemetry
                .filesystem_operation_count
                .load(Ordering::Relaxed),
            owned_frame_copies: callback_telemetry
                .owned_frame_copies
                .load(Ordering::Relaxed),
            average_gpu_copy_duration_ms,
            worst_gpu_copy_duration_ms,
            encoder_queue_depth: callback_telemetry
                .encoder_queue_depth
                .load(Ordering::Relaxed),
            maximum_encoder_queue_depth: callback_telemetry
                .maximum_encoder_queue_depth
                .load(Ordering::Relaxed),
            encoder_queue_capacity: callback_telemetry
                .encoder_queue_capacity
                .load(Ordering::Relaxed),
            encoder_queue_full_events: callback_telemetry
                .encoder_queue_full_events
                .load(Ordering::Relaxed),
            deliberately_dropped_frames: callback_telemetry
                .deliberately_dropped_frames
                .load(Ordering::Relaxed),
            frame_pool_creation_method: "CreateFreeThreaded".to_string(),
            frame_pool_buffer_count: WGC_FRAME_POOL_BUFFER_COUNT,
            rotation_lifecycle: self.rotation_lifecycle.clone(),
            recent_segments: self.ring.recent(RECENT_SEGMENT_LIMIT),
            audio,
        }
    }
}

struct SharedReplay {
    inner: Mutex<ReplayInner>,
    changed: Condvar,
    stop_requested: AtomicBool,
    frames_observed: AtomicU64,
    callback_telemetry: ReplayCallbackTelemetry,
    audio: Arc<AudioReplayShared>,
}

impl SharedReplay {
    fn new() -> Self {
        Self {
            inner: Mutex::new(ReplayInner::stopped()),
            changed: Condvar::new(),
            stop_requested: AtomicBool::new(false),
            frames_observed: AtomicU64::new(0),
            callback_telemetry: ReplayCallbackTelemetry::default(),
            audio: Arc::new(AudioReplayShared::new()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ReplayInner> {
        let started = Instant::now();
        let guard = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        IN_REPLAY_FRAME_CALLBACK.with(|in_callback| {
            if in_callback.get() {
                self.callback_telemetry.lock_wait.record(started.elapsed());
            }
        });
        guard
    }

    fn snapshot(&self) -> ReplayBufferStatus {
        let audio = self.audio.snapshot();
        self.lock().snapshot(
            self.frames_observed.load(Ordering::Relaxed),
            &self.callback_telemetry,
            audio,
        )
    }

    fn begin(&self, request: &ReplayBufferStartRequest) {
        self.audio.reset();
        self.stop_requested.store(false, Ordering::Release);
        self.frames_observed.store(0, Ordering::Relaxed);
        self.callback_telemetry.reset();
        let mut inner = self.lock();
        *inner = ReplayInner {
            state: ReplayLifecycleState::Starting,
            error_message: None,
            target_id: Some(request.target.id.clone()),
            target_label: None,
            requested_encoder: Some(request.encoder.result_name().to_string()),
            actual_encoder: None,
            replay_duration_seconds: request.replay_duration_seconds,
            frame_rate: request.frame_rate,
            width: 0,
            height: 0,
            session_id: None,
            session_directory: None,
            ring: SegmentRing::new(request.replay_duration_seconds),
            pending_finalizations: 0,
            dropped_segments: 0,
            last_segment_duration_ms: None,
            last_rotation_gap_ms: None,
            last_finalize_time_ms: None,
            last_source_frame_gap_ms: None,
            worst_source_frame_gap_ms: None,
            total_source_frame_gap_ms: 0.0,
            last_encoder_creation_ms: None,
            worst_encoder_creation_ms: None,
            total_encoder_creation_ms: 0.0,
            rotation_count: 0,
            last_estimated_frames_missed: None,
            estimated_frames_missed_total: 0,
            material_source_gap_count: 0,
            encoder_preparation_in_flight: false,
            prepared_encoder_ready: false,
            next_encoder_state: "starting".to_string(),
            rotation_lifecycle: RotationLifecycleTrace::default(),
            pins: HashMap::new(),
            deferred_deletions: HashMap::new(),
            next_rotation_request_id: 0,
            pending_rotation_request_id: None,
            acknowledged_rotation: None,
        };
        self.changed.notify_all();
    }

    fn configure(
        &self,
        target_label: String,
        actual_encoder: EncoderCodec,
        width: u32,
        height: u32,
        session_id: String,
        session_directory: PathBuf,
    ) {
        let mut inner = self.lock();
        inner.target_label = Some(target_label);
        inner.actual_encoder = Some(actual_encoder.display_name().to_string());
        inner.width = width;
        inner.height = height;
        inner.session_id = Some(session_id);
        inner.session_directory = Some(session_directory);
        self.changed.notify_all();
    }

    fn mark_running(&self) {
        let mut inner = self.lock();
        if inner.state != ReplayLifecycleState::Error {
            inner.state = ReplayLifecycleState::Running;
            inner.next_encoder_state = "waiting_for_prewarm_point".to_string();
        }
        self.changed.notify_all();
    }

    fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
        let mut inner = self.lock();
        if matches!(
            inner.state,
            ReplayLifecycleState::Starting | ReplayLifecycleState::Running
        ) {
            inner.state = ReplayLifecycleState::Stopping;
            inner.next_encoder_state = "stopping".to_string();
        }
        self.changed.notify_all();
    }

    fn should_stop(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }

    fn mark_stopped(&self) {
        let mut inner = self.lock();
        if inner.state != ReplayLifecycleState::Error {
            inner.state = ReplayLifecycleState::Stopped;
            inner.error_message = None;
        }
        inner.next_encoder_state = "not_active".to_string();
        self.changed.notify_all();
    }

    fn mark_error(&self, error: impl Into<String>) {
        self.stop_requested.store(true, Ordering::Release);
        let mut inner = self.lock();
        inner.state = ReplayLifecycleState::Error;
        inner.error_message = Some(error.into());
        inner.next_encoder_state = "error".to_string();
        self.changed.notify_all();
    }

    fn frame_observed(&self) {
        self.frames_observed.fetch_add(1, Ordering::Relaxed);
    }

    fn set_encoder_preparation_state(&self, in_flight: bool, ready: bool) {
        let mut inner = self.lock();
        inner.encoder_preparation_in_flight = in_flight;
        inner.prepared_encoder_ready = ready;
        inner.next_encoder_state = if in_flight {
            "preparing"
        } else if ready {
            "ready"
        } else if inner.state == ReplayLifecycleState::Running {
            "waiting_for_prewarm_point"
        } else {
            "not_active"
        }
        .to_string();
        self.changed.notify_all();
    }

    fn set_rotation_due_waiting(&self) {
        self.lock().next_encoder_state = "rotation_due_waiting_for_encoder".to_string();
        self.changed.notify_all();
    }

    fn publish_rotation_lifecycle(&self, trace: &RotationLifecycleTrace) {
        self.lock().rotation_lifecycle = trace.clone();
        self.changed.notify_all();
    }

    fn record_rotation(&self, diagnostics: RotationDiagnostics) {
        let mut inner = self.lock();
        inner.last_rotation_gap_ms = Some(diagnostics.encoder_creation_ms);
        inner.last_source_frame_gap_ms = Some(diagnostics.source_frame_gap_ms);
        inner.worst_source_frame_gap_ms = Some(
            inner
                .worst_source_frame_gap_ms
                .unwrap_or(0.0)
                .max(diagnostics.source_frame_gap_ms),
        );
        inner.total_source_frame_gap_ms += diagnostics.source_frame_gap_ms;
        inner.last_encoder_creation_ms = Some(diagnostics.encoder_creation_ms);
        inner.worst_encoder_creation_ms = Some(
            inner
                .worst_encoder_creation_ms
                .unwrap_or(0.0)
                .max(diagnostics.encoder_creation_ms),
        );
        inner.total_encoder_creation_ms += diagnostics.encoder_creation_ms;
        inner.rotation_count += 1;
        inner.last_estimated_frames_missed = Some(diagnostics.estimated_frames_missed);
        inner.estimated_frames_missed_total = inner
            .estimated_frames_missed_total
            .saturating_add(diagnostics.estimated_frames_missed);
        if diagnostics.material_source_gap {
            inner.material_source_gap_count += 1;
        }
        inner.encoder_preparation_in_flight = false;
        inner.prepared_encoder_ready = false;
        inner.next_encoder_state = "waiting_for_prewarm_point".to_string();
        self.changed.notify_all();
    }

    fn record_callback_phases(&self, callback_duration: Duration, phases: CallbackPhaseDurations) {
        self.callback_telemetry.callback.record(callback_duration);
        self.callback_telemetry
            .rotation_evaluation
            .record(phases.rotation_evaluation);
        self.callback_telemetry.swap.record(phases.swap);
        self.callback_telemetry
            .state_update
            .record(phases.state_update);
        self.callback_telemetry.filesystem.record(phases.filesystem);
        self.callback_telemetry
            .filesystem_operation_count
            .fetch_add(phases.filesystem_operation_count, Ordering::Relaxed);
    }

    fn segment_submitted(&self) {
        self.lock().pending_finalizations += 1;
        self.changed.notify_all();
    }

    fn complete_segment(&self, segment: CompletedSegment) {
        let (evicted, session_directory) = {
            let mut inner = self.lock();
            inner.pending_finalizations = inner.pending_finalizations.saturating_sub(1);
            inner.last_segment_duration_ms = Some(segment.actual_duration_ms);
            inner.last_finalize_time_ms = Some(segment.finalization_time_ms);
            let evicted = inner
                .ring
                .push(segment)
                .into_iter()
                .filter_map(|segment| {
                    if inner.pins.contains_key(&segment.sequence_number) {
                        inner
                            .deferred_deletions
                            .insert(segment.sequence_number, PathBuf::from(&segment.file_path));
                        None
                    } else {
                        Some(PathBuf::from(segment.file_path))
                    }
                })
                .collect::<Vec<_>>();
            (evicted, inner.session_directory.clone())
        };
        self.changed.notify_all();

        for path in evicted {
            if !path_is_inside_session(&path, session_directory.as_deref()) {
                self.mark_error(format!(
                    "Replay retention refused to delete a segment outside the active session: '{}'",
                    path.display()
                ));
                return;
            }
            if let Err(error) = fs::remove_file(&path) {
                self.mark_error(format!(
                    "Could not evict expired replay segment '{}': {error}",
                    path.display()
                ));
                return;
            }
        }
    }

    fn fail_segment(&self, path: &Path, error: impl Into<String>) {
        {
            let mut inner = self.lock();
            inner.pending_finalizations = inner.pending_finalizations.saturating_sub(1);
            inner.dropped_segments += 1;
        }
        let _ = fs::remove_file(path);
        self.mark_error(error);
        self.changed.notify_all();
    }

    fn discard_empty_segment(&self, path: &Path) {
        {
            let mut inner = self.lock();
            inner.pending_finalizations = inner.pending_finalizations.saturating_sub(1);
            inner.dropped_segments += 1;
        }
        let _ = fs::remove_file(path);
        self.changed.notify_all();
    }

    fn request_save_rotation(&self) -> Result<(u64, u64), String> {
        let mut inner = self.lock();
        if inner.state != ReplayLifecycleState::Running {
            return Err("Save Replay requires a running replay buffer.".to_string());
        }
        if inner.ring.len() == 0 {
            return Err("No finalized replay video is available yet.".to_string());
        }

        inner.next_rotation_request_id = inner.next_rotation_request_id.saturating_add(1);
        let request_id = inner.next_rotation_request_id;
        inner.pending_rotation_request_id = Some(request_id);
        inner.acknowledged_rotation = None;
        let requested_at = unix_timestamp_ms();
        self.changed.notify_all();
        Ok((request_id, requested_at))
    }

    fn pending_rotation_request(&self) -> Option<u64> {
        self.lock().pending_rotation_request_id
    }

    fn acknowledge_rotation(&self, request_id: u64, sequence_number: u64) {
        let mut inner = self.lock();
        if inner.pending_rotation_request_id == Some(request_id) {
            inner.pending_rotation_request_id = None;
            inner.acknowledged_rotation = Some((request_id, sequence_number));
        }
        self.changed.notify_all();
    }

    fn wait_for_save_boundary(&self, request_id: u64, timeout: Duration) -> Result<u64, String> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.lock();

        loop {
            if let Some((acknowledged_id, sequence_number)) = inner.acknowledged_rotation {
                if acknowledged_id == request_id && inner.ring.contains_sequence(sequence_number) {
                    return Ok(sequence_number);
                }
            }
            if inner.state == ReplayLifecycleState::Error {
                if inner.pending_rotation_request_id == Some(request_id) {
                    inner.pending_rotation_request_id = None;
                }
                return Err(inner.error_message.clone().unwrap_or_else(|| {
                    "The replay buffer failed while finalizing the save boundary.".to_string()
                }));
            }
            if inner.state != ReplayLifecycleState::Running {
                if inner.pending_rotation_request_id == Some(request_id) {
                    inner.pending_rotation_request_id = None;
                }
                return Err(format!(
                    "The replay buffer entered {:?} before the save boundary finalized.",
                    inner.state
                ));
            }

            let now = Instant::now();
            if now >= deadline {
                if inner.pending_rotation_request_id == Some(request_id) {
                    inner.pending_rotation_request_id = None;
                }
                return Err(
                    "Timed out waiting for the current replay segment to finalize.".to_string(),
                );
            }
            let remaining = deadline.saturating_duration_since(now);
            let (next_inner, _) = self
                .changed
                .wait_timeout(inner, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner = next_inner;
        }
    }

    fn pin_snapshot(
        self: &Arc<Self>,
        final_sequence_number: u64,
        save_request_timestamp_ms: u64,
    ) -> Result<ReplaySaveSnapshot, String> {
        let (segments, requested_duration_seconds, sequence_numbers) = {
            let mut inner = self.lock();
            let requested_duration_seconds = inner.replay_duration_seconds;
            let segments = inner.ring.select_suffix_through(
                final_sequence_number,
                u64::from(requested_duration_seconds) * 1_000,
            );
            if segments.is_empty() {
                return Err("No finalized replay segments were available to save.".to_string());
            }

            let sequence_numbers = segments
                .iter()
                .map(|segment| segment.sequence_number)
                .collect::<Vec<_>>();
            for sequence_number in &sequence_numbers {
                *inner.pins.entry(*sequence_number).or_insert(0) += 1;
            }
            (segments, requested_duration_seconds, sequence_numbers)
        };

        let video_timeline = SavedReplayTimeline::from_segments(&segments)?;
        debug_assert_eq!(
            video_timeline.video_pts_to_clip_100ns(segments[0].sequence_number, 0),
            Some(0)
        );
        let (audio_snapshot_plans, audio_pins) = self.audio.plan_and_pin(&video_timeline);

        Ok(ReplaySaveSnapshot {
            save_request_timestamp_ms,
            requested_duration_seconds,
            segments,
            video_timeline,
            audio_snapshot_plans,
            _pins: SegmentPinGuard {
                shared: Arc::clone(self),
                sequence_numbers,
            },
            _audio_pins: audio_pins,
        })
    }

    fn release_pins(&self, sequence_numbers: &[u64]) {
        let (paths, session_directory) = {
            let mut inner = self.lock();
            let mut paths = Vec::new();
            for sequence_number in sequence_numbers {
                let remove_pin = match inner.pins.get_mut(sequence_number) {
                    Some(count) if *count > 1 => {
                        *count -= 1;
                        false
                    }
                    Some(_) => true,
                    None => false,
                };
                if remove_pin {
                    inner.pins.remove(sequence_number);
                    if let Some(path) = inner.deferred_deletions.remove(sequence_number) {
                        paths.push(path);
                    }
                }
            }
            (paths, inner.session_directory.clone())
        };

        for path in paths {
            if path_is_inside_session(&path, session_directory.as_deref()) {
                let _ = fs::remove_file(path);
            }
        }
        self.changed.notify_all();
    }

    fn has_pins(&self) -> bool {
        !self.lock().pins.is_empty()
    }
}

#[derive(Clone)]
pub struct ReplayBufferManager {
    shared: Arc<SharedReplay>,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
    root: Arc<PathBuf>,
}

impl ReplayBufferManager {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|error| {
            format!(
                "Could not create the replay-buffer root '{}': {error}",
                root.display()
            )
        })?;
        cleanup_session_directories(&root)?;

        Ok(Self {
            shared: Arc::new(SharedReplay::new()),
            worker: Arc::new(Mutex::new(None)),
            root: Arc::new(root),
        })
    }

    pub fn status(&self) -> ReplayBufferStatus {
        self.shared.snapshot()
    }

    pub fn start(&self, request: ReplayBufferStartRequest) -> ReplayCommandResult {
        if is_continuous_baseline_active() {
            return ReplayCommandResult::failure(
                self.status(),
                "Wait for the continuous-capture baseline to finish before starting the Replay Buffer.",
            );
        }
        if let Err(error) = validate_start_request(&request) {
            return ReplayCommandResult::failure(self.status(), error);
        }

        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(finished) = worker.take() {
                let _ = finished.join();
            }
        }

        let current = self.status();
        if !current.state.can_start() || worker.is_some() {
            return ReplayCommandResult::failure(
                current,
                "A replay-buffer session is already starting, running, or stopping.",
            );
        }
        if self.shared.has_pins() {
            return ReplayCommandResult::failure(
                current,
                "A replay is still being saved. Wait for it to finish before starting a new buffer session.",
            );
        }

        if let Err(error) = cleanup_session_directories(&self.root) {
            return ReplayCommandResult::failure(self.status(), error);
        }

        self.shared.begin(&request);
        let shared = Arc::clone(&self.shared);
        let root = Arc::clone(&self.root);
        let thread = match thread::Builder::new()
            .name("justin-replay-buffer".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_replay_session(Arc::clone(&shared), root.as_ref(), request)
                }));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        if shared.snapshot().state != ReplayLifecycleState::Error {
                            shared.mark_error(error);
                        }
                    }
                    Err(_) => shared.mark_error("The replay-buffer capture worker panicked."),
                }
            }) {
            Ok(thread) => thread,
            Err(error) => {
                self.shared.mark_error(format!(
                    "Could not start the replay-buffer capture thread: {error}"
                ));
                return ReplayCommandResult::failure(self.status(), error.to_string());
            }
        };
        *worker = Some(thread);

        ReplayCommandResult::success(self.status())
    }

    pub fn stop_and_wait(&self) -> ReplayCommandResult {
        let status = self.status();
        if !status.state.is_active() {
            return if status.state == ReplayLifecycleState::Stopped {
                ReplayCommandResult::success(status)
            } else {
                ReplayCommandResult::failure(status, "The replay buffer is not currently running.")
            };
        }

        self.shared.request_stop();
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            if worker.join().is_err() {
                self.shared
                    .mark_error("The replay-buffer capture thread panicked while stopping.");
            }
        }

        if self.status().state == ReplayLifecycleState::Stopped && !self.shared.has_pins() {
            if let Err(error) = cleanup_session_directories(&self.root) {
                self.shared.mark_error(error);
            }
        }

        let status = self.status();
        if status.state == ReplayLifecycleState::Stopped {
            ReplayCommandResult::success(status)
        } else {
            ReplayCommandResult::failure(
                status.clone(),
                status
                    .error_message
                    .unwrap_or_else(|| "The replay buffer did not stop cleanly.".to_string()),
            )
        }
    }

    pub fn snapshot_for_save(&self) -> Result<ReplaySaveSnapshot, String> {
        let (request_id, requested_at) = self.shared.request_save_rotation()?;
        let final_sequence_number = self
            .shared
            .wait_for_save_boundary(request_id, Duration::from_secs(15))?;
        self.shared
            .pin_snapshot(final_sequence_number, requested_at)
    }

    pub fn shutdown_and_cleanup(&self) {
        self.shared.request_stop();
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            if worker.join().is_err() {
                self.shared
                    .mark_error("The replay-buffer capture thread panicked during app shutdown.");
            }
        }
        if let Err(error) = cleanup_session_directories(&self.root) {
            self.shared.mark_error(error);
        }
    }
}

fn validate_start_request(request: &ReplayBufferStartRequest) -> Result<(), String> {
    request.audio.validate()?;
    if !ALLOWED_REPLAY_DURATIONS.contains(&request.replay_duration_seconds) {
        return Err(format!(
            "Replay duration must be one of 30, 60, 120, 180, or 300 seconds; received {}.",
            request.replay_duration_seconds
        ));
    }
    if !matches!(request.frame_rate, 30 | 60) {
        return Err(format!(
            "Replay frame rate must be 30 or 60 FPS; received {}.",
            request.frame_rate
        ));
    }
    if matches!(request.encoder, EncoderChoice::Av1) {
        return Err(
            "AV1 production encoding is not available through windows-capture 2.0.1. Choose Automatic, HEVC, or H.264."
                .to_string(),
        );
    }

    Ok(())
}

fn run_replay_session(
    shared: Arc<SharedReplay>,
    root: &Path,
    request: ReplayBufferStartRequest,
) -> Result<(), String> {
    let session_clock = ReplaySessionClock::create()?;
    let resolved_encoder = resolve_encoder(request.encoder)?;
    let ResolvedCaptureTarget {
        target,
        label,
        width,
        height,
    } = resolve_target(&request.target)?;
    let width = even_dimension(width)?;
    let height = even_dimension(height)?;
    let session_id = create_session_id();
    let session_directory = root.join(&session_id);
    fs::create_dir(&session_directory).map_err(|error| {
        format!(
            "Could not create replay session directory '{}': {error}",
            session_directory.display()
        )
    })?;

    shared.configure(
        label,
        resolved_encoder.actual,
        width,
        height,
        session_id,
        session_directory.clone(),
    );

    shared.audio.begin(
        &request.audio,
        session_clock.clone(),
        session_directory.clone(),
        request.replay_duration_seconds,
    )?;
    let mut audio_session =
        AudioReplaySession::prepare(shared.audio.enabled_tracks(), session_clock.clone())?;
    audio_session.start()?;

    let flags = ReplayCaptureFlags {
        shared: Arc::clone(&shared),
        session_directory,
        codec: resolved_encoder.actual,
        width,
        height,
        frame_rate: request.frame_rate,
        session_started: session_clock.started,
    };
    let capture_result = match target {
        NativeCaptureTarget::Monitor(monitor) => start_target_capture(monitor, flags),
        NativeCaptureTarget::Window(window) => start_target_capture(window, flags),
    };
    audio_session.stop_and_wait();

    match capture_result {
        Ok(()) => {
            shared.mark_stopped();
            Ok(())
        }
        Err(error) => Err(error),
    }
}

#[derive(Clone)]
struct ReplayCaptureFlags {
    shared: Arc<SharedReplay>,
    session_directory: PathBuf,
    codec: EncoderCodec,
    width: u32,
    height: u32,
    frame_rate: u32,
    session_started: Instant,
}

struct ActiveSegment {
    sequence_number: u64,
    path: PathBuf,
    backend: Box<dyn VideoEncoderBackend>,
    first_frame_instant: Option<Instant>,
    first_frame_timestamp: Option<i64>,
    last_frame_timestamp: Option<i64>,
    start_timestamp_ms: Option<u64>,
    frame_count: u64,
    encoder_creation_time_ms: f64,
    encoder_creation_started_ms: f64,
    encoder_creation_completed_ms: f64,
    first_frame_submitted_ms: Option<f64>,
    last_frame_submitted_ms: Option<f64>,
    frame_timing_points: Vec<VideoFrameTimingPoint>,
}

struct FrameEncodeResult {
    send_duration: Duration,
    telemetry: EncoderFrameTelemetry,
}

impl ActiveSegment {
    fn create(
        flags: &ReplayCaptureFlags,
        sequence_number: u64,
        session_started: Instant,
    ) -> Result<Self, String> {
        let path = flags
            .session_directory
            .join(format!("segment-{sequence_number:06}.mp4"));
        let creation_started = Instant::now();
        let encoder_creation_started_ms = session_started.elapsed().as_secs_f64() * 1_000.0;
        let backend = WindowsCaptureFileBackend::create(
            &path,
            flags.codec,
            flags.width,
            flags.height,
            flags.frame_rate,
        )
        .map_err(|error| format!("Could not initialize replay segment encoder: {error}"))?;
        let encoder_creation_time_ms = creation_started.elapsed().as_secs_f64() * 1_000.0;
        let encoder_creation_completed_ms = session_started.elapsed().as_secs_f64() * 1_000.0;

        Ok(Self {
            sequence_number,
            path,
            backend,
            first_frame_instant: None,
            first_frame_timestamp: None,
            last_frame_timestamp: None,
            start_timestamp_ms: None,
            frame_count: 0,
            encoder_creation_time_ms,
            encoder_creation_started_ms,
            encoder_creation_completed_ms,
            first_frame_submitted_ms: None,
            last_frame_submitted_ms: None,
            frame_timing_points: Vec::new(),
        })
    }

    fn should_rotate(&self, now: Instant) -> bool {
        self.first_frame_instant
            .is_some_and(|started| now.duration_since(started) >= SEGMENT_DURATION)
    }

    fn has_frames(&self) -> bool {
        self.frame_count > 0
    }

    fn discard(self) {
        let path = self.path.clone();
        drop(self.backend);
        let _ = fs::remove_file(path);
    }

    fn should_prepare(&self, now: Instant) -> bool {
        self.first_frame_instant
            .is_some_and(|started| now.duration_since(started) >= SEGMENT_DURATION / 2)
    }

    fn encode_frame(
        &mut self,
        frame: &mut Frame<'_>,
        session_started: Instant,
        frame_rate: u32,
    ) -> Result<FrameEncodeResult, String> {
        let timestamp = frame
            .timestamp()
            .map_err(|error| format!("Could not read replay frame timestamp: {error}"))?
            .Duration;
        let send_started = Instant::now();
        let encoded = self
            .backend
            .encode_frame(frame)
            .map_err(|error| format!("Replay encoder rejected a captured frame: {error}"))?;
        let send_duration = send_started.elapsed();
        let submitted_ms = session_started.elapsed().as_secs_f64() * 1_000.0;

        if !encoded.telemetry.queued {
            return Ok(FrameEncodeResult {
                send_duration,
                telemetry: encoded.telemetry,
            });
        }

        if self.first_frame_instant.is_none() {
            self.first_frame_instant = Some(Instant::now());
            self.first_frame_timestamp = Some(timestamp);
            self.start_timestamp_ms = Some(unix_timestamp_ms());
            self.first_frame_submitted_ms = Some(submitted_ms);
        }
        self.last_frame_timestamp = Some(timestamp);
        self.last_frame_submitted_ms = Some(submitted_ms);
        let encoded_pts_100ns =
            ((i128::from(self.frame_count) * 10_000_000) / i128::from(frame_rate.max(1))) as i64;
        self.frame_timing_points.push(VideoFrameTimingPoint {
            frame_index: self.frame_count,
            source_qpc_100ns: timestamp,
            encoded_pts_100ns,
        });
        self.frame_count += 1;
        Ok(FrameEncodeResult {
            send_duration,
            telemetry: encoded.telemetry,
        })
    }

    fn into_finalize_job(
        self,
        flags: &ReplayCaptureFlags,
        boundary: Option<SegmentBoundaryTiming>,
    ) -> FinalizeJob {
        let actual_duration_100ns = ((i128::from(self.frame_count) * 10_000_000)
            / i128::from(flags.frame_rate.max(1))) as i64;
        let actual_duration_ms = u64::try_from(actual_duration_100ns.max(0) / 10_000)
            .unwrap_or(0)
            .max(1);
        let start_timestamp_ms = self.start_timestamp_ms.unwrap_or_else(unix_timestamp_ms);

        FinalizeJob {
            backend: self.backend,
            path: self.path,
            sequence_number: self.sequence_number,
            start_timestamp_ms,
            end_timestamp_ms: unix_timestamp_ms(),
            actual_duration_ms,
            encoded_start_pts_100ns: 0,
            encoded_last_frame_pts_100ns: self
                .frame_timing_points
                .last()
                .map(|point| point.encoded_pts_100ns)
                .unwrap_or(0),
            encoded_end_pts_100ns: actual_duration_100ns,
            encoded_duration_100ns: actual_duration_100ns,
            codec: flags.codec,
            width: flags.width,
            height: flags.height,
            frame_rate: flags.frame_rate,
            rotation_gap_ms: boundary
                .as_ref()
                .map(|timing| timing.encoder_creation_time_ms),
            frame_count: self.frame_count,
            first_frame_timestamp_100ns: self.first_frame_timestamp.unwrap_or(0),
            last_frame_timestamp_100ns: self.last_frame_timestamp.unwrap_or(0),
            next_segment_first_frame_timestamp_100ns: boundary
                .as_ref()
                .map(|timing| timing.next_first_frame_timestamp_100ns),
            source_frame_gap_ms: boundary.as_ref().map(|timing| timing.source_frame_gap_ms),
            encoder_creation_time_ms: self.encoder_creation_time_ms,
            encoder_creation_started_ms: self.encoder_creation_started_ms,
            encoder_creation_completed_ms: self.encoder_creation_completed_ms,
            rotation_requested_ms: boundary.as_ref().map(|timing| timing.rotation_requested_ms),
            first_frame_submitted_ms: self.first_frame_submitted_ms,
            last_frame_submitted_ms: self.last_frame_submitted_ms,
            next_first_frame_submitted_ms: boundary
                .as_ref()
                .map(|timing| timing.next_first_frame_submitted_ms),
            frame_timing_points: self.frame_timing_points,
        }
    }
}

struct SegmentBoundaryTiming {
    next_first_frame_timestamp_100ns: i64,
    source_frame_gap_ms: f64,
    encoder_creation_time_ms: f64,
    rotation_requested_ms: f64,
    next_first_frame_submitted_ms: f64,
}

struct FinalizeJob {
    backend: Box<dyn VideoEncoderBackend>,
    path: PathBuf,
    sequence_number: u64,
    start_timestamp_ms: u64,
    end_timestamp_ms: u64,
    actual_duration_ms: u64,
    encoded_start_pts_100ns: i64,
    encoded_last_frame_pts_100ns: i64,
    encoded_end_pts_100ns: i64,
    encoded_duration_100ns: i64,
    codec: EncoderCodec,
    width: u32,
    height: u32,
    frame_rate: u32,
    rotation_gap_ms: Option<f64>,
    frame_count: u64,
    first_frame_timestamp_100ns: i64,
    last_frame_timestamp_100ns: i64,
    next_segment_first_frame_timestamp_100ns: Option<i64>,
    source_frame_gap_ms: Option<f64>,
    encoder_creation_time_ms: f64,
    encoder_creation_started_ms: f64,
    encoder_creation_completed_ms: f64,
    rotation_requested_ms: Option<f64>,
    first_frame_submitted_ms: Option<f64>,
    last_frame_submitted_ms: Option<f64>,
    next_first_frame_submitted_ms: Option<f64>,
    frame_timing_points: Vec<VideoFrameTimingPoint>,
}

struct FinalizerWorker {
    sender: Option<mpsc::Sender<FinalizeJob>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<SharedReplay>,
}

impl FinalizerWorker {
    fn new(shared: Arc<SharedReplay>) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel::<FinalizeJob>();
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("justin-replay-finalizer".to_string())
            .spawn(move || finalize_segments(receiver, worker_shared))
            .map_err(|error| format!("Could not start the replay segment finalizer: {error}"))?;

        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
            shared,
        })
    }

    fn submit(&mut self, job: FinalizeJob) -> Result<(), String> {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| "The replay segment finalizer is already closed.".to_string())?;
        self.shared.segment_submitted();
        if let Err(error) = sender.send(job) {
            let path = error.0.path.clone();
            let message = format!("Could not queue replay segment finalization: {error}");
            self.shared.fail_segment(&path, message.clone());
            return Err(message);
        }

        Ok(())
    }

    fn shutdown(mut self) -> Result<(), String> {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| "The replay segment finalizer panicked.".to_string())?;
        }
        Ok(())
    }
}

impl Drop for FinalizerWorker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn finalize_segments(receiver: mpsc::Receiver<FinalizeJob>, shared: Arc<SharedReplay>) {
    for job in receiver {
        let finalization_started = Instant::now();
        let path = job.path.clone();
        let finish_result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.backend.finish()));
        let finish_result = match finish_result {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(_) => Err("The windows-capture encoder panicked while finalizing.".to_string()),
        };

        if job.frame_count == 0 {
            shared.discard_empty_segment(&path);
            continue;
        }
        if let Err(error) = finish_result {
            shared.fail_segment(
                &path,
                format!(
                    "Replay segment {} could not be finalized: {error}",
                    job.sequence_number
                ),
            );
            continue;
        }
        let file_size = match fs::metadata(&path) {
            Ok(metadata) if metadata.len() > 0 => metadata.len(),
            Ok(_) => {
                shared.fail_segment(
                    &path,
                    format!(
                        "Replay segment {} finalized as an empty file.",
                        job.sequence_number
                    ),
                );
                continue;
            }
            Err(error) => {
                shared.fail_segment(
                    &path,
                    format!(
                        "Replay segment {} could not be verified: {error}",
                        job.sequence_number
                    ),
                );
                continue;
            }
        };

        shared.complete_segment(CompletedSegment {
            sequence_number: job.sequence_number,
            file_path: path.to_string_lossy().into_owned(),
            start_timestamp_ms: job.start_timestamp_ms,
            end_timestamp_ms: job.end_timestamp_ms,
            actual_duration_ms: job.actual_duration_ms,
            first_frame_timestamp_100ns: job.first_frame_timestamp_100ns,
            last_frame_timestamp_100ns: job.last_frame_timestamp_100ns,
            encoded_start_pts_100ns: job.encoded_start_pts_100ns,
            encoded_last_frame_pts_100ns: job.encoded_last_frame_pts_100ns,
            encoded_end_pts_100ns: job.encoded_end_pts_100ns,
            encoded_duration_100ns: job.encoded_duration_100ns,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: 10_000_000,
            frame_timing_points: job.frame_timing_points,
            next_segment_first_frame_timestamp_100ns: job.next_segment_first_frame_timestamp_100ns,
            source_frame_gap_ms: job.source_frame_gap_ms,
            frame_count: job.frame_count,
            encoder_creation_time_ms: job.encoder_creation_time_ms,
            encoder_creation_started_ms: job.encoder_creation_started_ms,
            encoder_creation_completed_ms: job.encoder_creation_completed_ms,
            rotation_requested_ms: job.rotation_requested_ms,
            first_frame_submitted_ms: job.first_frame_submitted_ms,
            last_frame_submitted_ms: job.last_frame_submitted_ms,
            next_first_frame_submitted_ms: job.next_first_frame_submitted_ms,
            codec: job.codec.display_name().to_string(),
            width: job.width,
            height: job.height,
            frame_rate: job.frame_rate,
            file_size,
            finalized: true,
            finalization_time_ms: finalization_started.elapsed().as_secs_f64() * 1_000.0,
            rotation_gap_ms: job.rotation_gap_ms,
        });
    }
}

struct EncoderPrewarmer {
    sender: Option<mpsc::Sender<u64>>,
    receiver: mpsc::Receiver<Result<ActiveSegment, String>>,
    thread: Option<JoinHandle<()>>,
}

impl EncoderPrewarmer {
    fn new(flags: ReplayCaptureFlags, session_started: Instant) -> Result<Self, String> {
        let (request_sender, request_receiver) = mpsc::channel::<u64>();
        let (result_sender, result_receiver) = mpsc::channel::<Result<ActiveSegment, String>>();
        let thread = thread::Builder::new()
            .name("justin-replay-encoder-prewarm".to_string())
            .spawn(move || {
                for sequence_number in request_receiver {
                    let result = ActiveSegment::create(&flags, sequence_number, session_started);
                    if let Err(error) = result_sender.send(result) {
                        if let Ok(segment) = error.0 {
                            segment.discard();
                        }
                        break;
                    }
                }
            })
            .map_err(|error| format!("Could not start the encoder prewarm worker: {error}"))?;

        Ok(Self {
            sender: Some(request_sender),
            receiver: result_receiver,
            thread: Some(thread),
        })
    }

    fn request(&self, sequence_number: u64) -> Result<(), String> {
        self.sender
            .as_ref()
            .ok_or_else(|| "The encoder prewarm worker is closed.".to_string())?
            .send(sequence_number)
            .map_err(|error| format!("Could not request encoder prewarming: {error}"))
    }

    fn try_take(&self) -> Result<Option<ActiveSegment>, String> {
        match self.receiver.try_recv() {
            Ok(Ok(segment)) => Ok(Some(segment)),
            Ok(Err(error)) => Err(error),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("The encoder prewarm worker stopped unexpectedly.".to_string())
            }
        }
    }

    fn shutdown(mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        while let Ok(result) = self.receiver.try_recv() {
            if let Ok(segment) = result {
                segment.discard();
            }
        }
    }
}

struct RotationDiagnostics {
    source_frame_gap_ms: f64,
    encoder_creation_ms: f64,
    estimated_frames_missed: u64,
    material_source_gap: bool,
}

fn calculate_rotation_diagnostics(
    previous_timestamp_100ns: i64,
    next_timestamp_100ns: i64,
    frame_rate: u32,
    encoder_creation_ms: f64,
) -> RotationDiagnostics {
    let source_frame_gap_ms =
        (i128::from(next_timestamp_100ns) - i128::from(previous_timestamp_100ns)) as f64 / 10_000.0;
    let expected_frame_interval_ms = 1_000.0 / f64::from(frame_rate.max(1));
    let estimated_intervals = if source_frame_gap_ms > 0.0 {
        (source_frame_gap_ms / expected_frame_interval_ms).round() as u64
    } else {
        0
    };
    RotationDiagnostics {
        source_frame_gap_ms,
        encoder_creation_ms,
        estimated_frames_missed: estimated_intervals.saturating_sub(1),
        material_source_gap: source_frame_gap_ms
            > expected_frame_interval_ms * DIAGNOSTIC_MATERIAL_GAP_INTERVALS,
    }
}

struct ReplayCaptureHandler {
    flags: ReplayCaptureFlags,
    active: Option<ActiveSegment>,
    finalizer: Option<FinalizerWorker>,
    prewarmer: Option<EncoderPrewarmer>,
    prepared: Option<ActiveSegment>,
    preparation_in_flight: bool,
    next_sequence: u64,
    session_started: Instant,
    rotation_requested_ms: Option<f64>,
    rotation_lifecycle: RotationLifecycleTrace,
    awaiting_following_frame: bool,
    finished: bool,
}

impl ReplayCaptureHandler {
    fn elapsed_ms(&self) -> f64 {
        self.session_started.elapsed().as_secs_f64() * 1_000.0
    }

    fn publish_lifecycle(&self, phases: &mut CallbackPhaseDurations) {
        let started = Instant::now();
        self.flags
            .shared
            .publish_rotation_lifecycle(&self.rotation_lifecycle);
        phases.state_update += started.elapsed();
    }

    fn ensure_preparation(
        &mut self,
        phases: &mut CallbackPhaseDurations,
    ) -> Result<(), ReplayHandlerError> {
        if self.prepared.is_some() || self.preparation_in_flight {
            return Ok(());
        }
        let active_sequence_number = self.active.as_ref().map(|active| active.sequence_number);
        if self.rotation_lifecycle.active_sequence_number != active_sequence_number {
            self.rotation_lifecycle = RotationLifecycleTrace {
                active_sequence_number,
                active_segment_first_frame_ms: self
                    .active
                    .as_ref()
                    .and_then(|active| active.first_frame_submitted_ms),
                rotation_requested_ms: self.rotation_requested_ms,
                ..RotationLifecycleTrace::default()
            };
        }
        let requested_sequence = self.next_sequence;
        self.prewarmer
            .as_ref()
            .ok_or_else(|| ReplayHandlerError::new("The encoder prewarm worker is unavailable."))?
            .request(self.next_sequence)
            .map_err(ReplayHandlerError::new)?;
        self.next_sequence += 1;
        self.preparation_in_flight = true;
        self.rotation_lifecycle.next_sequence_number = Some(requested_sequence);
        self.rotation_lifecycle.prewarm_requested_ms = Some(self.elapsed_ms());
        let state_started = Instant::now();
        self.flags.shared.set_encoder_preparation_state(true, false);
        phases.state_update += state_started.elapsed();
        if self.rotation_requested_ms.is_some() {
            let state_started = Instant::now();
            self.flags.shared.set_rotation_due_waiting();
            phases.state_update += state_started.elapsed();
        }
        self.publish_lifecycle(phases);
        Ok(())
    }

    fn poll_preparation(
        &mut self,
        phases: &mut CallbackPhaseDurations,
    ) -> Result<(), ReplayHandlerError> {
        if !self.preparation_in_flight {
            return Ok(());
        }
        let prepared = self
            .prewarmer
            .as_ref()
            .ok_or_else(|| ReplayHandlerError::new("The encoder prewarm worker is unavailable."))?
            .try_take()
            .map_err(ReplayHandlerError::new)?;
        if let Some(prepared) = prepared {
            self.rotation_lifecycle.encoder_creation_started_ms =
                Some(prepared.encoder_creation_started_ms);
            self.rotation_lifecycle.encoder_creation_completed_ms =
                Some(prepared.encoder_creation_completed_ms);
            self.rotation_lifecycle.prepared_ready_ms = Some(self.elapsed_ms());
            self.prepared = Some(prepared);
            self.preparation_in_flight = false;
            let state_started = Instant::now();
            self.flags.shared.set_encoder_preparation_state(false, true);
            phases.state_update += state_started.elapsed();
            self.publish_lifecycle(phases);
        }
        Ok(())
    }

    fn mark_rotation_requested(&mut self, phases: &mut CallbackPhaseDurations) {
        if self.rotation_requested_ms.is_none() {
            let requested_ms = self.elapsed_ms();
            self.rotation_requested_ms = Some(requested_ms);
            self.rotation_lifecycle.rotation_requested_ms = Some(requested_ms);
            if self.prepared.is_none() {
                let state_started = Instant::now();
                self.flags.shared.set_rotation_due_waiting();
                phases.state_update += state_started.elapsed();
            }
            self.publish_lifecycle(phases);
        }
    }

    fn rotate_on_frame(
        &mut self,
        frame: &mut Frame<'_>,
        phases: &mut CallbackPhaseDurations,
    ) -> Result<Option<u64>, ReplayHandlerError> {
        self.rotation_lifecycle.swap_started_ms = Some(self.elapsed_ms());
        let mut next = self
            .prepared
            .take()
            .ok_or_else(|| ReplayHandlerError::new("The prepared replay encoder is missing."))?;
        let previous_last_timestamp = self
            .active
            .as_ref()
            .and_then(|segment| segment.last_frame_timestamp)
            .ok_or_else(|| ReplayHandlerError::new("The active replay segment has no frames."))?;
        let encoded = next
            .encode_frame(frame, self.session_started, self.flags.frame_rate)
            .map_err(ReplayHandlerError::new)?;
        self.flags
            .shared
            .callback_telemetry
            .record_send_frame(encoded.send_duration);
        self.flags
            .shared
            .callback_telemetry
            .record_encoder_frame(encoded.telemetry);
        if !encoded.telemetry.queued {
            self.prepared = Some(next);
            return Ok(None);
        }
        let next_first_timestamp = next.first_frame_timestamp.ok_or_else(|| {
            ReplayHandlerError::new("The prepared encoder did not accept its first frame.")
        })?;
        let next_first_submitted_ms = next.first_frame_submitted_ms.ok_or_else(|| {
            ReplayHandlerError::new("The prepared encoder has no first-frame submission time.")
        })?;
        let encoder_creation_time_ms = next.encoder_creation_time_ms;
        let diagnostics = calculate_rotation_diagnostics(
            previous_last_timestamp,
            next_first_timestamp,
            self.flags.frame_rate,
            encoder_creation_time_ms,
        );
        let boundary = SegmentBoundaryTiming {
            next_first_frame_timestamp_100ns: next_first_timestamp,
            source_frame_gap_ms: diagnostics.source_frame_gap_ms,
            encoder_creation_time_ms,
            rotation_requested_ms: self
                .rotation_requested_ms
                .unwrap_or_else(|| self.session_started.elapsed().as_secs_f64() * 1_000.0),
            next_first_frame_submitted_ms: next_first_submitted_ms,
        };

        let previous = self
            .active
            .replace(next)
            .ok_or_else(|| ReplayHandlerError::new("The active replay segment is missing."))?;
        let previous_sequence_number = previous.sequence_number;
        let job = previous.into_finalize_job(&self.flags, Some(boundary));
        let state_started = Instant::now();
        let submit_result = self
            .finalizer
            .as_mut()
            .ok_or_else(|| ReplayHandlerError::new("The replay finalizer is unavailable."))?
            .submit(job);
        if submit_result.is_err() {
            phases.filesystem += state_started.elapsed();
            phases.filesystem_operation_count += 1;
        }
        submit_result.map_err(ReplayHandlerError::new)?;
        phases.state_update += state_started.elapsed();
        self.rotation_lifecycle.old_segment_queued_ms = Some(self.elapsed_ms());
        let state_started = Instant::now();
        self.flags.shared.record_rotation(diagnostics);
        phases.state_update += state_started.elapsed();
        self.rotation_lifecycle.swap_completed_ms = Some(self.elapsed_ms());
        self.publish_lifecycle(phases);
        self.awaiting_following_frame = true;
        self.rotation_requested_ms = None;
        Ok(Some(previous_sequence_number))
    }

    fn process_frame(
        &mut self,
        frame: &mut Frame<'_>,
        phases: &mut CallbackPhaseDurations,
    ) -> Result<(), ReplayHandlerError> {
        let state_started = Instant::now();
        self.flags.shared.frame_observed();
        phases.state_update += state_started.elapsed();

        if self.awaiting_following_frame {
            self.rotation_lifecycle.following_frame_arrived_ms = Some(self.elapsed_ms());
            self.awaiting_following_frame = false;
            self.publish_lifecycle(phases);
        }

        let evaluation_started = Instant::now();
        self.poll_preparation(phases)?;
        let pending_save_rotation = self.flags.shared.pending_rotation_request();
        let active_has_frames = self.active.as_ref().is_some_and(ActiveSegment::has_frames);
        let nominal_rotation_due = self
            .active
            .as_ref()
            .is_some_and(|active| active.should_rotate(Instant::now()))
            && active_has_frames;
        let save_rotation_due = pending_save_rotation.filter(|_| active_has_frames);

        if nominal_rotation_due || save_rotation_due.is_some() {
            self.mark_rotation_requested(phases);
            self.ensure_preparation(phases)?;
            self.poll_preparation(phases)?;
            if self.prepared.is_some() {
                phases.rotation_evaluation += evaluation_started.elapsed();
                let swap_started = Instant::now();
                let sequence_number = self.rotate_on_frame(frame, phases)?;
                phases.swap += swap_started.elapsed();
                if let (Some(request_id), Some(sequence_number)) =
                    (save_rotation_due, sequence_number)
                {
                    let state_started = Instant::now();
                    self.flags
                        .shared
                        .acknowledge_rotation(request_id, sequence_number);
                    phases.state_update += state_started.elapsed();
                }
                return Ok(());
            }
        } else if self
            .active
            .as_ref()
            .is_some_and(|active| active.should_prepare(Instant::now()))
        {
            self.ensure_preparation(phases)?;
        }
        phases.rotation_evaluation += evaluation_started.elapsed();

        let was_empty = !self.active.as_ref().is_some_and(ActiveSegment::has_frames);
        let encoded = self
            .active
            .as_mut()
            .ok_or_else(|| ReplayHandlerError::new("The active replay segment is missing."))?
            .encode_frame(frame, self.session_started, self.flags.frame_rate)
            .map_err(ReplayHandlerError::new)?;
        self.flags
            .shared
            .callback_telemetry
            .record_send_frame(encoded.send_duration);
        self.flags
            .shared
            .callback_telemetry
            .record_encoder_frame(encoded.telemetry);
        if was_empty {
            self.rotation_lifecycle.active_segment_first_frame_ms = self
                .active
                .as_ref()
                .and_then(|active| active.first_frame_submitted_ms);
            self.publish_lifecycle(phases);
        }
        Ok(())
    }

    fn finish_session(&mut self) -> Result<(), ReplayHandlerError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;

        if let Some(prepared) = self.prepared.take() {
            prepared.discard();
        }
        if let Some(prewarmer) = self.prewarmer.take() {
            prewarmer.shutdown();
        }
        self.preparation_in_flight = false;
        self.flags
            .shared
            .set_encoder_preparation_state(false, false);

        if let Some(active) = self.active.take() {
            let job = active.into_finalize_job(&self.flags, None);
            self.finalizer
                .as_mut()
                .ok_or_else(|| ReplayHandlerError::new("The replay finalizer is unavailable."))?
                .submit(job)
                .map_err(ReplayHandlerError::new)?;
        }
        if let Some(finalizer) = self.finalizer.take() {
            finalizer.shutdown().map_err(ReplayHandlerError::new)?;
        }

        Ok(())
    }
}

impl Drop for ReplayCaptureHandler {
    fn drop(&mut self) {
        if let Err(error) = self.finish_session() {
            self.flags.shared.mark_error(format!(
                "The active replay segment could not be finalized during capture shutdown: {error}"
            ));
        }
    }
}

#[derive(Debug)]
struct ReplayHandlerError(String);

impl ReplayHandlerError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ReplayHandlerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ReplayHandlerError {}

impl GraphicsCaptureApiHandler for ReplayCaptureHandler {
    type Flags = ReplayCaptureFlags;
    type Error = ReplayHandlerError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        ensure_borderless_capture_access().map_err(ReplayHandlerError::new)?;
        let session_started = ctx.flags.session_started;
        let finalizer =
            FinalizerWorker::new(Arc::clone(&ctx.flags.shared)).map_err(ReplayHandlerError::new)?;
        let active = ActiveSegment::create(&ctx.flags, 1, session_started)
            .map_err(ReplayHandlerError::new)?;
        let prewarmer = EncoderPrewarmer::new(ctx.flags.clone(), session_started)
            .map_err(ReplayHandlerError::new)?;
        let rotation_lifecycle = RotationLifecycleTrace {
            active_sequence_number: Some(active.sequence_number),
            ..RotationLifecycleTrace::default()
        };
        ctx.flags
            .shared
            .publish_rotation_lifecycle(&rotation_lifecycle);
        ctx.flags.shared.mark_running();

        Ok(Self {
            flags: ctx.flags,
            active: Some(active),
            finalizer: Some(finalizer),
            prewarmer: Some(prewarmer),
            prepared: None,
            preparation_in_flight: false,
            next_sequence: 2,
            session_started,
            rotation_requested_ms: None,
            rotation_lifecycle,
            awaiting_following_frame: false,
            finished: false,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let callback_started = Instant::now();
        let mut phases = CallbackPhaseDurations::default();
        IN_REPLAY_FRAME_CALLBACK.with(|in_callback| in_callback.set(true));
        let result = self.process_frame(frame, &mut phases);
        IN_REPLAY_FRAME_CALLBACK.with(|in_callback| in_callback.set(false));
        self.flags
            .shared
            .record_callback_phases(callback_started.elapsed(), phases);
        result
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        let finalize_result = self.finish_session();
        let message = match finalize_result {
            Ok(()) => {
                "The selected capture target closed. The final replay segment was finalized safely."
                    .to_string()
            }
            Err(error) => format!(
                "The selected capture target closed, and the final replay segment could not be finalized: {error}"
            ),
        };
        self.flags.shared.mark_error(message.clone());
        Err(ReplayHandlerError::new(message))
    }
}

fn start_target_capture<T>(target: T, flags: ReplayCaptureFlags) -> Result<(), String>
where
    T: TryInto<GraphicsCaptureItemType> + Send + 'static,
{
    let shared = Arc::clone(&flags.shared);
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
        ReplayCaptureHandler::start_free_threaded(settings).map_err(|error| match error {
            GraphicsCaptureApiError::NewHandlerError(error) => error.to_string(),
            error => format!("Replay capture could not start: {error}"),
        })?;

    loop {
        if control.is_finished() {
            return control
                .wait()
                .map_err(|error| format!("Replay capture ended unexpectedly: {error}"));
        }
        if shared.should_stop() {
            return control
                .stop()
                .map_err(|error| format!("Replay capture could not stop cleanly: {error}"));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn cleanup_session_directories(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| {
        format!(
            "Could not access replay-buffer root '{}': {error}",
            root.display()
        )
    })?;

    let entries = fs::read_dir(root).map_err(|error| {
        format!(
            "Could not inspect replay-buffer root '{}': {error}",
            root.display()
        )
    })?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Could not inspect a stale replay session: {error}"))?;
        let path = entry.path();
        if path.parent() != Some(root)
            || !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
        {
            continue;
        }
        fs::remove_dir_all(&path).map_err(|error| {
            format!(
                "Could not remove stale replay session '{}': {error}",
                path.display()
            )
        })?;
    }

    Ok(())
}

fn path_is_inside_session(path: &Path, session_directory: Option<&Path>) -> bool {
    session_directory.is_some_and(|directory| path.parent() == Some(directory))
}

fn even_dimension(value: u32) -> Result<u32, String> {
    if value == 0 {
        return Err("The selected capture target has a zero-sized dimension.".to_string());
    }
    Ok(if value % 2 == 0 { value } else { value + 1 })
}

fn create_session_id() -> String {
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{}-{counter:04}", unix_timestamp_ms())
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::Path;
    use std::sync::Arc;

    use super::{
        calculate_rotation_diagnostics, validate_start_request, AudioReplayConfiguration,
        ReplayBufferStartRequest, SharedReplay, VideoFrameTimingPoint,
    };
    use crate::capture::encoder::{EncoderChoice, EncoderCodec};
    use crate::capture::targets::{CaptureTargetRequest, CaptureTargetType};
    use crate::replay::segment::CompletedSegment;
    use crate::replay::state::ReplayLifecycleState;

    fn request(duration: u32, frame_rate: u32) -> ReplayBufferStartRequest {
        ReplayBufferStartRequest {
            target: CaptureTargetRequest {
                target_type: CaptureTargetType::Monitor,
                id: "monitor:test".to_string(),
            },
            encoder: EncoderChoice::Automatic,
            replay_duration_seconds: duration,
            frame_rate,
            audio: AudioReplayConfiguration::default(),
        }
    }

    fn completed_segment(path: &Path, sequence_number: u64) -> CompletedSegment {
        let frame_timing_points = (0..120)
            .map(|frame_index| VideoFrameTimingPoint {
                frame_index,
                source_qpc_100ns: ((i128::from(frame_index) * 20_000_000) / 120) as i64,
                encoded_pts_100ns: ((i128::from(frame_index) * 20_000_000) / 120) as i64,
            })
            .collect::<Vec<_>>();
        CompletedSegment {
            sequence_number,
            file_path: path.to_string_lossy().into_owned(),
            start_timestamp_ms: sequence_number * 2_000,
            end_timestamp_ms: (sequence_number + 1) * 2_000,
            actual_duration_ms: 2_000,
            first_frame_timestamp_100ns: 0,
            last_frame_timestamp_100ns: 19_833_333,
            encoded_start_pts_100ns: 0,
            encoded_last_frame_pts_100ns: 19_833_334,
            encoded_end_pts_100ns: 20_000_000,
            encoded_duration_100ns: 20_000_000,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: 10_000_000,
            frame_timing_points,
            next_segment_first_frame_timestamp_100ns: None,
            source_frame_gap_ms: None,
            frame_count: 120,
            encoder_creation_time_ms: 10.0,
            encoder_creation_started_ms: 0.0,
            encoder_creation_completed_ms: 10.0,
            rotation_requested_ms: None,
            first_frame_submitted_ms: Some(0.0),
            last_frame_submitted_ms: Some(2_000.0),
            next_first_frame_submitted_ms: None,
            codec: "H.264".to_string(),
            width: 1920,
            height: 1080,
            frame_rate: 60,
            file_size: 4,
            finalized: true,
            finalization_time_ms: 10.0,
            rotation_gap_ms: Some(2.0),
        }
    }

    #[test]
    fn accepts_only_supported_replay_durations() {
        for duration in [30, 60, 120, 180, 300] {
            assert!(validate_start_request(&request(duration, 60)).is_ok());
        }
        assert!(validate_start_request(&request(45, 60)).is_err());
    }

    #[test]
    fn rejects_unsupported_frame_rate() {
        assert!(validate_start_request(&request(30, 144)).is_err());
    }

    #[test]
    fn normal_boundary_interval_reports_no_missed_frames() {
        let diagnostics = calculate_rotation_diagnostics(10_000_000, 10_166_667, 60, 175.0);
        assert!((diagnostics.source_frame_gap_ms - 16.6667).abs() < 0.001);
        assert_eq!(diagnostics.estimated_frames_missed, 0);
        assert!(!diagnostics.material_source_gap);
        assert_eq!(diagnostics.encoder_creation_ms, 175.0);
    }

    #[test]
    fn material_boundary_interval_estimates_missed_frames() {
        let diagnostics = calculate_rotation_diagnostics(10_000_000, 11_833_333, 60, 175.0);
        assert!((diagnostics.source_frame_gap_ms - 183.3333).abs() < 0.001);
        assert_eq!(diagnostics.estimated_frames_missed, 10);
        assert!(diagnostics.material_source_gap);
    }

    #[test]
    fn normal_session_state_transitions_are_explicit() {
        let shared = SharedReplay::new();
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Stopped);

        shared.begin(&request(60, 60));
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Starting);

        shared.mark_running();
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Running);

        shared.request_stop();
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Stopping);

        shared.mark_stopped();
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Stopped);
    }

    #[test]
    fn pinned_evicted_segment_is_deleted_only_after_unpin() {
        let directory =
            std::env::temp_dir().join(format!("justin-replay-pin-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();

        let shared = Arc::new(SharedReplay::new());
        shared.begin(&request(30, 60));
        {
            let mut inner = shared.lock();
            inner.replay_duration_seconds = 5;
            inner.ring = super::SegmentRing::new(5);
        }
        shared.configure(
            "Test display".to_string(),
            EncoderCodec::H264,
            1920,
            1080,
            "test-session".to_string(),
            directory.clone(),
        );
        shared.mark_running();

        for sequence in 1..=4 {
            let path = directory.join(format!("segment-{sequence:06}.mp4"));
            fs::write(&path, b"test").unwrap();
            if sequence <= 3 {
                shared.complete_segment(completed_segment(&path, sequence));
            }
        }

        let snapshot = shared.pin_snapshot(3, 123).unwrap();
        let first_path = directory.join("segment-000001.mp4");
        shared.complete_segment(completed_segment(&directory.join("segment-000004.mp4"), 4));
        assert!(first_path.exists());

        drop(snapshot);
        assert!(!first_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
