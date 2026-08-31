// The retired custom Replay DXGI scheduler remains temporarily in this file as isolated
// migration/reference code and test scaffolding. Production Replay no longer calls it.
#![allow(dead_code, unused_imports)]

use std::cell::Cell;
use std::collections::{HashMap, VecDeque};
use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use windows::core::Interface;
use windows::Win32::Graphics::Dxgi::IDXGISurface1;
use windows::Win32::Graphics::Gdi::{DeleteObject, HGDIOBJ};
use windows::Win32::UI::WindowsAndMessaging::{
    DrawIconEx, GetCursorInfo, GetIconInfo, CURSORINFO, CURSOR_SHOWING, CURSOR_SUPPRESSED,
    DI_NORMAL, HICON, ICONINFO,
};
use windows_capture::encoder::DetachedFrame;
use windows_capture::settings::ColorFormat;

use crate::capture::compatibility::{
    CaptureHealth, CaptureMode, FrameHealthSample, PresentationClass, VideoBackend,
};
use crate::capture::continuous_baseline::is_continuous_baseline_active;
use crate::capture::encoder::{
    active_media_foundation_encoder_count, resolve_encoder, EncoderChoice, EncoderCodec,
    EncoderFrameTelemetry, VideoEncoderBackend, WindowsCaptureFileBackend,
};
use crate::capture::targets::{
    dxgi_monitor_for_target, ffmpeg_output_for_target, monitor_desktop_origin, resolve_target,
    CaptureTargetRequest, CaptureTargetType, NativeCaptureTarget, ResolvedCaptureTarget,
};
use windows_capture::dxgi_duplication_api::{
    DxgiDuplicationApi, DxgiDuplicationFormat, DxgiDuplicationRecreationContext, Error as DxgiError,
};
use windows_capture::monitor::Monitor;

use super::audio::{
    AudioReplayConfiguration, AudioReplaySession, AudioReplayShared, AudioSaveBarrierTelemetry,
    AudioSnapshotPinGuard, AudioSnapshotPlan, AudioSnapshotTrack, ReplaySessionClock,
};
use super::ffmpeg_capture::{resolve_capture_capabilities, FfmpegCapturePlan, FfmpegReplayCapture};
use super::segment::{average_bitrate_mbps, CompletedSegment, SegmentRing, VideoFrameTimingPoint};
use super::state::{
    DxgiStartupStatus, ReplayBufferStatus, ReplayCommandResult, ReplayLifecycleState,
    ReplayNativeResourceStatus, RotationLifecycleTrace,
};
use super::timeline::SavedReplayTimeline;

pub const SEGMENT_DURATION: Duration = Duration::from_secs(2);
const RECENT_SEGMENT_LIMIT: usize = 5;
// Development telemetry heuristic only; this is not a production failure policy.
const DIAGNOSTIC_MATERIAL_GAP_INTERVALS: f64 = 2.0;
const ALLOWED_REPLAY_DURATIONS: [u32; 5] = [30, 60, 120, 180, 300];
const MAX_CATCH_UP_FRAMES_PER_WAKE: u64 = 4;
const MAX_REALTIME_BACKLOG_FRAMES: u64 = 120;
const MAX_PENDING_SOURCE_FRAMES: usize = 8;
const NORMAL_PREWARM_LEAD_SECONDS: u64 = 1;
// Two seconds for a normal WAV segment plus bounded queue/write/finalize headroom.
const AUDIO_SAVE_BARRIER_TIMEOUT: Duration = Duration::from_secs(5);
const CAPTURE_HEALTH_PROBE_DURATION: Duration = Duration::from_millis(2_200);
const DXGI_HEALTH_VALIDATION_LIMIT: Duration = Duration::from_secs(6);
const DXGI_RECOVERY_DEADLINE: Duration = Duration::from_secs(10);
const DXGI_RECOVERY_STABILITY_POLL_INTERVAL: Duration = Duration::from_millis(250);
const DXGI_RECOVERY_RECREATE_BACKOFF: Duration = Duration::from_millis(500);
const DXGI_RECOVERY_STABLE_POLLS: u8 = 3;
const DXGI_RECOVERY_RETAINED_DEVICE_ATTEMPTS: u64 = 2;
const DXGI_STARTUP_STABILITY_DEADLINE: Duration = Duration::from_secs(2);
const DXGI_STARTUP_STABILITY_POLL_INTERVAL: Duration = Duration::from_millis(150);
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
    video_timeline_start_qpc_100ns: AtomicI64,
    scheduler_current_output_frame_index: AtomicU64,
    scheduler_expected_output_frame_index: AtomicU64,
    scheduler_current_lateness_100ns: AtomicU64,
    scheduler_worst_lateness_100ns: AtomicU64,
    scheduler_catch_up_wakeups: AtomicU64,
    scheduler_max_catch_up_burst: AtomicU64,
    scheduler_catch_up_frames: AtomicU64,
    scheduler_rotation_catch_up_frames: AtomicU64,
    scheduler_save_pending_catch_up_frames: AtomicU64,
    scheduler_discontinuities: AtomicU64,
    scheduler_rebases: AtomicU64,
    dxgi_duplication_recreations: AtomicU64,
    ffmpeg_capture_processes: AtomicU64,
    ffmpeg_capture_restarts: AtomicU64,
    queue_full_retry_attempts: AtomicU64,
    recovered_queue_full_frames: AtomicU64,
    last_rotation_lateness_before_100ns: AtomicI64,
    last_rotation_lateness_after_100ns: AtomicI64,
    fresh_output_frames: AtomicU64,
    held_output_frames: AtomicU64,
    superseded_source_updates: AtomicU64,
    missed_realtime_output_frames: AtomicU64,
    source_frames_detached: AtomicU64,
    source_frames_rate_limited: AtomicU64,
    wgc_capture_sessions: AtomicU64,
    dxgi_duplication_sessions: AtomicU64,
    video_workers: AtomicU64,
}

impl ReplayCallbackTelemetry {
    fn reset(&self) {
        // Same-display recovery occurs inside the one long-lived Replay worker. Preserve native
        // resource gauges while resetting per-session metrics so a live worker is never erased
        // from diagnostics and later decremented from zero.
        let wgc_capture_sessions = self.wgc_capture_sessions.load(Ordering::Relaxed);
        let dxgi_duplication_sessions = self.dxgi_duplication_sessions.load(Ordering::Relaxed);
        let video_workers = self.video_workers.load(Ordering::Relaxed);
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
        self.video_timeline_start_qpc_100ns
            .store(-1, Ordering::Relaxed);
        self.scheduler_current_output_frame_index
            .store(0, Ordering::Relaxed);
        self.scheduler_expected_output_frame_index
            .store(0, Ordering::Relaxed);
        self.scheduler_current_lateness_100ns
            .store(0, Ordering::Relaxed);
        self.scheduler_worst_lateness_100ns
            .store(0, Ordering::Relaxed);
        self.scheduler_catch_up_wakeups.store(0, Ordering::Relaxed);
        self.scheduler_max_catch_up_burst
            .store(0, Ordering::Relaxed);
        self.scheduler_catch_up_frames.store(0, Ordering::Relaxed);
        self.scheduler_rotation_catch_up_frames
            .store(0, Ordering::Relaxed);
        self.scheduler_save_pending_catch_up_frames
            .store(0, Ordering::Relaxed);
        self.scheduler_discontinuities.store(0, Ordering::Relaxed);
        self.scheduler_rebases.store(0, Ordering::Relaxed);
        self.dxgi_duplication_recreations
            .store(0, Ordering::Relaxed);
        self.ffmpeg_capture_processes.store(0, Ordering::Relaxed);
        self.ffmpeg_capture_restarts.store(0, Ordering::Relaxed);
        self.queue_full_retry_attempts.store(0, Ordering::Relaxed);
        self.recovered_queue_full_frames.store(0, Ordering::Relaxed);
        self.last_rotation_lateness_before_100ns
            .store(-1, Ordering::Relaxed);
        self.last_rotation_lateness_after_100ns
            .store(-1, Ordering::Relaxed);
        self.fresh_output_frames.store(0, Ordering::Relaxed);
        self.held_output_frames.store(0, Ordering::Relaxed);
        self.superseded_source_updates.store(0, Ordering::Relaxed);
        self.missed_realtime_output_frames
            .store(0, Ordering::Relaxed);
        self.source_frames_detached.store(0, Ordering::Relaxed);
        self.source_frames_rate_limited.store(0, Ordering::Relaxed);
        self.wgc_capture_sessions
            .store(wgc_capture_sessions, Ordering::Relaxed);
        self.dxgi_duplication_sessions
            .store(dxgi_duplication_sessions, Ordering::Relaxed);
        self.video_workers.store(video_workers, Ordering::Relaxed);
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
            self.queue_full_retry_attempts
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
    #[serde(default)]
    pub capture_mode: CaptureMode,
    pub encoder: EncoderChoice,
    pub replay_duration_seconds: u32,
    pub frame_rate: u32,
    #[serde(default)]
    pub quality: ReplayQuality,
    #[serde(default)]
    pub audio: AudioReplayConfiguration,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum ReplayQuality {
    High,
    #[default]
    Balanced,
    SmallerFiles,
}

impl ReplayQuality {
    const fn qp(self) -> u8 {
        match self {
            Self::High => 18,
            Self::Balanced => 23,
            Self::SmallerFiles => 28,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ActiveCaptureMode {
    DisplayCapture,
}

impl ActiveCaptureMode {
    const fn label(self) -> &'static str {
        match self {
            Self::DisplayCapture => "Display Capture",
        }
    }

    const fn backend(self) -> VideoBackend {
        VideoBackend::FfmpegDdagrab
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct CapturePlan {
    active_mode: ActiveCaptureMode,
    selection_reason: &'static str,
}

fn plan_capture(_requested_mode: CaptureMode, _target_type: CaptureTargetType) -> CapturePlan {
    CapturePlan {
        active_mode: ActiveCaptureMode::DisplayCapture,
        selection_reason: "Replay captures one persistent physical display",
    }
}

pub struct ReplaySaveSnapshot {
    pub save_request_timestamp_ms: u64,
    pub save_request_qpc_100ns: i64,
    pub requested_duration_seconds: u32,
    pub capture_target_label: Option<String>,
    pub capture_target_type: Option<String>,
    pub overlay_monitor_origin: Option<(i32, i32)>,
    pub segments: Vec<CompletedSegment>,
    pub video_timeline: SavedReplayTimeline,
    pub audio_snapshot_plans: Vec<AudioSnapshotPlan>,
    pub audio_snapshot_tracks: Vec<AudioSnapshotTrack>,
    pub audio_save_barriers: Vec<AudioSaveBarrierTelemetry>,
    pub video_boundary_wait_ms: f64,
    pub audio_barrier_wait_ms: f64,
    pub snapshot_ready_latency_ms: f64,
    _pins: Option<SegmentPinGuard>,
    _audio_pins: Option<AudioSnapshotPinGuard>,
}

impl ReplaySaveSnapshot {
    pub(crate) fn from_completed_recording(
        save_request_timestamp_ms: u64,
        requested_duration_seconds: u32,
        capture_target_label: String,
        segments: Vec<CompletedSegment>,
        video_timeline: SavedReplayTimeline,
        audio_snapshot_plans: Vec<AudioSnapshotPlan>,
        audio_snapshot_tracks: Vec<AudioSnapshotTrack>,
        audio_save_barriers: Vec<AudioSaveBarrierTelemetry>,
        audio_pins: AudioSnapshotPinGuard,
        audio_barrier_wait_ms: f64,
    ) -> Self {
        Self {
            save_request_timestamp_ms,
            save_request_qpc_100ns: video_timeline.clip_capture_end_qpc_100ns,
            requested_duration_seconds,
            capture_target_label: Some(capture_target_label),
            capture_target_type: Some("watchParty".to_string()),
            overlay_monitor_origin: None,
            segments,
            video_timeline,
            audio_snapshot_plans,
            audio_snapshot_tracks,
            audio_save_barriers,
            video_boundary_wait_ms: 0.0,
            audio_barrier_wait_ms,
            snapshot_ready_latency_ms: audio_barrier_wait_ms,
            _pins: None,
            _audio_pins: Some(audio_pins),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SaveBoundaryRequest {
    request_id: u64,
    anchor_qpc_100ns: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct AcknowledgedSaveBoundary {
    request_id: u64,
    anchor_qpc_100ns: i64,
    final_sequence_number: u64,
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
    target_type: Option<String>,
    requested_encoder: Option<String>,
    actual_encoder: Option<String>,
    requested_capture_mode: String,
    active_capture_mode: Option<String>,
    presentation: Option<String>,
    video_backend: String,
    backend_selection_reason: Option<String>,
    capture_health: String,
    capture_fallback_reason: Option<String>,
    last_recovery_message: Option<String>,
    recovery_elapsed_ms: Option<f64>,
    recovery_recreation_attempts: u64,
    dxgi_startup: DxgiStartupStatus,
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
    source_frame_gap_count: u64,
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
    session_clock: Option<ReplaySessionClock>,
    pending_save_boundary: Option<SaveBoundaryRequest>,
    acknowledged_save_boundary: Option<AcknowledgedSaveBoundary>,
}

impl ReplayInner {
    fn stopped() -> Self {
        Self {
            state: ReplayLifecycleState::Stopped,
            error_message: None,
            target_id: None,
            target_label: None,
            target_type: None,
            requested_encoder: None,
            actual_encoder: None,
            requested_capture_mode: "Display Capture".to_string(),
            active_capture_mode: None,
            presentation: None,
            video_backend: "FFmpeg ddagrab".to_string(),
            backend_selection_reason: None,
            capture_health: "Idle".to_string(),
            capture_fallback_reason: None,
            last_recovery_message: None,
            recovery_elapsed_ms: None,
            recovery_recreation_attempts: 0,
            dxgi_startup: DxgiStartupStatus::default(),
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
            source_frame_gap_count: 0,
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
            session_clock: None,
            pending_save_boundary: None,
            acknowledged_save_boundary: None,
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
        let timeline_start = callback_telemetry
            .video_timeline_start_qpc_100ns
            .load(Ordering::Relaxed);
        let expected_output_index = callback_telemetry
            .scheduler_expected_output_frame_index
            .load(Ordering::Relaxed);
        let output_frames = callback_telemetry
            .fresh_output_frames
            .load(Ordering::Relaxed)
            .saturating_add(
                callback_telemetry
                    .held_output_frames
                    .load(Ordering::Relaxed),
            );
        let elapsed_output_seconds = (self.frame_rate > 0 && timeline_start >= 0)
            .then(|| (expected_output_index.saturating_add(1)) as f64 / f64::from(self.frame_rate));
        let system_cursor_flags = cursor_flags();
        ReplayBufferStatus {
            state: self.state,
            error_message: if self.state != ReplayLifecycleState::Error
                && self.capture_health == "Healthy"
            {
                None
            } else {
                self.error_message.clone()
            },
            target_id: self.target_id.clone(),
            target_label: self.target_label.clone(),
            requested_encoder: self.requested_encoder.clone(),
            actual_encoder: self.actual_encoder.clone(),
            requested_capture_mode: self.requested_capture_mode.clone(),
            active_capture_mode: self.active_capture_mode.clone(),
            presentation: self.presentation.clone(),
            video_backend: self.video_backend.clone(),
            backend_selection_reason: self.backend_selection_reason.clone(),
            capture_health: if self.state == ReplayLifecycleState::Starting
                && self.capture_health == "Idle"
            {
                "Probing".to_string()
            } else {
                self.capture_health.clone()
            },
            capture_fallback_reason: self.capture_fallback_reason.clone(),
            last_recovery_message: self.last_recovery_message.clone(),
            recovery_elapsed_ms: self.recovery_elapsed_ms,
            recovery_recreation_attempts: self.recovery_recreation_attempts,
            dxgi_startup: self.dxgi_startup.clone(),
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
            average_source_frame_gap_ms: (self.source_frame_gap_count > 0)
                .then(|| self.total_source_frame_gap_ms / self.source_frame_gap_count as f64),
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
            video_timeline_start_qpc_100ns: (timeline_start >= 0).then_some(timeline_start),
            scheduler_current_output_frame_index: (timeline_start >= 0).then(|| {
                callback_telemetry
                    .scheduler_current_output_frame_index
                    .load(Ordering::Relaxed)
            }),
            scheduler_expected_output_frame_index: (timeline_start >= 0)
                .then_some(expected_output_index),
            scheduler_current_lateness_ms: (timeline_start >= 0).then(|| {
                callback_telemetry
                    .scheduler_current_lateness_100ns
                    .load(Ordering::Relaxed) as f64
                    / 10_000.0
            }),
            scheduler_worst_lateness_ms: (timeline_start >= 0).then(|| {
                callback_telemetry
                    .scheduler_worst_lateness_100ns
                    .load(Ordering::Relaxed) as f64
                    / 10_000.0
            }),
            scheduler_catch_up_wakeups: callback_telemetry
                .scheduler_catch_up_wakeups
                .load(Ordering::Relaxed),
            scheduler_max_catch_up_burst: callback_telemetry
                .scheduler_max_catch_up_burst
                .load(Ordering::Relaxed),
            scheduler_catch_up_frames: callback_telemetry
                .scheduler_catch_up_frames
                .load(Ordering::Relaxed),
            scheduler_rotation_catch_up_frames: callback_telemetry
                .scheduler_rotation_catch_up_frames
                .load(Ordering::Relaxed),
            scheduler_save_pending_catch_up_frames: callback_telemetry
                .scheduler_save_pending_catch_up_frames
                .load(Ordering::Relaxed),
            scheduler_discontinuities: callback_telemetry
                .scheduler_discontinuities
                .load(Ordering::Relaxed),
            scheduler_rebases: callback_telemetry.scheduler_rebases.load(Ordering::Relaxed),
            dxgi_duplication_recreations: callback_telemetry
                .dxgi_duplication_recreations
                .load(Ordering::Relaxed),
            ffmpeg_capture_restarts: callback_telemetry
                .ffmpeg_capture_restarts
                .load(Ordering::Relaxed),
            queue_full_retry_attempts: callback_telemetry
                .queue_full_retry_attempts
                .load(Ordering::Relaxed),
            recovered_queue_full_frames: callback_telemetry
                .recovered_queue_full_frames
                .load(Ordering::Relaxed),
            last_rotation_lateness_before_ms: atomic_100ns_ms(
                &callback_telemetry.last_rotation_lateness_before_100ns,
            ),
            last_rotation_lateness_after_ms: atomic_100ns_ms(
                &callback_telemetry.last_rotation_lateness_after_100ns,
            ),
            fresh_output_frames: callback_telemetry
                .fresh_output_frames
                .load(Ordering::Relaxed),
            held_output_frames: callback_telemetry
                .held_output_frames
                .load(Ordering::Relaxed),
            superseded_source_updates: callback_telemetry
                .superseded_source_updates
                .load(Ordering::Relaxed),
            missed_realtime_output_frames: callback_telemetry
                .missed_realtime_output_frames
                .load(Ordering::Relaxed),
            source_frames_detached: callback_telemetry
                .source_frames_detached
                .load(Ordering::Relaxed),
            source_frames_rate_limited: callback_telemetry
                .source_frames_rate_limited
                .load(Ordering::Relaxed),
            source_frame_update_rate: elapsed_output_seconds
                .filter(|elapsed| *elapsed > 0.0)
                .map(|elapsed| frames_observed as f64 / elapsed),
            output_cfr_rate: elapsed_output_seconds
                .filter(|elapsed| *elapsed > 0.0)
                .map(|elapsed| output_frames as f64 / elapsed),
            frame_pool_creation_method: if self.video_backend == "FFmpeg ddagrab" {
                "FFmpeg-owned D3D11 frames".to_string()
            } else {
                "Not used by display capture".to_string()
            },
            frame_pool_buffer_count: 0,
            native_resources: ReplayNativeResourceStatus {
                logical_replay_sessions: u64::from(self.state.is_active()),
                wgc_capture_sessions: callback_telemetry
                    .wgc_capture_sessions
                    .load(Ordering::Relaxed),
                dxgi_duplication_sessions: callback_telemetry
                    .dxgi_duplication_sessions
                    .load(Ordering::Relaxed),
                d3d11_devices: callback_telemetry
                    .wgc_capture_sessions
                    .load(Ordering::Relaxed)
                    + callback_telemetry
                        .dxgi_duplication_sessions
                        .load(Ordering::Relaxed),
                d3d11_device_contexts: callback_telemetry
                    .wgc_capture_sessions
                    .load(Ordering::Relaxed)
                    + callback_telemetry
                        .dxgi_duplication_sessions
                        .load(Ordering::Relaxed),
                frame_pools: callback_telemetry
                    .wgc_capture_sessions
                    .load(Ordering::Relaxed),
                media_foundation_encoders: active_media_foundation_encoder_count(),
                ffmpeg_capture_processes: callback_telemetry
                    .ffmpeg_capture_processes
                    .load(Ordering::Relaxed),
                video_workers: callback_telemetry.video_workers.load(Ordering::Relaxed)
                    + active_media_foundation_encoder_count(),
                audio_workers: audio
                    .tracks
                    .iter()
                    .filter(|track| {
                        track.enabled
                            && matches!(
                                track.state,
                                super::audio::AudioTrackState::Preparing
                                    | super::audio::AudioTrackState::Prepared
                                    | super::audio::AudioTrackState::Running
                            )
                    })
                    .count() as u64,
                system_cursor_visible: system_cursor_flags
                    .map(|flags| flags & CURSOR_SHOWING.0 != 0),
                system_cursor_suppressed: system_cursor_flags
                    .map(|flags| flags & CURSOR_SUPPRESSED.0 != 0),
                capture_cursor_mode: if self.video_backend == "DXGI" {
                    "desktopDuplicationCompositedSystemCursor".to_string()
                } else if self.video_backend == "FFmpeg ddagrab" {
                    "ffmpegDdagrabCompositedSystemCursor".to_string()
                } else {
                    "systemDefaultCapturedOutputOnly".to_string()
                },
            },
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
    last_source_frame_qpc_100ns: AtomicI64,
    callback_telemetry: ReplayCallbackTelemetry,
    scheduler_discontinuity: Arc<SchedulerDiscontinuity>,
    audio: Arc<AudioReplayShared>,
}

#[derive(Default)]
struct SchedulerDiscontinuity {
    generation: AtomicU64,
    paused: AtomicBool,
    resume_qpc_100ns: AtomicI64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SchedulerDiscontinuitySnapshot {
    generation: u64,
    paused: bool,
    resume_qpc_100ns: Option<i64>,
}

impl SchedulerDiscontinuity {
    fn reset(&self) {
        self.resume_qpc_100ns.store(-1, Ordering::Relaxed);
        self.paused.store(false, Ordering::Relaxed);
        self.generation.store(0, Ordering::Release);
    }

    fn pause(&self) -> u64 {
        self.resume_qpc_100ns.store(-1, Ordering::Relaxed);
        self.paused.store(true, Ordering::Release);
        self.generation.fetch_add(1, Ordering::AcqRel) + 1
    }

    fn resume(&self, qpc_100ns: i64) {
        self.resume_qpc_100ns.store(qpc_100ns, Ordering::Relaxed);
        self.paused.store(false, Ordering::Release);
    }

    fn snapshot(&self) -> SchedulerDiscontinuitySnapshot {
        let generation = self.generation.load(Ordering::Acquire);
        let paused = self.paused.load(Ordering::Acquire);
        let resume_qpc_100ns = self.resume_qpc_100ns.load(Ordering::Relaxed);
        SchedulerDiscontinuitySnapshot {
            generation,
            paused,
            resume_qpc_100ns: (resume_qpc_100ns >= 0).then_some(resume_qpc_100ns),
        }
    }
}

struct VideoWorkerGuard {
    shared: Arc<SharedReplay>,
}

impl VideoWorkerGuard {
    fn new(shared: &Arc<SharedReplay>) -> Self {
        shared
            .callback_telemetry
            .video_workers
            .fetch_add(1, Ordering::Relaxed);
        Self {
            shared: Arc::clone(shared),
        }
    }
}

impl Drop for VideoWorkerGuard {
    fn drop(&mut self) {
        self.shared
            .callback_telemetry
            .video_workers
            .fetch_sub(1, Ordering::Relaxed);
    }
}

struct DxgiSessionGuard {
    shared: Arc<SharedReplay>,
}

impl DxgiSessionGuard {
    fn new(shared: &Arc<SharedReplay>) -> Self {
        shared
            .callback_telemetry
            .dxgi_duplication_sessions
            .fetch_add(1, Ordering::Relaxed);
        Self {
            shared: Arc::clone(shared),
        }
    }
}

impl Drop for DxgiSessionGuard {
    fn drop(&mut self) {
        self.shared
            .callback_telemetry
            .dxgi_duplication_sessions
            .fetch_sub(1, Ordering::Relaxed);
    }
}

impl SharedReplay {
    fn new() -> Self {
        Self {
            inner: Mutex::new(ReplayInner::stopped()),
            changed: Condvar::new(),
            stop_requested: AtomicBool::new(false),
            frames_observed: AtomicU64::new(0),
            last_source_frame_qpc_100ns: AtomicI64::new(-1),
            callback_telemetry: ReplayCallbackTelemetry::default(),
            scheduler_discontinuity: Arc::new(SchedulerDiscontinuity::default()),
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

    #[cfg(test)]
    fn begin(&self, request: &ReplayBufferStartRequest) {
        self.prepare_start(
            request,
            None,
            CapturePlan {
                active_mode: ActiveCaptureMode::DisplayCapture,
                selection_reason: "Replay captures one persistent physical display",
            },
        );
    }

    fn begin_with_plan(
        &self,
        request: &ReplayBufferStartRequest,
        presentation: Option<PresentationClass>,
        plan: CapturePlan,
    ) {
        self.prepare_start(request, presentation, plan);
    }

    fn prepare_start(
        &self,
        request: &ReplayBufferStartRequest,
        presentation: Option<PresentationClass>,
        plan: CapturePlan,
    ) {
        self.audio.reset();
        self.stop_requested.store(false, Ordering::Release);
        self.frames_observed.store(0, Ordering::Relaxed);
        self.last_source_frame_qpc_100ns
            .store(-1, Ordering::Relaxed);
        self.callback_telemetry.reset();
        self.scheduler_discontinuity.reset();
        let mut inner = self.lock();
        *inner = ReplayInner {
            state: ReplayLifecycleState::Starting,
            error_message: None,
            target_id: Some(request.target.id.clone()),
            target_label: None,
            target_type: Some(
                match request.target.target_type {
                    CaptureTargetType::Monitor => "monitor",
                    CaptureTargetType::Window => "window",
                }
                .to_string(),
            ),
            requested_encoder: Some(request.encoder.result_name().to_string()),
            actual_encoder: None,
            // Keep the response field for frontend/backward wire compatibility, but expose only
            // the effective product policy. Legacy Auto/Game/Screen preference values are no
            // longer capture decisions.
            requested_capture_mode: "Display Capture".to_string(),
            active_capture_mode: Some(plan.active_mode.label().to_string()),
            presentation: presentation.map(|value| value.label().to_string()),
            video_backend: plan.active_mode.backend().label().to_string(),
            backend_selection_reason: Some(plan.selection_reason.to_string()),
            capture_health: "Probing".to_string(),
            capture_fallback_reason: None,
            last_recovery_message: None,
            recovery_elapsed_ms: None,
            recovery_recreation_attempts: 0,
            dxgi_startup: DxgiStartupStatus::default(),
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
            source_frame_gap_count: 0,
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
            session_clock: None,
            pending_save_boundary: None,
            acknowledged_save_boundary: None,
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
        session_clock: ReplaySessionClock,
    ) {
        let mut inner = self.lock();
        inner.target_label = Some(target_label);
        inner.actual_encoder = Some(actual_encoder.display_name().to_string());
        inner.width = width;
        inner.height = height;
        inner.session_id = Some(session_id);
        inner.session_directory = Some(session_directory);
        inner.session_clock = Some(session_clock);
        self.changed.notify_all();
    }

    fn next_segment_sequence_number(&self) -> u64 {
        self.lock()
            .ring
            .recent(1)
            .first()
            .map_or(1, |segment| segment.sequence_number.saturating_add(1))
    }

    fn mark_running(&self) {
        let mut inner = self.lock();
        if inner.state != ReplayLifecycleState::Error {
            inner.state = ReplayLifecycleState::Running;
            inner.next_encoder_state = "waiting_for_prewarm_point".to_string();
        }
        self.changed.notify_all();
    }

    fn set_capture_health(
        &self,
        backend: VideoBackend,
        health: &str,
        fallback_reason: Option<String>,
    ) {
        let mut inner = self.lock();
        if inner.state == ReplayLifecycleState::Error {
            return;
        }
        inner.video_backend = backend.label().to_string();
        inner.capture_health = health.to_string();
        inner.capture_fallback_reason = fallback_reason;
        if health == "Healthy" && inner.state != ReplayLifecycleState::Error {
            inner.error_message = None;
        }
        self.changed.notify_all();
    }

    fn set_capture_selection(
        &self,
        presentation: Option<PresentationClass>,
        active_mode: ActiveCaptureMode,
        selection_reason: &str,
        health: &str,
        fallback_reason: Option<String>,
    ) {
        let mut inner = self.lock();
        inner.presentation = presentation.map(|value| value.label().to_string());
        inner.active_capture_mode = Some(active_mode.label().to_string());
        inner.video_backend = active_mode.backend().label().to_string();
        inner.backend_selection_reason = Some(selection_reason.to_string());
        inner.capture_health = health.to_string();
        inner.capture_fallback_reason = fallback_reason;
        self.changed.notify_all();
    }

    fn begin_dxgi_recovery(&self) {
        let mut inner = self.lock();
        if matches!(
            inner.state,
            ReplayLifecycleState::Starting | ReplayLifecycleState::Running
        ) {
            inner.state = ReplayLifecycleState::Starting;
            inner.video_backend = VideoBackend::Dxgi.label().to_string();
            inner.capture_health = "Recovering".to_string();
            inner.error_message = None;
            inner.capture_fallback_reason = Some(
                "Display Capture access was interrupted by a Windows display transition; SlickClip is reconnecting to the same display."
                    .to_string(),
            );
            inner.recovery_elapsed_ms = Some(0.0);
            inner.recovery_recreation_attempts = 0;
            inner.next_encoder_state = "dxgi_recovering".to_string();
        }
        self.changed.notify_all();
    }

    fn update_dxgi_recovery(&self, elapsed: Duration, recreation_attempts: u64) {
        let mut inner = self.lock();
        if inner.capture_health == "Recovering" {
            inner.recovery_elapsed_ms = Some(elapsed.as_secs_f64() * 1_000.0);
            inner.recovery_recreation_attempts = recreation_attempts;
        }
        self.changed.notify_all();
    }

    fn set_dxgi_output_diagnostics(
        &self,
        key: &DxgiRecoveryStabilityKey,
        adapter_identity: String,
    ) {
        let mut inner = self.lock();
        inner.dxgi_startup.output_identity = Some(key.output_identity.clone());
        inner.dxgi_startup.adapter_identity = Some(adapter_identity);
        inner.dxgi_startup.desktop_x = Some(key.desktop_origin.0);
        inner.dxgi_startup.desktop_y = Some(key.desktop_origin.1);
        inner.dxgi_startup.output_width = Some(key.width);
        inner.dxgi_startup.output_height = Some(key.height);
        self.changed.notify_all();
    }

    fn set_ffmpeg_output_diagnostics(
        &self,
        output_identity: String,
        adapter_index: u32,
        output_index: u32,
        desktop_x: i32,
        desktop_y: i32,
        width: u32,
        height: u32,
    ) {
        let mut inner = self.lock();
        inner.dxgi_startup.output_identity = Some(output_identity);
        inner.dxgi_startup.adapter_identity = Some(format!(
            "FFmpeg ddagrab adapter={adapter_index}, output_idx={output_index} (owned child process)"
        ));
        inner.dxgi_startup.desktop_x = Some(desktop_x);
        inner.dxgi_startup.desktop_y = Some(desktop_y);
        inner.dxgi_startup.output_width = Some(width);
        inner.dxgi_startup.output_height = Some(height);
        self.changed.notify_all();
    }

    fn set_ffmpeg_capability_diagnostics(
        &self,
        program: &str,
        actual_encoder: String,
        encoder_probe_failures: &[String],
    ) {
        let mut inner = self.lock();
        inner.actual_encoder = Some(actual_encoder);
        inner.backend_selection_reason = Some(format!(
            "Bundled FFmpeg ddagrab capability verified at '{program}'; encoder runtime probes selected the first usable configured candidate."
        ));
        if !encoder_probe_failures.is_empty() {
            inner.last_recovery_message = Some(format!(
                "Encoder probe fallbacks: {}",
                encoder_probe_failures.join("; ")
            ));
        }
        self.changed.notify_all();
    }

    fn begin_ffmpeg_recovery(&self, restart_count: u32, detail: String) {
        let mut inner = self.lock();
        if matches!(
            inner.state,
            ReplayLifecycleState::Starting | ReplayLifecycleState::Running
        ) {
            inner.state = ReplayLifecycleState::Starting;
            inner.video_backend = VideoBackend::FfmpegDdagrab.label().to_string();
            inner.capture_health = "Recovering".to_string();
            inner.error_message = None;
            inner.capture_fallback_reason = Some(detail);
            inner.recovery_recreation_attempts = u64::from(restart_count);
            inner.next_encoder_state = "ffmpeg_restarting".to_string();
        }
        self.changed.notify_all();
    }

    fn complete_ffmpeg_recovery(&self, restart_count: u32) {
        let mut inner = self.lock();
        if inner.state != ReplayLifecycleState::Error {
            inner.state = ReplayLifecycleState::Running;
            inner.error_message = None;
            inner.video_backend = VideoBackend::FfmpegDdagrab.label().to_string();
            inner.capture_health = "Healthy".to_string();
            inner.capture_fallback_reason = None;
            inner.last_recovery_message = Some(format!(
                "The owned FFmpeg display-capture child resumed on the same physical output after restart {restart_count}."
            ));
            inner.recovery_recreation_attempts = 0;
            inner.next_encoder_state = "ffmpeg_running".to_string();
        }
        self.changed.notify_all();
    }

    fn record_dxgi_health_probe(&self, probe: &DxgiHealthProbeStats) {
        let mut inner = self.lock();
        let status = &mut inner.dxgi_startup;
        status.acquire_results = status.acquire_results.saturating_add(probe.acquire_results);
        status.acquired_frames = status.acquired_frames.saturating_add(probe.acquired_frames);
        status.timeouts = status.timeouts.saturating_add(probe.timeouts);
        status.access_losses = status.access_losses.saturating_add(probe.access_losses);
        status.invalid_textures = status
            .invalid_textures
            .saturating_add(probe.invalid_textures);
        status.cpu_readbacks = status.cpu_readbacks.saturating_add(probe.cpu_readbacks);
        status.valid_samples = status.valid_samples.saturating_add(probe.valid_samples);
        if probe.last_acquire_result.is_some() {
            status.last_acquire_result = probe.last_acquire_result.clone();
        }
        if probe.last_valid_frame_elapsed_ms.is_some() {
            status.last_valid_frame_elapsed_ms = probe.last_valid_frame_elapsed_ms;
        }
        if probe.last_cpu_readback_ms.is_some() {
            status.last_cpu_readback_ms = probe.last_cpu_readback_ms;
        }
        self.changed.notify_all();
    }

    fn complete_dxgi_recovery(
        &self,
        fallback_reason: Option<String>,
        elapsed: Duration,
        recreation_attempts: u64,
    ) {
        let mut inner = self.lock();
        if inner.state != ReplayLifecycleState::Error {
            inner.error_message = None;
            inner.video_backend = VideoBackend::Dxgi.label().to_string();
            inner.capture_health = "Healthy".to_string();
            inner.capture_fallback_reason = fallback_reason;
            inner.last_recovery_message =
                Some(format!(
                "Recovered Display Capture on the same display in {:.0} ms ({} DXGI recreation {}).",
                elapsed.as_secs_f64() * 1_000.0,
                recreation_attempts,
                if recreation_attempts == 1 { "attempt" } else { "attempts" },
            ));
            inner.recovery_elapsed_ms = None;
            inner.recovery_recreation_attempts = 0;
            inner.next_encoder_state = "waiting_for_rebased_frame".to_string();
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
        if inner.state == ReplayLifecycleState::Error {
            return;
        }
        inner.state = ReplayLifecycleState::Error;
        inner.error_message = Some(error.into());
        inner.capture_health = "Failed".to_string();
        inner.next_encoder_state = "error".to_string();
        self.changed.notify_all();
    }

    fn frame_observed(&self) {
        self.frames_observed.fetch_add(1, Ordering::Relaxed);
    }

    fn record_ffmpeg_segment_metrics(&self, segment: &CompletedSegment) {
        let frame_count = segment.frame_count;
        if frame_count == 0 {
            return;
        }

        self.frames_observed
            .fetch_add(segment.source_update_count, Ordering::Relaxed);
        let previous_fresh = self
            .callback_telemetry
            .fresh_output_frames
            .fetch_add(segment.fresh_output_frame_count, Ordering::Relaxed);
        let previous_held = self
            .callback_telemetry
            .held_output_frames
            .fetch_add(segment.held_output_frame_count, Ordering::Relaxed);
        let total_output_frames = previous_fresh
            .saturating_add(previous_held)
            .saturating_add(segment.fresh_output_frame_count)
            .saturating_add(segment.held_output_frame_count);
        let last_frame_index = total_output_frames.saturating_sub(1);
        self.callback_telemetry
            .scheduler_current_output_frame_index
            .store(last_frame_index, Ordering::Relaxed);
        self.callback_telemetry
            .scheduler_expected_output_frame_index
            .store(last_frame_index, Ordering::Relaxed);

        let _ = self
            .callback_telemetry
            .video_timeline_start_qpc_100ns
            .compare_exchange(
                -1,
                segment.segment_session_start_qpc_100ns,
                Ordering::Relaxed,
                Ordering::Relaxed,
            );
    }

    fn record_source_frame_timestamp(&self, qpc_100ns: i64, frame_rate: u32) {
        let previous = self
            .last_source_frame_qpc_100ns
            .swap(qpc_100ns, Ordering::Relaxed);
        if previous < 0 {
            return;
        }
        let gap_ms = qpc_100ns.saturating_sub(previous).max(0) as f64 / 10_000.0;
        let expected_ms = 1_000.0 / f64::from(frame_rate.max(1));
        let estimated_intervals = (gap_ms / expected_ms).round() as u64;
        let estimated_missed = estimated_intervals.saturating_sub(1);
        let mut inner = self.lock();
        inner.last_source_frame_gap_ms = Some(gap_ms);
        inner.worst_source_frame_gap_ms =
            Some(inner.worst_source_frame_gap_ms.unwrap_or(0.0).max(gap_ms));
        inner.total_source_frame_gap_ms += gap_ms;
        inner.source_frame_gap_count = inner.source_frame_gap_count.saturating_add(1);
        inner.last_estimated_frames_missed = Some(estimated_missed);
        inner.estimated_frames_missed_total = inner
            .estimated_frames_missed_total
            .saturating_add(estimated_missed);
        if gap_ms > expected_ms * DIAGNOSTIC_MATERIAL_GAP_INTERVALS {
            inner.material_source_gap_count = inner.material_source_gap_count.saturating_add(1);
        }
        self.changed.notify_all();
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
        inner.last_encoder_creation_ms = Some(diagnostics.encoder_creation_ms);
        inner.worst_encoder_creation_ms = Some(
            inner
                .worst_encoder_creation_ms
                .unwrap_or(0.0)
                .max(diagnostics.encoder_creation_ms),
        );
        inner.total_encoder_creation_ms += diagnostics.encoder_creation_ms;
        inner.rotation_count += 1;
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

    fn request_save_boundary(&self) -> Result<(u64, u64, i64), String> {
        let clock = self
            .lock()
            .session_clock
            .clone()
            .ok_or_else(|| "The replay session clock is unavailable.".to_string())?;
        let anchor_qpc_100ns = clock.now_qpc_100ns()?;
        self.request_save_boundary_at(anchor_qpc_100ns)
    }

    fn request_save_boundary_at(&self, anchor_qpc_100ns: i64) -> Result<(u64, u64, i64), String> {
        let mut inner = self.lock();
        if inner.state != ReplayLifecycleState::Running {
            return Err("Save Replay requires a running replay buffer.".to_string());
        }
        if inner.ring.len() == 0 {
            return Err("No finalized replay video is available yet.".to_string());
        }
        if inner.pending_save_boundary.is_some() {
            return Err("A Save Replay boundary request is already pending.".to_string());
        }

        inner.next_rotation_request_id = inner.next_rotation_request_id.saturating_add(1);
        let request_id = inner.next_rotation_request_id;
        inner.pending_save_boundary = Some(SaveBoundaryRequest {
            request_id,
            anchor_qpc_100ns,
        });
        inner.acknowledged_save_boundary = None;
        let requested_at = unix_timestamp_ms();
        self.changed.notify_all();
        Ok((request_id, requested_at, anchor_qpc_100ns))
    }

    fn pending_save_boundary(&self) -> Option<SaveBoundaryRequest> {
        self.lock().pending_save_boundary
    }

    fn acknowledge_save_boundary(
        &self,
        request: SaveBoundaryRequest,
        sequence_number: u64,
        boundary_qpc_100ns: i64,
    ) {
        let mut inner = self.lock();
        if inner.pending_save_boundary == Some(request)
            && boundary_qpc_100ns >= request.anchor_qpc_100ns
        {
            inner.pending_save_boundary = None;
            inner.acknowledged_save_boundary = Some(AcknowledgedSaveBoundary {
                request_id: request.request_id,
                anchor_qpc_100ns: request.anchor_qpc_100ns,
                final_sequence_number: sequence_number,
            });
        }
        self.changed.notify_all();
    }

    fn wait_for_save_boundary(
        &self,
        request_id: u64,
        timeout: Duration,
    ) -> Result<(u64, i64), String> {
        let deadline = Instant::now() + timeout;
        let mut inner = self.lock();

        loop {
            if let Some(acknowledged) = inner.acknowledged_save_boundary {
                if acknowledged.request_id == request_id
                    && inner
                        .ring
                        .contains_sequence(acknowledged.final_sequence_number)
                {
                    return Ok((
                        acknowledged.final_sequence_number,
                        acknowledged.anchor_qpc_100ns,
                    ));
                }
            }
            if inner.state == ReplayLifecycleState::Error {
                if inner
                    .pending_save_boundary
                    .is_some_and(|request| request.request_id == request_id)
                {
                    inner.pending_save_boundary = None;
                }
                return Err(inner.error_message.clone().unwrap_or_else(|| {
                    "The replay buffer failed while finalizing the save boundary.".to_string()
                }));
            }
            if inner.state != ReplayLifecycleState::Running {
                if inner
                    .pending_save_boundary
                    .is_some_and(|request| request.request_id == request_id)
                {
                    inner.pending_save_boundary = None;
                }
                return Err(format!(
                    "The replay buffer entered {:?} before the save boundary finalized.",
                    inner.state
                ));
            }

            let now = Instant::now();
            if now >= deadline {
                if inner
                    .pending_save_boundary
                    .is_some_and(|request| request.request_id == request_id)
                {
                    inner.pending_save_boundary = None;
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
        save_request_qpc_100ns: i64,
        video_boundary_wait_ms: f64,
        save_started: Instant,
    ) -> Result<ReplaySaveSnapshot, String> {
        let (
            segments,
            requested_duration_seconds,
            capture_target_label,
            capture_target_type,
            overlay_monitor_origin,
            sequence_numbers,
        ) = {
            let mut inner = self.lock();
            if inner.state != ReplayLifecycleState::Running {
                return Err(
                    "Replay capture changed state before the save snapshot could be pinned."
                        .to_string(),
                );
            }
            let requested_duration_seconds = inner.replay_duration_seconds;
            let segments = inner.ring.select_suffix_through(
                final_sequence_number,
                u64::from(requested_duration_seconds) * 1_000,
            );
            if segments.is_empty() {
                return Err("No finalized replay segments were available to save.".to_string());
            }
            if !segments.last().is_some_and(|segment| {
                segment.segment_session_end_qpc_100ns >= save_request_qpc_100ns
            }) {
                return Err(
                    "The finalized video snapshot does not cover the immutable Save Replay QPC anchor."
                        .to_string(),
                );
            }

            let sequence_numbers = segments
                .iter()
                .map(|segment| segment.sequence_number)
                .collect::<Vec<_>>();
            for sequence_number in &sequence_numbers {
                *inner.pins.entry(*sequence_number).or_insert(0) += 1;
            }
            (
                segments,
                requested_duration_seconds,
                inner.target_label.clone(),
                inner.target_type.clone(),
                inner
                    .dxgi_startup
                    .desktop_x
                    .zip(inner.dxgi_startup.desktop_y),
                sequence_numbers,
            )
        };

        let video_pins = SegmentPinGuard {
            shared: Arc::clone(self),
            sequence_numbers,
        };
        let video_timeline = SavedReplayTimeline::from_segments(&segments)?;
        debug_assert_eq!(
            video_timeline.video_pts_to_clip_100ns(segments[0].sequence_number, 0),
            Some(0)
        );
        let audio = self
            .audio
            .wait_for_coverage_and_plan(&video_timeline, AUDIO_SAVE_BARRIER_TIMEOUT)
            .map_err(|failure| failure.message)?;

        Ok(ReplaySaveSnapshot {
            save_request_timestamp_ms,
            save_request_qpc_100ns,
            requested_duration_seconds,
            capture_target_label,
            capture_target_type,
            overlay_monitor_origin,
            segments,
            video_timeline,
            audio_snapshot_plans: audio.plans,
            audio_snapshot_tracks: audio.tracks,
            audio_save_barriers: audio.barriers,
            video_boundary_wait_ms,
            audio_barrier_wait_ms: audio.wait_duration.as_secs_f64() * 1_000.0,
            snapshot_ready_latency_ms: save_started.elapsed().as_secs_f64() * 1_000.0,
            _pins: Some(video_pins),
            _audio_pins: Some(audio.pins),
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ReplayStartDisposition {
    StartNew,
    AlreadyActive,
    WorkerStillStopping,
}

fn replay_start_disposition(
    state: ReplayLifecycleState,
    worker_present: bool,
) -> ReplayStartDisposition {
    if state.is_active() {
        ReplayStartDisposition::AlreadyActive
    } else if state.can_start() && !worker_present {
        ReplayStartDisposition::StartNew
    } else {
        ReplayStartDisposition::WorkerStillStopping
    }
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
        match replay_start_disposition(current.state, worker.is_some()) {
            ReplayStartDisposition::AlreadyActive => {
                return ReplayCommandResult::success(current)
            }
            ReplayStartDisposition::WorkerStillStopping => {
                return ReplayCommandResult::failure(
                    current,
                    "The previous Replay worker is still stopping. Wait for it to finish before starting again.",
                )
            }
            ReplayStartDisposition::StartNew => {}
        }
        if self.shared.has_pins() {
            return ReplayCommandResult::failure(
                current,
                "A replay is still being saved. Wait for it to finish before starting a new buffer session.",
            );
        }

        let initial_target = match resolve_target(&request.target) {
            Ok(target) => target,
            Err(error) => return ReplayCommandResult::failure(self.status(), error),
        };
        let initial_presentation = initial_target.presentation;
        let initial_plan = plan_capture(request.capture_mode, request.target.target_type);

        if let Err(error) = cleanup_session_directories(&self.root) {
            return ReplayCommandResult::failure(self.status(), error);
        }

        self.shared
            .begin_with_plan(&request, initial_presentation, initial_plan);
        let shared = Arc::clone(&self.shared);
        let root = Arc::clone(&self.root);
        let thread = match thread::Builder::new()
            .name("slickclip-buffer".to_string())
            .spawn(move || {
                let _worker = VideoWorkerGuard::new(&shared);
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

        ReplayCommandResult::started(self.status())
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
        let save_started = Instant::now();
        self.shared.audio.clear_save_barriers();
        let (request_id, requested_at, requested_qpc_100ns) =
            self.shared.request_save_boundary()?;
        let (final_sequence_number, acknowledged_qpc_100ns) = self
            .shared
            .wait_for_save_boundary(request_id, Duration::from_secs(15))?;
        debug_assert_eq!(requested_qpc_100ns, acknowledged_qpc_100ns);
        let video_boundary_wait_ms = save_started.elapsed().as_secs_f64() * 1_000.0;
        self.shared.pin_snapshot(
            final_sequence_number,
            requested_at,
            acknowledged_qpc_100ns,
            video_boundary_wait_ms,
            save_started,
        )
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
    let ResolvedCaptureTarget {
        target,
        label,
        presentation,
        ..
    } = resolve_target(&request.target)?;
    let presentation_class = presentation;
    let plan = plan_capture(request.capture_mode, request.target.target_type);
    shared.set_capture_selection(
        presentation_class,
        plan.active_mode,
        plan.selection_reason,
        "Probing",
        None,
    );
    let output = ffmpeg_output_for_target(&target)?;
    let output_index = output.output_index;
    let output_identity = output.identity;
    let width = output.width;
    let height = output.height;
    let width = even_dimension(width)?;
    let height = even_dimension(height)?;
    let capability_result = resolve_capture_capabilities(
        request.encoder,
        output.adapter_index,
        output_index,
        request.frame_rate,
    );
    let (ffmpeg, capabilities) = match capability_result {
        Ok(result) => result,
        Err(_) if shared.should_stop() => {
            shared.mark_stopped();
            return Ok(());
        }
        Err(error) => return Err(error),
    };
    if shared.should_stop() {
        shared.mark_stopped();
        return Ok(());
    }
    let session_clock = ReplaySessionClock::create()?;
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
        capabilities.encoder.codec,
        width,
        height,
        session_id,
        session_directory.clone(),
        session_clock.clone(),
    );
    shared.set_ffmpeg_output_diagnostics(
        output_identity.clone(),
        output.adapter_index,
        output_index,
        output.desktop_x,
        output.desktop_y,
        width,
        height,
    );
    debug_assert!(capabilities.ddagrab_compiled);
    shared.set_ffmpeg_capability_diagnostics(
        &capabilities.program,
        capabilities.encoder.display_name(),
        &capabilities.encoder_probe_failures,
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
    let capture_plan = FfmpegCapturePlan {
        adapter_index: output.adapter_index,
        output_index,
        output_identity,
        width,
        height,
        frame_rate: request.frame_rate,
        quality_qp: request.quality.qp(),
        encoder: capabilities.encoder,
        session_directory,
    };
    let capture_start_qpc_100ns = session_clock.now_qpc_100ns()?;
    let mut capture =
        match FfmpegReplayCapture::start(ffmpeg, capture_plan, capture_start_qpc_100ns) {
            Ok(capture) => capture,
            Err(error) => {
                audio_session.stop_and_wait();
                if shared.should_stop() {
                    shared.mark_stopped();
                    return Ok(());
                }
                return Err(error);
            }
        };
    shared
        .callback_telemetry
        .ffmpeg_capture_processes
        .store(1, Ordering::Relaxed);
    let mut awaiting_restart_segment = false;
    let capture_result = (|| {
        while !shared.should_stop() {
            let now_qpc_100ns = session_clock.now_qpc_100ns()?;
            let poll = capture.poll(now_qpc_100ns)?;
            let first_real_segment = poll.first_real_segment;
            let child_restarted_this_poll = poll.child_restarted;
            shared
                .callback_telemetry
                .ffmpeg_capture_processes
                .store(u64::from(capture.has_live_child()), Ordering::Relaxed);
            if child_restarted_this_poll {
                let restart_count = capture.restart_count();
                shared
                    .callback_telemetry
                    .ffmpeg_capture_restarts
                    .store(u64::from(restart_count), Ordering::Relaxed);
                shared.begin_ffmpeg_recovery(
                    restart_count,
                    format!(
                        "The owned FFmpeg capture process exited. SlickClip is performing bounded restart {restart_count} on the same display; native audio workers and the logical Replay clock remain active."
                    ),
                );
                awaiting_restart_segment = true;
            }
            for segment in poll.completed_segments {
                let pending = shared.pending_save_boundary();
                let sequence_number = segment.sequence_number;
                let boundary_qpc_100ns = segment.segment_session_end_qpc_100ns;
                shared.record_ffmpeg_segment_metrics(&segment);
                shared.complete_segment(segment);
                if let Some(request) = pending {
                    shared.acknowledge_save_boundary(request, sequence_number, boundary_qpc_100ns);
                }
                if shared.snapshot().state == ReplayLifecycleState::Starting {
                    if awaiting_restart_segment && !child_restarted_this_poll {
                        shared.complete_ffmpeg_recovery(capture.restart_count());
                        awaiting_restart_segment = false;
                    } else if first_real_segment && !awaiting_restart_segment {
                        shared.set_capture_health(VideoBackend::FfmpegDdagrab, "Healthy", None);
                        shared.mark_running();
                    }
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
        let tail = capture.stop_and_finalize()?;
        shared
            .callback_telemetry
            .ffmpeg_capture_processes
            .store(0, Ordering::Relaxed);
        for segment in tail {
            shared.record_ffmpeg_segment_metrics(&segment);
            shared.complete_segment(segment);
        }
        Ok(())
    })();
    shared
        .callback_telemetry
        .ffmpeg_capture_processes
        .store(0, Ordering::Relaxed);
    audio_session.stop_and_wait();

    match capture_result {
        Ok(()) => {
            shared.mark_stopped();
            Ok(())
        }
        Err(_)
            if shared.should_stop() && shared.snapshot().state != ReplayLifecycleState::Error =>
        {
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
    clock: ReplaySessionClock,
    scheduler_discontinuity: Arc<SchedulerDiscontinuity>,
    capture_fallback_reason: Option<String>,
}

struct PreparedDxgiCapture {
    api: Option<DxgiDuplicationApi>,
    startup_frame: Option<DetachedFrame>,
    recreation_context: Option<DxgiDuplicationRecreationContext>,
    monitor: Monitor,
    output_identity: String,
    desktop_origin: (i32, i32),
    pending_recovery: Option<DxgiRecoveryReport>,
    _session: DxgiSessionGuard,
}

impl PreparedDxgiCapture {
    fn api(&self) -> Result<&DxgiDuplicationApi, String> {
        self.api
            .as_ref()
            .ok_or_else(|| "The Display Capture duplication object is unavailable.".to_string())
    }

    fn api_mut(&mut self) -> Result<&mut DxgiDuplicationApi, String> {
        self.api
            .as_mut()
            .ok_or_else(|| "The Display Capture duplication object is unavailable.".to_string())
    }

    fn suspend_invalid_duplication(&mut self) {
        if self.recreation_context.is_none() {
            self.recreation_context = self
                .api
                .take()
                .map(DxgiDuplicationApi::into_recreation_context);
        }
    }
}

#[derive(Clone, Copy)]
struct DxgiRecoveryReport {
    elapsed: Duration,
    recreation_attempts: u64,
}

enum DxgiHealthValidationFailure {
    AccessLost,
    Stopped,
    Fatal(String),
}

#[derive(Default)]
struct DxgiHealthProbeStats {
    acquire_results: u64,
    acquired_frames: u64,
    timeouts: u64,
    access_losses: u64,
    invalid_textures: u64,
    cpu_readbacks: u64,
    last_cpu_readback_ms: Option<f64>,
    valid_samples: u64,
    last_acquire_result: Option<String>,
    last_valid_frame_elapsed_ms: Option<f64>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DxgiRecoveryStabilityKey {
    output_identity: String,
    desktop_origin: (i32, i32),
    width: u32,
    height: u32,
}

struct DxgiRecoveryObservation {
    key: DxgiRecoveryStabilityKey,
    monitor: Monitor,
    desktop_origin: (i32, i32),
}

#[derive(Default)]
struct DxgiRecoveryStabilityTracker {
    last: Option<DxgiRecoveryStabilityKey>,
    consecutive_polls: u8,
}

impl DxgiRecoveryStabilityTracker {
    fn observe(&mut self, key: DxgiRecoveryStabilityKey) -> bool {
        if self.last.as_ref() == Some(&key) {
            self.consecutive_polls = self.consecutive_polls.saturating_add(1);
        } else {
            self.last = Some(key);
            self.consecutive_polls = 1;
        }
        self.consecutive_polls >= DXGI_RECOVERY_STABLE_POLLS
    }

    fn reset(&mut self) {
        self.last = None;
        self.consecutive_polls = 0;
    }
}

struct DxgiRecoveryCoordinator {
    started: Instant,
    deadline: Instant,
    next_recreation_at: Instant,
    stability: DxgiRecoveryStabilityTracker,
    recreation_attempts: u64,
    last_error: String,
}

impl DxgiRecoveryCoordinator {
    fn new(now: Instant) -> Self {
        Self {
            started: now,
            deadline: now + DXGI_RECOVERY_DEADLINE,
            next_recreation_at: now,
            stability: DxgiRecoveryStabilityTracker::default(),
            recreation_attempts: 0,
            last_error: "Windows has not exposed the selected Display Capture display yet."
                .to_string(),
        }
    }

    fn elapsed(&self, now: Instant) -> Duration {
        now.saturating_duration_since(self.started)
    }

    fn expired(&self, now: Instant) -> bool {
        now >= self.deadline
    }

    fn observe(&mut self, now: Instant, key: DxgiRecoveryStabilityKey) -> bool {
        self.stability.observe(key) && now >= self.next_recreation_at
    }

    fn record_observation_failure(&mut self, error: String) {
        self.last_error = error;
        self.stability.reset();
    }

    fn begin_recreation(&mut self) -> u64 {
        self.recreation_attempts = self.recreation_attempts.saturating_add(1);
        self.recreation_attempts
    }

    fn record_recreation_failure(&mut self, now: Instant, error: String) {
        self.last_error = error;
        self.next_recreation_at = now + DXGI_RECOVERY_RECREATE_BACKOFF;
    }

    fn record_access_lost_during_validation(&mut self, now: Instant) {
        self.last_error =
            "Display Capture access was lost again while validating the recreated display."
                .to_string();
        self.next_recreation_at = now + DXGI_RECOVERY_RECREATE_BACKOFF;
        self.stability.reset();
    }
}

fn prepare_dxgi_capture(
    shared: &Arc<SharedReplay>,
    target: &NativeCaptureTarget,
) -> Result<PreparedDxgiCapture, String> {
    let selected_monitor = dxgi_monitor_for_target(target)?;
    let output_identity = selected_monitor
        .device_name()
        .map_err(|error| format!("Could not identify the Display Capture display: {error}"))?;
    let stable_observation = wait_for_stable_dxgi_startup_display(shared, &output_identity)?;
    let monitor = stable_observation.monitor;
    let desktop_origin = stable_observation.desktop_origin;
    let session = DxgiSessionGuard::new(shared);
    let api = DxgiDuplicationApi::new_options(monitor, &[DxgiDuplicationFormat::Bgra8])
        .map_err(|error| format!("Could not start Display Capture: {error}"))?;
    let adapter_identity =
        dxgi_adapter_identity(&api).unwrap_or_else(|error| format!("Unavailable ({error})"));
    shared.set_dxgi_output_diagnostics(&stable_observation.key, adapter_identity);
    let mut prepared = PreparedDxgiCapture {
        api: Some(api),
        startup_frame: None,
        recreation_context: None,
        monitor,
        output_identity,
        desktop_origin,
        pending_recovery: None,
        _session: session,
    };
    match validate_dxgi_health(
        shared,
        &mut prepared,
        Instant::now() + DXGI_HEALTH_VALIDATION_LIMIT,
    ) {
        Ok(()) => {}
        Err(DxgiHealthValidationFailure::AccessLost) => {
            shared.begin_dxgi_recovery();
            shared.scheduler_discontinuity.pause();
            prepared.suspend_invalid_duplication();
            prepared.pending_recovery = Some(recover_dxgi_duplication(shared, &mut prepared)?);
        }
        Err(DxgiHealthValidationFailure::Stopped) => {
            return Err("Display Capture health validation stopped with Replay.".to_string())
        }
        Err(DxgiHealthValidationFailure::Fatal(error)) => return Err(error),
    }
    Ok(prepared)
}

fn wait_for_stable_dxgi_startup_display(
    shared: &Arc<SharedReplay>,
    output_identity: &str,
) -> Result<DxgiRecoveryObservation, String> {
    let deadline = Instant::now() + DXGI_STARTUP_STABILITY_DEADLINE;
    let mut stability = DxgiRecoveryStabilityTracker::default();
    let mut last_error = "Windows has not exposed a stable selected display yet.".to_string();
    while Instant::now() < deadline && !shared.should_stop() {
        match observe_dxgi_recovery_display(output_identity) {
            Ok(observation) => {
                last_error = format!(
                    "display geometry is still changing ({}x{} at {}, {})",
                    observation.key.width,
                    observation.key.height,
                    observation.key.desktop_origin.0,
                    observation.key.desktop_origin.1,
                );
                if stability.observe(observation.key.clone()) {
                    return Ok(observation);
                }
            }
            Err(error) => {
                last_error = error;
                stability.reset();
            }
        }
        let poll_deadline = (Instant::now() + DXGI_STARTUP_STABILITY_POLL_INTERVAL).min(deadline);
        while Instant::now() < poll_deadline {
            if shared.should_stop() {
                return Err("Display Capture startup stopped with Replay.".to_string());
            }
            thread::sleep(Duration::from_millis(10));
        }
    }
    if shared.should_stop() {
        Err("Display Capture startup stopped with Replay.".to_string())
    } else {
        Err(format!(
            "Display Capture waited {:.0} seconds for the selected display to stabilize before creating Desktop Duplication: {last_error}",
            DXGI_STARTUP_STABILITY_DEADLINE.as_secs_f64(),
        ))
    }
}

fn dxgi_adapter_identity(api: &DxgiDuplicationApi) -> Result<String, String> {
    let adapter = unsafe { api.dxgi_device().GetAdapter() }
        .map_err(|error| format!("could not resolve adapter: {error}"))?;
    let description = unsafe { adapter.GetDesc() }
        .map_err(|error| format!("could not read adapter description: {error}"))?;
    let length = description
        .Description
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(description.Description.len());
    let name = String::from_utf16_lossy(&description.Description[..length]);
    if name.trim().is_empty() {
        Err("Windows returned an empty adapter description".to_string())
    } else {
        Ok(name)
    }
}

fn validate_dxgi_health(
    shared: &Arc<SharedReplay>,
    prepared: &mut PreparedDxgiCapture,
    validation_deadline: Instant,
) -> Result<(), DxgiHealthValidationFailure> {
    let probe_started = Instant::now();
    let deadline = (probe_started + CAPTURE_HEALTH_PROBE_DURATION).min(validation_deadline);
    let mut stats = DxgiHealthProbeStats::default();
    let mut packed = Vec::new();
    while Instant::now() < deadline && !shared.should_stop() {
        let api = prepared
            .api_mut()
            .map_err(DxgiHealthValidationFailure::Fatal)?;
        match api.acquire_next_frame(200) {
            Ok(mut frame) => {
                stats.acquire_results = stats.acquire_results.saturating_add(1);
                stats.acquired_frames = stats.acquired_frames.saturating_add(1);
                stats.last_acquire_result = Some("frame".to_string());
                let width = frame.width();
                let height = frame.height();
                let detached = DetachedFrame::new_gdi_render_target(
                    frame.device(),
                    frame.device_context(),
                    width,
                    height,
                    ColorFormat::Bgra8,
                )
                .map_err(|error| {
                    DxgiHealthValidationFailure::Fatal(format!(
                        "Could not preserve the validated Display Capture startup frame: {error}"
                    ))
                })?;
                unsafe {
                    frame
                        .device_context()
                        .CopyResource(detached.as_raw_texture(), frame.texture());
                }
                stats.cpu_readbacks = stats.cpu_readbacks.saturating_add(1);
                let cpu_readback_started = Instant::now();
                let buffer = match frame.buffer() {
                    Ok(buffer) => buffer,
                    Err(error) => {
                        stats.invalid_textures = stats.invalid_textures.saturating_add(1);
                        stats.last_acquire_result = Some("invalid texture".to_string());
                        shared.record_dxgi_health_probe(&stats);
                        return Err(DxgiHealthValidationFailure::Fatal(format!(
                            "Could not inspect a Display Capture startup frame: {error}"
                        )));
                    }
                };
                let bytes = buffer.as_nopadding_buffer(&mut packed);
                let Some(sample) =
                    FrameHealthSample::from_bgra(bytes, width, height, width.saturating_mul(4))
                else {
                    stats.invalid_textures = stats.invalid_textures.saturating_add(1);
                    stats.last_acquire_result = Some("invalid texture".to_string());
                    shared.record_dxgi_health_probe(&stats);
                    return Err(DxgiHealthValidationFailure::Fatal(
                        "Display Capture returned an invalid startup texture.".to_string(),
                    ));
                };
                stats.last_cpu_readback_ms =
                    Some(cpu_readback_started.elapsed().as_secs_f64() * 1_000.0);
                stats.valid_samples = stats.valid_samples.saturating_add(1);
                stats.last_valid_frame_elapsed_ms =
                    Some(probe_started.elapsed().as_secs_f64() * 1_000.0);
                drop(buffer);
                drop(frame);
                shared.record_dxgi_health_probe(&stats);
                return match classify_dxgi_health_samples(&[sample]) {
                    CaptureHealth::Healthy => {
                        prepared.startup_frame = Some(detached);
                        Ok(())
                    }
                    _ => Err(DxgiHealthValidationFailure::Fatal(
                        "Display Capture returned an unusable startup texture.".to_string(),
                    )),
                };
            }
            Err(DxgiError::Timeout) => {
                stats.acquire_results = stats.acquire_results.saturating_add(1);
                stats.timeouts = stats.timeouts.saturating_add(1);
                stats.last_acquire_result = Some("timeout (no desktop update)".to_string());
            }
            Err(DxgiError::AccessLost) => {
                stats.acquire_results = stats.acquire_results.saturating_add(1);
                stats.access_losses = stats.access_losses.saturating_add(1);
                stats.last_acquire_result = Some("access lost".to_string());
                shared.record_dxgi_health_probe(&stats);
                return Err(DxgiHealthValidationFailure::AccessLost);
            }
            Err(error) => {
                stats.acquire_results = stats.acquire_results.saturating_add(1);
                stats.last_acquire_result = Some(format!("API failure: {error}"));
                shared.record_dxgi_health_probe(&stats);
                return Err(DxgiHealthValidationFailure::Fatal(format!(
                    "Display Capture health check failed: {error}"
                )));
            }
        }
    }
    if shared.should_stop() {
        shared.record_dxgi_health_probe(&stats);
        return Err(DxgiHealthValidationFailure::Stopped);
    }
    shared.record_dxgi_health_probe(&stats);
    Err(DxgiHealthValidationFailure::Fatal(
        "Display Capture did not provide a valid desktop frame before its health deadline."
            .to_string(),
    ))
}

fn classify_dxgi_health_samples(samples: &[FrameHealthSample]) -> CaptureHealth {
    // Desktop Duplication reports desktop updates, not a continuous stream. A single readable
    // texture establishes that Display Capture is valid; static and legitimately dark desktops
    // must not be rejected as frozen or black. Known failures remain API/access-loss/texture
    // errors, while zero valid frames remains unhealthy.
    if samples.is_empty() || samples.iter().any(|sample| sample.sampled_pixels == 0) {
        CaptureHealth::Invalid
    } else {
        CaptureHealth::Healthy
    }
}

fn recover_dxgi_duplication(
    shared: &Arc<SharedReplay>,
    prepared: &mut PreparedDxgiCapture,
) -> Result<DxgiRecoveryReport, String> {
    prepared.suspend_invalid_duplication();
    let mut coordinator = DxgiRecoveryCoordinator::new(Instant::now());

    loop {
        let now = Instant::now();
        shared.update_dxgi_recovery(coordinator.elapsed(now), coordinator.recreation_attempts);
        if shared.should_stop() {
            return Err("Display Capture recovery stopped with Replay.".to_string());
        }
        if coordinator.expired(now) {
            return Err(format!(
                "Display Capture recovery could not reacquire the same display within {:.0} seconds after {} recreation attempts: {}",
                DXGI_RECOVERY_DEADLINE.as_secs_f64(),
                coordinator.recreation_attempts,
                coordinator.last_error,
            ));
        }

        let observation = match observe_dxgi_recovery_display(&prepared.output_identity) {
            Ok(observation) => observation,
            Err(error) => {
                coordinator.record_observation_failure(error);
                if !wait_for_dxgi_recovery_poll(shared, &coordinator) {
                    return Err("Display Capture recovery stopped with Replay.".to_string());
                }
                continue;
            }
        };

        if !coordinator.observe(now, observation.key.clone()) {
            if !wait_for_dxgi_recovery_poll(shared, &coordinator) {
                return Err("Display Capture recovery stopped with Replay.".to_string());
            }
            continue;
        }

        let attempt = coordinator.begin_recreation();
        shared.update_dxgi_recovery(coordinator.elapsed(now), attempt);
        if let Err(error) = recreate_dxgi_duplication(prepared, observation, attempt) {
            coordinator.record_recreation_failure(Instant::now(), error);
            if !wait_for_dxgi_recovery_poll(shared, &coordinator) {
                return Err("Display Capture recovery stopped with Replay.".to_string());
            }
            continue;
        }
        shared
            .callback_telemetry
            .dxgi_duplication_recreations
            .fetch_add(1, Ordering::Relaxed);

        match validate_dxgi_health(shared, prepared, coordinator.deadline) {
            Ok(()) => {
                return Ok(DxgiRecoveryReport {
                    elapsed: coordinator.elapsed(Instant::now()),
                    recreation_attempts: coordinator.recreation_attempts,
                })
            }
            Err(DxgiHealthValidationFailure::AccessLost) => {
                prepared.suspend_invalid_duplication();
                coordinator.record_access_lost_during_validation(Instant::now());
            }
            Err(DxgiHealthValidationFailure::Stopped) => {
                return Err("Display Capture recovery stopped with Replay.".to_string())
            }
            Err(DxgiHealthValidationFailure::Fatal(error)) => return Err(error),
        }
    }
}

fn observe_dxgi_recovery_display(output_identity: &str) -> Result<DxgiRecoveryObservation, String> {
    let monitor = Monitor::enumerate()
        .map_err(|error| {
            format!("Could not enumerate displays during Display Capture recovery: {error}")
        })?
        .into_iter()
        .find(|monitor| {
            monitor
                .device_name()
                .is_ok_and(|name| recovery_output_matches_selected_display(output_identity, &name))
        })
        .ok_or_else(|| {
            format!("The selected display '{output_identity}' is not currently exposed by Windows.")
        })?;
    recovery_observation_for_monitor(monitor)
}

fn recovery_output_matches_selected_display(selected: &str, observed: &str) -> bool {
    selected == observed
}

fn recovery_observation_for_monitor(monitor: Monitor) -> Result<DxgiRecoveryObservation, String> {
    let output_identity = monitor.device_name().map_err(|error| {
        format!("Could not identify the recovering Display Capture display: {error}")
    })?;
    let desktop_origin = monitor_desktop_origin(monitor)?;
    let width = monitor
        .width()
        .map_err(|error| format!("Could not read the recovering output width: {error}"))?;
    let height = monitor
        .height()
        .map_err(|error| format!("Could not read the recovering output height: {error}"))?;
    Ok(DxgiRecoveryObservation {
        key: DxgiRecoveryStabilityKey {
            output_identity,
            desktop_origin,
            width,
            height,
        },
        monitor,
        desktop_origin,
    })
}

fn recreate_dxgi_duplication(
    prepared: &mut PreparedDxgiCapture,
    observation: DxgiRecoveryObservation,
    attempt: u64,
) -> Result<(), String> {
    let api = if attempt <= DXGI_RECOVERY_RETAINED_DEVICE_ATTEMPTS {
        if let Some(context) = prepared.recreation_context.as_ref() {
            context
                .recreate_for_monitor_options(
                    observation.monitor,
                    &[DxgiDuplicationFormat::Bgra8],
                )
                .map_err(|error| {
                    format!(
                        "Could not recreate Display Capture with the retained graphics device: {error}"
                    )
                })?
        } else {
            DxgiDuplicationApi::new_options(observation.monitor, &[DxgiDuplicationFormat::Bgra8])
                .map_err(|error| format!("Could not rebuild Display Capture resources: {error}"))?
        }
    } else {
        prepared.recreation_context.take();
        DxgiDuplicationApi::new_options(observation.monitor, &[DxgiDuplicationFormat::Bgra8])
            .map_err(|error| format!("Could not rebuild Display Capture resources: {error}"))?
    };
    prepared.recreation_context = None;
    prepared.api = Some(api);
    prepared.monitor = observation.monitor;
    prepared.output_identity = observation.key.output_identity;
    prepared.desktop_origin = observation.desktop_origin;
    Ok(())
}

fn wait_for_dxgi_recovery_poll(
    shared: &Arc<SharedReplay>,
    coordinator: &DxgiRecoveryCoordinator,
) -> bool {
    let deadline =
        (Instant::now() + DXGI_RECOVERY_STABILITY_POLL_INTERVAL).min(coordinator.deadline);
    while Instant::now() < deadline {
        if shared.should_stop() {
            return false;
        }
        thread::sleep(Duration::from_millis(10));
    }
    true
}

struct SourceFrame {
    frame: DetachedFrame,
    source_qpc_100ns: i64,
    generation: u64,
}

#[derive(Default)]
struct SourceFrameStore {
    pending: VecDeque<SourceFrame>,
    current: Option<SourceFrame>,
    next_generation: u64,
    superseded_before_sampling: u64,
    first_superseded_qpc_100ns: Option<i64>,
    last_superseded_qpc_100ns: Option<i64>,
}

#[derive(Clone, Copy)]
struct SourceSelection {
    source_qpc_100ns: i64,
    first_consumed_source_qpc_100ns: Option<i64>,
    generation: u64,
    consumed_updates: u64,
    superseded_updates: u64,
}

impl SourceFrameStore {
    fn update_detached(&mut self, detached: DetachedFrame, source_qpc_100ns: i64) {
        self.next_generation = self.next_generation.saturating_add(1);
        self.pending.push_back(SourceFrame {
            frame: detached,
            source_qpc_100ns,
            generation: self.next_generation,
        });
        while self.pending.len() > MAX_PENDING_SOURCE_FRAMES {
            if let Some(superseded) = self.pending.pop_front() {
                self.first_superseded_qpc_100ns = self
                    .first_superseded_qpc_100ns
                    .or(Some(superseded.source_qpc_100ns));
                self.last_superseded_qpc_100ns = Some(superseded.source_qpc_100ns);
                self.superseded_before_sampling = self.superseded_before_sampling.saturating_add(1);
            }
        }
    }

    fn select(&mut self, output_qpc_100ns: i64) -> Option<SourceSelection> {
        let mut newest = None;
        let overflow_is_due = self
            .last_superseded_qpc_100ns
            .is_some_and(|qpc| qpc <= output_qpc_100ns);
        let overflow_superseded = if overflow_is_due {
            self.last_superseded_qpc_100ns = None;
            std::mem::take(&mut self.superseded_before_sampling)
        } else {
            0
        };
        let mut first_consumed_source_qpc_100ns = overflow_is_due
            .then(|| self.first_superseded_qpc_100ns.take())
            .flatten();
        let due_updates = due_source_update_count(
            self.pending.iter().map(|frame| frame.source_qpc_100ns),
            output_qpc_100ns,
        );
        let mut consumed = 0u64;
        for _ in 0..due_updates {
            newest = self.pending.pop_front();
            first_consumed_source_qpc_100ns = first_consumed_source_qpc_100ns
                .or_else(|| newest.as_ref().map(|frame| frame.source_qpc_100ns));
            consumed = consumed.saturating_add(1);
        }
        if let Some(newest) = newest {
            self.current = Some(newest);
        }
        self.current.as_ref().map(|current| SourceSelection {
            source_qpc_100ns: current.source_qpc_100ns,
            first_consumed_source_qpc_100ns,
            generation: current.generation,
            consumed_updates: consumed.saturating_add(overflow_superseded),
            superseded_updates: consumed
                .saturating_sub(1)
                .saturating_add(overflow_superseded),
        })
    }

    fn current_frame(&self) -> Option<&DetachedFrame> {
        self.current.as_ref().map(|source| &source.frame)
    }
}

struct ActiveSegment {
    sequence_number: u64,
    path: PathBuf,
    backend: Box<dyn VideoEncoderBackend>,
    segment_session_start_qpc_100ns: Option<i64>,
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
    source_update_count: u64,
    fresh_output_frame_count: u64,
    held_output_frame_count: u64,
}

struct FrameEncodeResult {
    send_duration: Duration,
    telemetry: EncoderFrameTelemetry,
}

#[derive(Default)]
struct QueueFullRecovery {
    retrying_output: bool,
}

impl QueueFullRecovery {
    fn record_refusal(&mut self) {
        self.retrying_output = true;
    }

    fn record_acceptance(&mut self) -> bool {
        std::mem::take(&mut self.retrying_output)
    }
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
            segment_session_start_qpc_100ns: None,
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
            source_update_count: 0,
            fresh_output_frame_count: 0,
            held_output_frame_count: 0,
        })
    }

    fn should_rotate(&self, frame_rate: u32) -> bool {
        normal_rotation_due(self.frame_count, frame_rate)
    }

    fn has_frames(&self) -> bool {
        self.frame_count > 0
    }

    fn discard(self) {
        let path = self.path.clone();
        drop(self.backend);
        let _ = fs::remove_file(path);
    }

    fn should_prepare(&self, frame_rate: u32) -> bool {
        normal_prewarm_due(self.frame_count, frame_rate)
    }

    fn encode_frame(
        &mut self,
        frame: &DetachedFrame,
        output_qpc_100ns: i64,
        source_qpc_100ns: i64,
        first_consumed_source_qpc_100ns: Option<i64>,
        fresh_source: bool,
        consumed_source_updates: u64,
        session_started: Instant,
        frame_rate: u32,
    ) -> Result<FrameEncodeResult, String> {
        let encoded_pts_100ns =
            ((i128::from(self.frame_count) * 10_000_000) / i128::from(frame_rate.max(1))) as i64;
        let send_started = Instant::now();
        let encoded = self
            .backend
            .encode_detached_frame(frame, encoded_pts_100ns)
            .map_err(|error| format!("Replay encoder rejected a scheduled CFR frame: {error}"))?;
        let send_duration = send_started.elapsed();
        let submitted_ms = session_started.elapsed().as_secs_f64() * 1_000.0;

        if !encoded.telemetry.queued {
            return Ok(FrameEncodeResult {
                send_duration,
                telemetry: encoded.telemetry,
            });
        }

        if self.segment_session_start_qpc_100ns.is_none() {
            self.segment_session_start_qpc_100ns = Some(output_qpc_100ns);
            self.first_frame_timestamp =
                Some(first_consumed_source_qpc_100ns.unwrap_or(source_qpc_100ns));
            self.start_timestamp_ms = Some(unix_timestamp_ms());
            self.first_frame_submitted_ms = Some(submitted_ms);
        }
        self.last_frame_timestamp = Some(source_qpc_100ns);
        self.last_frame_submitted_ms = Some(submitted_ms);
        self.frame_timing_points.push(VideoFrameTimingPoint {
            frame_index: self.frame_count,
            output_qpc_100ns,
            source_qpc_100ns,
            encoded_pts_100ns,
            fresh_source,
        });
        self.source_update_count = self
            .source_update_count
            .saturating_add(consumed_source_updates);
        if fresh_source {
            self.fresh_output_frame_count = self.fresh_output_frame_count.saturating_add(1);
        } else {
            self.held_output_frame_count = self.held_output_frame_count.saturating_add(1);
        }
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
        let actual_duration_100ns = cfr_duration_100ns(self.frame_count, flags.frame_rate);
        let actual_duration_ms = u64::try_from(actual_duration_100ns.max(0) / 10_000)
            .unwrap_or(0)
            .max(1);
        let start_timestamp_ms = self.start_timestamp_ms.unwrap_or_else(unix_timestamp_ms);
        let segment_session_start_qpc_100ns = self.segment_session_start_qpc_100ns.unwrap_or(0);

        FinalizeJob {
            backend: self.backend,
            path: self.path,
            sequence_number: self.sequence_number,
            start_timestamp_ms,
            end_timestamp_ms: unix_timestamp_ms(),
            actual_duration_ms,
            segment_session_start_qpc_100ns,
            segment_session_end_qpc_100ns: segment_session_start_qpc_100ns
                .saturating_add(actual_duration_100ns),
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
            source_update_count: self.source_update_count,
            fresh_output_frame_count: self.fresh_output_frame_count,
            held_output_frame_count: self.held_output_frame_count,
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
    segment_session_start_qpc_100ns: i64,
    segment_session_end_qpc_100ns: i64,
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
    source_update_count: u64,
    fresh_output_frame_count: u64,
    held_output_frame_count: u64,
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
            .name("slickclip-finalizer".to_string())
            .spawn(move || {
                let _worker = VideoWorkerGuard::new(&worker_shared);
                finalize_segments(receiver, worker_shared);
            })
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
            segment_session_start_qpc_100ns: job.segment_session_start_qpc_100ns,
            segment_session_end_qpc_100ns: job.segment_session_end_qpc_100ns,
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
            source_update_count: job.source_update_count,
            fresh_output_frame_count: job.fresh_output_frame_count,
            held_output_frame_count: job.held_output_frame_count,
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
            average_bitrate_mbps: average_bitrate_mbps(file_size, job.encoded_duration_100ns)
                .unwrap_or(0.0),
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
            .name("slickclip-encoder-prewarm".to_string())
            .spawn(move || {
                let _worker = VideoWorkerGuard::new(&flags.shared);
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
    #[cfg_attr(not(test), allow(dead_code))]
    estimated_frames_missed: u64,
    #[cfg_attr(not(test), allow(dead_code))]
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

struct RealtimeCfrScheduler {
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
    source_store: Arc<Mutex<SourceFrameStore>>,
    video_timeline_start_qpc_100ns: i64,
    next_output_frame_index: u64,
    last_output_source_generation: Option<u64>,
    pending_output_selection: Option<(u64, SourceSelection)>,
    queue_full_recovery: QueueFullRecovery,
    observed_discontinuity_generation: u64,
    rebased_discontinuity_generation: u64,
    pipeline_stop_requested: Arc<AtomicBool>,
}

impl RealtimeCfrScheduler {
    fn run(mut self) -> Result<(), ReplayHandlerError> {
        self.flags
            .shared
            .callback_telemetry
            .video_timeline_start_qpc_100ns
            .store(self.video_timeline_start_qpc_100ns, Ordering::Relaxed);
        let initial_discontinuity = self.flags.scheduler_discontinuity.snapshot();
        if initial_discontinuity.generation == 0 && !initial_discontinuity.paused {
            self.flags.shared.mark_running();
        }
        let telemetry_shared = Arc::clone(&self.flags.shared);

        loop {
            if self.pipeline_stop_requested.load(Ordering::Acquire) {
                break;
            }
            let discontinuity = self.flags.scheduler_discontinuity.snapshot();
            if discontinuity.generation > self.observed_discontinuity_generation {
                let new_discontinuities = discontinuity
                    .generation
                    .saturating_sub(self.observed_discontinuity_generation);
                telemetry_shared
                    .callback_telemetry
                    .scheduler_discontinuities
                    .fetch_add(new_discontinuities, Ordering::Relaxed);
                self.observed_discontinuity_generation = discontinuity.generation;
            }
            if discontinuity.paused {
                if self.flags.shared.should_stop()
                    || self.pipeline_stop_requested.load(Ordering::Acquire)
                {
                    break;
                }
                thread::sleep(Duration::from_millis(2));
                continue;
            }
            if discontinuity.generation > self.rebased_discontinuity_generation {
                let resume_qpc_100ns = discontinuity.resume_qpc_100ns.ok_or_else(|| {
                    ReplayHandlerError::new(
                        "The capture backend resumed without a scheduler rebase timestamp.",
                    )
                })?;
                self.rebase_after_discontinuity(resume_qpc_100ns);
                self.rebased_discontinuity_generation = discontinuity.generation;
                telemetry_shared
                    .callback_telemetry
                    .scheduler_rebases
                    .fetch_add(1, Ordering::Relaxed);
                self.flags.shared.mark_running();
            }
            let now_qpc_100ns = self
                .flags
                .clock
                .now_qpc_100ns()
                .map_err(ReplayHandlerError::new)?;
            let expected_frame_index = expected_cfr_frame_index(
                self.video_timeline_start_qpc_100ns,
                now_qpc_100ns,
                self.flags.frame_rate,
            );
            let telemetry = &telemetry_shared.callback_telemetry;
            telemetry
                .scheduler_expected_output_frame_index
                .store(expected_frame_index, Ordering::Relaxed);

            let next_due_qpc = cfr_frame_qpc(
                self.video_timeline_start_qpc_100ns,
                self.next_output_frame_index,
                self.flags.frame_rate,
            );
            let lateness_100ns = now_qpc_100ns.saturating_sub(next_due_qpc).max(0) as u64;
            telemetry
                .scheduler_current_lateness_100ns
                .store(lateness_100ns, Ordering::Relaxed);
            telemetry
                .scheduler_worst_lateness_100ns
                .fetch_max(lateness_100ns, Ordering::Relaxed);

            let due_count = if self.next_output_frame_index <= expected_frame_index {
                expected_frame_index
                    .saturating_sub(self.next_output_frame_index)
                    .saturating_add(1)
            } else {
                0
            };
            if due_count > MAX_REALTIME_BACKLOG_FRAMES {
                telemetry
                    .missed_realtime_output_frames
                    .fetch_add(due_count, Ordering::Relaxed);
                return Err(ReplayHandlerError::new(format!(
                    "Realtime CFR scheduler fell {due_count} output frames behind; the session cannot preserve realtime video safely."
                )));
            }
            let burst = due_count.min(MAX_CATCH_UP_FRAMES_PER_WAKE);
            let rotation_due_at_wake = self
                .active
                .as_ref()
                .is_some_and(|active| active.should_rotate(self.flags.frame_rate));
            let save_pending_at_wake =
                burst > 1 && self.flags.shared.pending_save_boundary().is_some();

            let mut emitted = 0u64;
            for _ in 0..burst {
                let mut phases = CallbackPhaseDurations::default();
                if !self.emit_output_frame(self.next_output_frame_index, &mut phases)? {
                    break;
                }
                self.next_output_frame_index = self.next_output_frame_index.saturating_add(1);
                emitted = emitted.saturating_add(1);
                self.flags
                    .shared
                    .record_callback_phases(Duration::ZERO, phases);
            }

            if emitted > 1 {
                let catch_up_frames = emitted - 1;
                telemetry
                    .scheduler_catch_up_wakeups
                    .fetch_add(1, Ordering::Relaxed);
                telemetry
                    .scheduler_max_catch_up_burst
                    .fetch_max(emitted, Ordering::Relaxed);
                telemetry
                    .scheduler_catch_up_frames
                    .fetch_add(catch_up_frames, Ordering::Relaxed);
                if rotation_due_at_wake {
                    telemetry
                        .scheduler_rotation_catch_up_frames
                        .fetch_add(catch_up_frames, Ordering::Relaxed);
                }
                if save_pending_at_wake {
                    telemetry
                        .scheduler_save_pending_catch_up_frames
                        .fetch_add(catch_up_frames, Ordering::Relaxed);
                }
            }

            if self.flags.shared.should_stop() {
                if self.next_output_frame_index > expected_frame_index {
                    break;
                }
                continue;
            }
            if emitted < burst || self.next_output_frame_index <= expected_frame_index {
                thread::sleep(Duration::from_millis(1));
                continue;
            }

            let next_qpc = cfr_frame_qpc(
                self.video_timeline_start_qpc_100ns,
                self.next_output_frame_index,
                self.flags.frame_rate,
            );
            let now_qpc = self
                .flags
                .clock
                .now_qpc_100ns()
                .map_err(ReplayHandlerError::new)?;
            let wait_100ns = next_qpc.saturating_sub(now_qpc);
            if wait_100ns > 0 {
                thread::sleep(Duration::from_nanos(
                    u64::try_from(wait_100ns)
                        .unwrap_or(u64::MAX)
                        .saturating_mul(100),
                ));
            } else {
                thread::yield_now();
            }
        }

        self.finish_session()
    }

    fn rebase_after_discontinuity(&mut self, resume_qpc_100ns: i64) {
        self.video_timeline_start_qpc_100ns = rebased_cfr_timeline_start(
            self.video_timeline_start_qpc_100ns,
            self.next_output_frame_index,
            resume_qpc_100ns,
            self.flags.frame_rate,
        );
        self.pending_output_selection = None;
        self.queue_full_recovery = QueueFullRecovery::default();
        let telemetry = &self.flags.shared.callback_telemetry;
        telemetry
            .video_timeline_start_qpc_100ns
            .store(self.video_timeline_start_qpc_100ns, Ordering::Relaxed);
        telemetry
            .scheduler_current_lateness_100ns
            .store(0, Ordering::Relaxed);
        telemetry
            .scheduler_expected_output_frame_index
            .store(self.next_output_frame_index, Ordering::Relaxed);
    }

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

    fn rotate_on_output(
        &mut self,
        frame: &DetachedFrame,
        output_qpc_100ns: i64,
        selection: &SourceSelection,
        fresh_source: bool,
        phases: &mut CallbackPhaseDurations,
    ) -> Result<Option<u64>, ReplayHandlerError> {
        let rotation_started_qpc = self
            .flags
            .clock
            .now_qpc_100ns()
            .map_err(ReplayHandlerError::new)?;
        self.flags
            .shared
            .callback_telemetry
            .last_rotation_lateness_before_100ns
            .store(
                rotation_started_qpc.saturating_sub(output_qpc_100ns).max(0),
                Ordering::Relaxed,
            );
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
            .encode_frame(
                frame,
                output_qpc_100ns,
                selection.source_qpc_100ns,
                selection.first_consumed_source_qpc_100ns,
                fresh_source,
                selection.consumed_updates,
                self.session_started,
                self.flags.frame_rate,
            )
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
        let rotation_completed_qpc = self
            .flags
            .clock
            .now_qpc_100ns()
            .map_err(ReplayHandlerError::new)?;
        self.flags
            .shared
            .callback_telemetry
            .last_rotation_lateness_after_100ns
            .store(
                rotation_completed_qpc
                    .saturating_sub(output_qpc_100ns)
                    .max(0),
                Ordering::Relaxed,
            );
        Ok(Some(previous_sequence_number))
    }

    fn emit_output_frame(
        &mut self,
        global_frame_index: u64,
        phases: &mut CallbackPhaseDurations,
    ) -> Result<bool, ReplayHandlerError> {
        if self.awaiting_following_frame {
            self.rotation_lifecycle.following_frame_arrived_ms = Some(self.elapsed_ms());
            self.awaiting_following_frame = false;
            self.publish_lifecycle(phases);
        }

        let evaluation_started = Instant::now();
        self.poll_preparation(phases)?;
        let active_has_frames = self.active.as_ref().is_some_and(ActiveSegment::has_frames);
        let nominal_rotation_due = self
            .active
            .as_ref()
            .is_some_and(|active| active.should_rotate(self.flags.frame_rate))
            && active_has_frames;

        if nominal_rotation_due {
            self.mark_rotation_requested(phases);
            self.ensure_preparation(phases)?;
            self.poll_preparation(phases)?;
        } else if self
            .active
            .as_ref()
            .is_some_and(|active| active.should_prepare(self.flags.frame_rate))
        {
            self.ensure_preparation(phases)?;
        }
        phases.rotation_evaluation += evaluation_started.elapsed();

        let output_qpc_100ns = self.video_timeline_start_qpc_100ns.saturating_add(
            ((i128::from(global_frame_index) * 10_000_000)
                / i128::from(self.flags.frame_rate.max(1))) as i64,
        );
        let source_store = Arc::clone(&self.source_store);
        let mut source_store = source_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let selection = match self.pending_output_selection {
            Some((pending_index, selection)) if pending_index == global_frame_index => selection,
            _ => {
                let selection = source_store.select(output_qpc_100ns).ok_or_else(|| {
                    ReplayHandlerError::new(
                        "The realtime scheduler has no initial display-capture visual frame.",
                    )
                })?;
                self.pending_output_selection = Some((global_frame_index, selection));
                selection
            }
        };
        let fresh_source =
            source_generation_is_fresh(self.last_output_source_generation, selection.generation);
        let frame = source_store.current_frame().ok_or_else(|| {
            ReplayHandlerError::new("The realtime scheduler lost its current GPU source frame.")
        })?;

        let rotate_now = nominal_rotation_due && self.prepared.is_some();
        let queued = if rotate_now {
            let swap_started = Instant::now();
            let rotated =
                self.rotate_on_output(frame, output_qpc_100ns, &selection, fresh_source, phases)?;
            phases.swap += swap_started.elapsed();
            if let Some(sequence_number) = rotated {
                if let Some(request) = self.flags.shared.pending_save_boundary() {
                    let state_started = Instant::now();
                    self.flags.shared.acknowledge_save_boundary(
                        request,
                        sequence_number,
                        output_qpc_100ns,
                    );
                    phases.state_update += state_started.elapsed();
                }
                true
            } else {
                false
            }
        } else {
            let was_empty = !self.active.as_ref().is_some_and(ActiveSegment::has_frames);
            let encoded = self
                .active
                .as_mut()
                .ok_or_else(|| ReplayHandlerError::new("The active replay segment is missing."))?
                .encode_frame(
                    frame,
                    output_qpc_100ns,
                    selection.source_qpc_100ns,
                    selection.first_consumed_source_qpc_100ns,
                    fresh_source,
                    selection.consumed_updates,
                    self.session_started,
                    self.flags.frame_rate,
                )
                .map_err(ReplayHandlerError::new)?;
            self.flags
                .shared
                .callback_telemetry
                .record_send_frame(encoded.send_duration);
            self.flags
                .shared
                .callback_telemetry
                .record_encoder_frame(encoded.telemetry);
            if encoded.telemetry.queued && was_empty {
                self.rotation_lifecycle.active_segment_first_frame_ms = self
                    .active
                    .as_ref()
                    .and_then(|active| active.first_frame_submitted_ms);
                self.publish_lifecycle(phases);
            }
            encoded.telemetry.queued
        };

        if !queued {
            self.queue_full_recovery.record_refusal();
            return Ok(false);
        }
        if self.queue_full_recovery.record_acceptance() {
            self.flags
                .shared
                .callback_telemetry
                .recovered_queue_full_frames
                .fetch_add(1, Ordering::Relaxed);
        }
        self.pending_output_selection = None;

        self.last_output_source_generation = Some(selection.generation);
        let telemetry = &self.flags.shared.callback_telemetry;
        if fresh_source {
            telemetry
                .fresh_output_frames
                .fetch_add(1, Ordering::Relaxed);
        } else {
            telemetry.held_output_frames.fetch_add(1, Ordering::Relaxed);
        }
        telemetry
            .superseded_source_updates
            .fetch_add(selection.superseded_updates, Ordering::Relaxed);
        telemetry
            .scheduler_current_output_frame_index
            .store(global_frame_index, Ordering::Relaxed);
        Ok(true)
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

fn expected_cfr_frame_index(start_qpc_100ns: i64, now_qpc_100ns: i64, frame_rate: u32) -> u64 {
    let elapsed = now_qpc_100ns.saturating_sub(start_qpc_100ns).max(0);
    u64::try_from((i128::from(elapsed) * i128::from(frame_rate.max(1))) / 10_000_000i128)
        .unwrap_or(u64::MAX)
}

fn atomic_100ns_ms(value: &AtomicI64) -> Option<f64> {
    let value = value.load(Ordering::Relaxed);
    (value >= 0).then(|| value as f64 / 10_000.0)
}

fn cfr_frame_qpc(start_qpc_100ns: i64, frame_index: u64, frame_rate: u32) -> i64 {
    start_qpc_100ns.saturating_add(
        ((i128::from(frame_index) * 10_000_000i128) / i128::from(frame_rate.max(1))) as i64,
    )
}

fn cfr_duration_100ns(frame_count: u64, frame_rate: u32) -> i64 {
    ((i128::from(frame_count) * 10_000_000i128) / i128::from(frame_rate.max(1))) as i64
}

fn rebased_cfr_timeline_start(
    previous_start_qpc_100ns: i64,
    next_output_frame_index: u64,
    resume_qpc_100ns: i64,
    frame_rate: u32,
) -> i64 {
    let previous_next_qpc = cfr_frame_qpc(
        previous_start_qpc_100ns,
        next_output_frame_index,
        frame_rate,
    );
    let next_qpc = resume_qpc_100ns.max(previous_next_qpc);
    next_qpc.saturating_sub(cfr_duration_100ns(next_output_frame_index, frame_rate))
}

fn segment_output_frame_capacity(frame_rate: u32) -> u64 {
    u64::from(frame_rate).saturating_mul(SEGMENT_DURATION.as_secs())
}

fn normal_prewarm_due(frame_count: u64, frame_rate: u32) -> bool {
    let lead_frames = u64::from(frame_rate).saturating_mul(NORMAL_PREWARM_LEAD_SECONDS);
    frame_count >= segment_output_frame_capacity(frame_rate).saturating_sub(lead_frames)
}

fn normal_rotation_due(frame_count: u64, frame_rate: u32) -> bool {
    frame_count >= segment_output_frame_capacity(frame_rate)
}

fn due_source_update_count(timestamps: impl Iterator<Item = i64>, output_qpc_100ns: i64) -> usize {
    timestamps
        .take_while(|timestamp| *timestamp <= output_qpc_100ns)
        .count()
}

fn source_generation_is_fresh(previous: Option<u64>, current: u64) -> bool {
    previous != Some(current)
}

impl Drop for RealtimeCfrScheduler {
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

struct ReplayFramePipeline {
    flags: ReplayCaptureFlags,
    source_store: Arc<Mutex<SourceFrameStore>>,
    scheduler: Option<RealtimeCfrScheduler>,
    scheduler_thread: Option<JoinHandle<()>>,
    last_detached_source_qpc_100ns: Option<i64>,
    scheduler_stop_requested: Arc<AtomicBool>,
}

impl ReplayFramePipeline {
    fn from_flags(flags: ReplayCaptureFlags) -> Result<Self, ReplayHandlerError> {
        let session_started = flags.session_started;
        let first_sequence = flags.shared.next_segment_sequence_number();
        let scheduler_stop_requested = Arc::new(AtomicBool::new(false));
        let finalizer =
            FinalizerWorker::new(Arc::clone(&flags.shared)).map_err(ReplayHandlerError::new)?;
        let active = ActiveSegment::create(&flags, first_sequence, session_started)
            .map_err(ReplayHandlerError::new)?;
        let prewarmer = EncoderPrewarmer::new(flags.clone(), session_started)
            .map_err(ReplayHandlerError::new)?;
        let rotation_lifecycle = RotationLifecycleTrace {
            active_sequence_number: Some(active.sequence_number),
            ..RotationLifecycleTrace::default()
        };
        flags.shared.publish_rotation_lifecycle(&rotation_lifecycle);
        let source_store = Arc::new(Mutex::new(SourceFrameStore::default()));
        Ok(Self {
            flags: flags.clone(),
            source_store: Arc::clone(&source_store),
            scheduler: Some(RealtimeCfrScheduler {
                flags,
                active: Some(active),
                finalizer: Some(finalizer),
                prewarmer: Some(prewarmer),
                prepared: None,
                preparation_in_flight: false,
                next_sequence: first_sequence.saturating_add(1),
                session_started,
                rotation_requested_ms: None,
                rotation_lifecycle,
                awaiting_following_frame: false,
                finished: false,
                source_store,
                video_timeline_start_qpc_100ns: 0,
                next_output_frame_index: 0,
                last_output_source_generation: None,
                pending_output_selection: None,
                queue_full_recovery: QueueFullRecovery::default(),
                observed_discontinuity_generation: 0,
                rebased_discontinuity_generation: 0,
                pipeline_stop_requested: Arc::clone(&scheduler_stop_requested),
            }),
            scheduler_thread: None,
            last_detached_source_qpc_100ns: None,
            scheduler_stop_requested,
        })
    }

    fn start_scheduler(&mut self, video_start_qpc_100ns: i64) -> Result<(), ReplayHandlerError> {
        let mut scheduler = self.scheduler.take().ok_or_else(|| {
            ReplayHandlerError::new("The realtime CFR scheduler was already started.")
        })?;
        scheduler.video_timeline_start_qpc_100ns = video_start_qpc_100ns;
        let shared = Arc::clone(&self.flags.shared);
        let thread = thread::Builder::new()
            .name("slickclip-cfr-scheduler".to_string())
            .spawn(move || {
                let _worker = VideoWorkerGuard::new(&shared);
                if let Err(error) = scheduler.run() {
                    shared.mark_error(format!("Realtime CFR scheduler failed: {error}"));
                }
            })
            .map_err(|error| {
                ReplayHandlerError::new(format!(
                    "Could not start the realtime CFR scheduler: {error}"
                ))
            })?;
        self.scheduler_thread = Some(thread);
        Ok(())
    }

    fn stop_scheduler(&mut self) -> Result<(), ReplayHandlerError> {
        self.flags.shared.request_stop();
        self.stop_scheduler_for_transition()
    }

    fn stop_scheduler_for_transition(&mut self) -> Result<(), ReplayHandlerError> {
        self.scheduler_stop_requested.store(true, Ordering::Release);
        if let Some(thread) = self.scheduler_thread.take() {
            thread
                .join()
                .map_err(|_| ReplayHandlerError::new("The realtime CFR scheduler panicked."))?;
        }
        if let Some(mut scheduler) = self.scheduler.take() {
            scheduler.finish_session()?;
        }
        Ok(())
    }

    fn accept_detached_frame(
        &mut self,
        frame: DetachedFrame,
        source_qpc_100ns: i64,
    ) -> Result<(), ReplayHandlerError> {
        self.flags.shared.frame_observed();
        self.flags
            .shared
            .record_source_frame_timestamp(source_qpc_100ns, self.flags.frame_rate);
        let Some(detach_watermark) = accepted_source_frame_watermark(
            self.last_detached_source_qpc_100ns,
            source_qpc_100ns,
            self.flags.frame_rate,
        ) else {
            self.flags
                .shared
                .callback_telemetry
                .source_frames_rate_limited
                .fetch_add(1, Ordering::Relaxed);
            return Ok(());
        };
        self.last_detached_source_qpc_100ns = Some(detach_watermark);
        self.source_store
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .update_detached(frame, source_qpc_100ns);
        self.flags
            .shared
            .callback_telemetry
            .source_frames_detached
            .fetch_add(1, Ordering::Relaxed);
        if self.scheduler_thread.is_none() {
            self.start_scheduler(source_qpc_100ns)?;
        }
        Ok(())
    }
}

impl Drop for ReplayFramePipeline {
    fn drop(&mut self) {
        if let Err(error) = self.stop_scheduler() {
            self.flags.shared.mark_error(error.to_string());
        }
    }
}

fn run_dxgi_capture(
    mut prepared: PreparedDxgiCapture,
    flags: ReplayCaptureFlags,
    pipeline: Arc<Mutex<ReplayFramePipeline>>,
) -> Result<(), String> {
    let frame_interval =
        Duration::from_nanos(1_000_000_000u64 / u64::from(flags.frame_rate.max(1)));
    let mut next_frame_at = Instant::now();
    while !flags.shared.should_stop() {
        let now = Instant::now();
        if now < next_frame_at {
            thread::sleep((next_frame_at - now).min(Duration::from_millis(10)));
            continue;
        }
        next_frame_at = now + frame_interval;
        let detached = if let Some(detached) = prepared.startup_frame.take() {
            compose_system_cursor(&detached, prepared.desktop_origin)?;
            detached
        } else {
            match prepared.api_mut()?.acquire_next_frame(20) {
                Ok(frame) => {
                    if frame.width() != flags.width || frame.height() != flags.height {
                        return Err(format!(
                        "The Display Capture display geometry changed from {}x{} to {}x{} during capture.",
                        flags.width,
                        flags.height,
                        frame.width(),
                        frame.height()
                    ));
                    }
                    let detached = DetachedFrame::new_gdi_render_target(
                        frame.device(),
                        frame.device_context(),
                        frame.width(),
                        frame.height(),
                        ColorFormat::Bgra8,
                    )
                    .map_err(|error| {
                        format!("Could not allocate a Display Capture GPU frame: {error}")
                    })?;
                    unsafe {
                        frame
                            .device_context()
                            .CopyResource(detached.as_raw_texture(), frame.texture());
                    }
                    compose_system_cursor(&detached, prepared.desktop_origin)?;
                    detached
                }
                Err(DxgiError::Timeout) => continue,
                Err(DxgiError::AccessLost) => {
                    flags.scheduler_discontinuity.pause();
                    flags.shared.begin_dxgi_recovery();
                    prepared.suspend_invalid_duplication();
                    prepared.pending_recovery =
                        Some(recover_dxgi_duplication(&flags.shared, &mut prepared)?);
                    next_frame_at = Instant::now();
                    continue;
                }
                Err(error) => return Err(format!("Display Capture failed: {error}")),
            }
        };
        let source_qpc_100ns = flags.clock.now_qpc_100ns()?;
        pipeline
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .accept_detached_frame(detached, source_qpc_100ns)
            .map_err(|error| format!("Display Capture replay pipeline failed: {error}"))?;
        if flags.scheduler_discontinuity.snapshot().paused {
            let recovery = prepared
                .pending_recovery
                .take()
                .unwrap_or(DxgiRecoveryReport {
                    elapsed: Duration::ZERO,
                    recreation_attempts: 0,
                });
            flags.shared.complete_dxgi_recovery(
                flags.capture_fallback_reason.clone(),
                recovery.elapsed,
                recovery.recreation_attempts,
            );
            flags.scheduler_discontinuity.resume(source_qpc_100ns);
        }
    }
    pipeline
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .stop_scheduler()
        .map_err(|error| format!("Display Capture replay pipeline could not stop cleanly: {error}"))
}

fn compose_system_cursor(frame: &DetachedFrame, desktop_origin: (i32, i32)) -> Result<(), String> {
    let mut cursor = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    if unsafe { GetCursorInfo(&mut cursor) }.is_err()
        || cursor.flags.0 & CURSOR_SHOWING.0 == 0
        || cursor.hCursor.is_invalid()
    {
        return Ok(());
    }

    let mut icon = ICONINFO::default();
    unsafe { GetIconInfo(HICON(cursor.hCursor.0), &mut icon) }
        .map_err(|error| format!("Could not inspect the captured cursor shape: {error}"))?;
    let x = cursor
        .ptScreenPos
        .x
        .saturating_sub(desktop_origin.0)
        .saturating_sub(icon.xHotspot as i32);
    let y = cursor
        .ptScreenPos
        .y
        .saturating_sub(desktop_origin.1)
        .saturating_sub(icon.yHotspot as i32);

    let surface: IDXGISurface1 = frame.as_raw_texture().cast().map_err(|error| {
        format!("Could not open the Display Capture cursor-composition surface: {error}")
    })?;
    let hdc = unsafe { surface.GetDC(false) }.map_err(|error| {
        format!("Could not acquire the Display Capture cursor-composition surface: {error}")
    })?;
    let draw_result =
        unsafe { DrawIconEx(hdc, x, y, HICON(cursor.hCursor.0), 0, 0, 0, None, DI_NORMAL) };
    let release_result = unsafe { surface.ReleaseDC(None) };
    if !icon.hbmMask.is_invalid() {
        let _ = unsafe { DeleteObject(HGDIOBJ(icon.hbmMask.0)) };
    }
    if !icon.hbmColor.is_invalid() {
        let _ = unsafe { DeleteObject(HGDIOBJ(icon.hbmColor.0)) };
    }
    draw_result.map_err(|error| format!("Could not compose the captured cursor: {error}"))?;
    release_result.map_err(|error| {
        format!("Could not release the Display Capture cursor-composition surface: {error}")
    })
}

fn accepted_source_frame_watermark(
    last_detached_qpc_100ns: Option<i64>,
    source_qpc_100ns: i64,
    frame_rate: u32,
) -> Option<i64> {
    let Some(last) = last_detached_qpc_100ns else {
        return Some(source_qpc_100ns);
    };
    let minimum_interval_100ns = 10_000_000i64 / i64::from(frame_rate.max(1));
    let tolerance_100ns = minimum_interval_100ns / 100;
    let elapsed = source_qpc_100ns.saturating_sub(last);
    if elapsed < minimum_interval_100ns.saturating_sub(tolerance_100ns) {
        return None;
    }
    Some(if elapsed > minimum_interval_100ns.saturating_mul(4) {
        source_qpc_100ns
    } else {
        last.saturating_add(minimum_interval_100ns)
    })
}

fn cursor_flags() -> Option<u32> {
    let mut info = CURSORINFO {
        cbSize: std::mem::size_of::<CURSORINFO>() as u32,
        ..Default::default()
    };
    unsafe { GetCursorInfo(&mut info) }.ok()?;
    Some(info.flags.0)
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
    use std::time::{Duration, Instant};

    use super::{
        accepted_source_frame_watermark, calculate_rotation_diagnostics, cfr_duration_100ns,
        classify_dxgi_health_samples, due_source_update_count, expected_cfr_frame_index,
        normal_prewarm_due, normal_rotation_due, plan_capture, rebased_cfr_timeline_start,
        recovery_output_matches_selected_display, replay_start_disposition,
        segment_output_frame_capacity, source_generation_is_fresh, validate_start_request,
        ActiveCaptureMode, AudioReplayConfiguration, DxgiRecoveryCoordinator,
        DxgiRecoveryStabilityKey, DxgiSessionGuard, QueueFullRecovery, ReplayBufferStartRequest,
        ReplayQuality, ReplaySessionClock, ReplayStartDisposition, SchedulerDiscontinuity,
        SharedReplay, VideoFrameTimingPoint, DXGI_RECOVERY_DEADLINE,
        DXGI_RECOVERY_STABILITY_POLL_INTERVAL,
    };
    use crate::capture::compatibility::{
        CaptureHealth, CaptureMode, FrameHealthSample, PresentationClass, VideoBackend,
    };
    use crate::capture::encoder::{EncoderChoice, EncoderCodec};
    use crate::capture::targets::{CaptureTargetRequest, CaptureTargetType};
    use crate::replay::audio::{
        AudioSourceKind, AudioTrackConfiguration, AudioTrackRole, AudioTrackState,
    };
    use crate::replay::segment::CompletedSegment;
    use crate::replay::state::ReplayLifecycleState;

    fn request(duration: u32, frame_rate: u32) -> ReplayBufferStartRequest {
        ReplayBufferStartRequest {
            target: CaptureTargetRequest {
                target_type: CaptureTargetType::Monitor,
                id: "monitor:test".to_string(),
            },
            capture_mode: CaptureMode::Auto,
            encoder: EncoderChoice::Automatic,
            replay_duration_seconds: duration,
            frame_rate,
            quality: ReplayQuality::Balanced,
            audio: AudioReplayConfiguration::default(),
        }
    }

    fn completed_segment(path: &Path, sequence_number: u64) -> CompletedSegment {
        let session_start_qpc_100ns = i64::try_from(sequence_number.saturating_sub(1))
            .unwrap()
            .saturating_mul(20_000_000);
        let frame_timing_points = (0..120)
            .map(|frame_index| VideoFrameTimingPoint {
                frame_index,
                output_qpc_100ns: session_start_qpc_100ns
                    + ((i128::from(frame_index) * 10_000_000) / 60) as i64,
                source_qpc_100ns: session_start_qpc_100ns
                    + ((i128::from(frame_index) * 10_000_000) / 60) as i64,
                encoded_pts_100ns: ((i128::from(frame_index) * 10_000_000) / 60) as i64,
                fresh_source: true,
            })
            .collect::<Vec<_>>();
        CompletedSegment {
            sequence_number,
            file_path: path.to_string_lossy().into_owned(),
            start_timestamp_ms: sequence_number * 2_000,
            end_timestamp_ms: (sequence_number + 1) * 2_000,
            actual_duration_ms: 2_000,
            segment_session_start_qpc_100ns: session_start_qpc_100ns,
            segment_session_end_qpc_100ns: session_start_qpc_100ns + 20_000_000,
            first_frame_timestamp_100ns: session_start_qpc_100ns,
            last_frame_timestamp_100ns: session_start_qpc_100ns + 19_833_333,
            encoded_start_pts_100ns: 0,
            encoded_last_frame_pts_100ns: 19_833_333,
            encoded_end_pts_100ns: 20_000_000,
            encoded_duration_100ns: 20_000_000,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: 10_000_000,
            frame_timing_points,
            next_segment_first_frame_timestamp_100ns: None,
            source_frame_gap_ms: None,
            source_update_count: 120,
            fresh_output_frame_count: 120,
            held_output_frame_count: 0,
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
            average_bitrate_mbps: 0.000016,
            finalized: true,
            finalization_time_ms: 10.0,
            rotation_gap_ms: Some(2.0),
        }
    }

    fn recovery_key(output_identity: &str) -> DxgiRecoveryStabilityKey {
        DxgiRecoveryStabilityKey {
            output_identity: output_identity.to_string(),
            desktop_origin: (0, 0),
            width: 1920,
            height: 1080,
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
    fn fullscreen_dxgi_access_loss_enters_recovering_without_a_fatal_error() {
        let shared = SharedReplay::new();
        shared.begin(&request(120, 60));
        shared.set_capture_health(VideoBackend::Dxgi, "Healthy", None);
        shared.mark_running();

        shared.begin_dxgi_recovery();

        let status = shared.snapshot();
        assert_eq!(status.state, ReplayLifecycleState::Starting);
        assert_eq!(status.capture_health, "Recovering");
        assert!(status.error_message.is_none());
        assert!(status.capture_fallback_reason.is_some());
    }

    #[test]
    fn changing_display_identity_does_not_consume_recreation_attempts() {
        let started = Instant::now();
        let mut recovery = DxgiRecoveryCoordinator::new(started);

        for poll in 0..20u32 {
            let now = started + DXGI_RECOVERY_STABILITY_POLL_INTERVAL * poll;
            assert!(!recovery.observe(
                now,
                recovery_key(if poll % 2 == 0 {
                    r"\\.\DISPLAY1"
                } else {
                    r"\\.\DISPLAY2"
                }),
            ));
            assert_eq!(recovery.recreation_attempts, 0);
            assert!(!recovery.expired(now));
        }
    }

    #[test]
    fn same_display_reaches_each_bounded_recreation_gate() {
        let started = Instant::now();
        let mut recovery = DxgiRecoveryCoordinator::new(started);
        let selected_display = recovery_key(r"\\.\DISPLAY1");

        assert!(!recovery.observe(started, selected_display.clone()));
        assert!(!recovery.observe(
            started + DXGI_RECOVERY_STABILITY_POLL_INTERVAL,
            selected_display.clone(),
        ));
        assert!(recovery.observe(
            started + DXGI_RECOVERY_STABILITY_POLL_INTERVAL * 2,
            selected_display.clone(),
        ));
        assert_eq!(recovery.begin_recreation(), 1);

        recovery.record_access_lost_during_validation(
            started + DXGI_RECOVERY_STABILITY_POLL_INTERVAL * 3,
        );
        assert!(!recovery.observe(
            started + DXGI_RECOVERY_STABILITY_POLL_INTERVAL * 4,
            selected_display.clone(),
        ));
        assert!(!recovery.observe(
            started + DXGI_RECOVERY_STABILITY_POLL_INTERVAL * 5,
            selected_display.clone(),
        ));
        assert!(recovery.observe(
            started + DXGI_RECOVERY_STABILITY_POLL_INTERVAL * 6,
            selected_display,
        ));
        assert_eq!(recovery.begin_recreation(), 2);
    }

    #[test]
    fn recovery_deadline_is_bounded_without_a_rapid_loss_counter() {
        let started = Instant::now();
        let recovery = DxgiRecoveryCoordinator::new(started);
        assert!(!recovery.expired(started + DXGI_RECOVERY_DEADLINE - Duration::from_millis(1)));
        assert!(recovery.expired(started + DXGI_RECOVERY_DEADLINE));
        assert_eq!(recovery.recreation_attempts, 0);
    }

    #[test]
    fn stop_requested_during_recovery_is_immediately_authoritative() {
        let shared = SharedReplay::new();
        shared.begin(&request(120, 60));
        shared.mark_running();
        shared.begin_dxgi_recovery();
        shared.request_stop();

        assert!(shared.should_stop());
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Stopping);
    }

    #[test]
    fn recovery_state_preserves_one_logical_and_one_dxgi_session() {
        let shared = Arc::new(SharedReplay::new());
        shared.begin(&request(120, 60));
        shared.mark_running();
        let _dxgi = DxgiSessionGuard::new(&shared);

        shared.begin_dxgi_recovery();
        let recovering = shared.snapshot().native_resources;
        assert_eq!(recovering.logical_replay_sessions, 1);
        assert_eq!(recovering.dxgi_duplication_sessions, 1);

        shared.complete_dxgi_recovery(None, Duration::from_secs(1), 1);
        shared.mark_running();
        let recovered = shared.snapshot().native_resources;
        assert_eq!(recovered.logical_replay_sessions, 1);
        assert_eq!(recovered.dxgi_duplication_sessions, 1);
    }

    #[test]
    fn scheduler_discontinuity_pauses_and_rebases_without_historical_catch_up() {
        let signal = SchedulerDiscontinuity::default();
        let generation = signal.pause();
        assert_eq!(generation, 1);
        assert!(signal.snapshot().paused);

        let old_start = 100_000_000;
        let next_index = 120;
        let last_expected_next = super::cfr_frame_qpc(old_start, next_index, 60);
        let resumed_at = last_expected_next + 20_000_000;
        signal.resume(resumed_at);
        let snapshot = signal.snapshot();
        assert!(!snapshot.paused);
        assert_eq!(snapshot.resume_qpc_100ns, Some(resumed_at));

        let rebased = rebased_cfr_timeline_start(old_start, next_index, resumed_at, 60);
        assert_eq!(
            expected_cfr_frame_index(rebased, resumed_at, 60),
            next_index
        );
        assert_eq!(super::cfr_frame_qpc(rebased, next_index, 60), resumed_at);
        assert!(super::cfr_frame_qpc(rebased, next_index, 60) >= last_expected_next);
    }

    #[test]
    fn screen_health_accepts_static_and_dark_valid_frames_but_rejects_no_valid_frame() {
        let black = FrameHealthSample {
            dark_pixels: 144,
            sampled_pixels: 144,
            signature: 1,
        };
        let static_frame = FrameHealthSample {
            dark_pixels: 20,
            sampled_pixels: 144,
            signature: 2,
        };
        assert_eq!(
            classify_dxgi_health_samples(&[static_frame; 5]),
            CaptureHealth::Healthy
        );
        assert_eq!(
            classify_dxgi_health_samples(&[black]),
            CaptureHealth::Healthy,
        );
        assert_eq!(classify_dxgi_health_samples(&[]), CaptureHealth::Invalid);
    }

    #[test]
    fn unstable_startup_observations_do_not_create_duplication_attempts() {
        let mut stability = super::DxgiRecoveryStabilityTracker::default();
        let key = |width| DxgiRecoveryStabilityKey {
            output_identity: r"\\.\DISPLAY1".to_string(),
            desktop_origin: (0, 0),
            width,
            height: 1080,
        };
        assert!(!stability.observe(key(1920)));
        assert!(!stability.observe(key(1280)));
        assert!(!stability.observe(key(1920)));
        assert!(!stability.observe(key(1920)));
        assert!(stability.observe(key(1920)));
    }

    #[test]
    fn successful_recovery_clears_current_error_and_retains_history_only() {
        let shared = SharedReplay::new();
        shared.begin(&request(120, 60));
        shared.mark_running();
        {
            shared.lock().error_message =
                Some("Capture compatibility issue: transient access loss".to_string());
        }

        shared.begin_dxgi_recovery();
        shared.complete_dxgi_recovery(None, Duration::from_millis(2_750), 2);
        shared.mark_running();

        let status = shared.snapshot();
        assert_eq!(status.state, ReplayLifecycleState::Running);
        assert_eq!(status.capture_health, "Healthy");
        assert!(status.error_message.is_none());
        assert!(status.capture_fallback_reason.is_none());
        assert!(status
            .last_recovery_message
            .as_deref()
            .is_some_and(|message| message.contains("Recovered Display Capture")));
    }

    #[test]
    fn fatal_recovery_failure_is_latched_once_and_cannot_serialize_healthy() {
        let shared = SharedReplay::new();
        shared.begin(&request(120, 60));
        shared.mark_error("DXGI recovery deadline exhausted.");
        shared.mark_error("A later duplicate failure must not replace the first.");
        shared.set_capture_health(VideoBackend::Dxgi, "Healthy", None);

        let status = shared.snapshot();
        assert_eq!(status.state, ReplayLifecycleState::Error);
        assert_eq!(status.capture_health, "Failed");
        assert_eq!(
            status.error_message.as_deref(),
            Some("DXGI recovery deadline exhausted.")
        );
    }

    #[test]
    fn finalized_retained_segments_survive_transient_recovery() {
        let shared = SharedReplay::new();
        shared.begin(&request(120, 60));
        {
            let mut inner = shared.lock();
            inner
                .ring
                .push(completed_segment(Path::new("segment-1.mp4"), 1));
            inner
                .ring
                .push(completed_segment(Path::new("segment-2.mp4"), 2));
        }
        shared.mark_running();
        let before = shared.snapshot();

        shared.begin_dxgi_recovery();
        let recovering = shared.snapshot();
        assert_eq!(
            recovering.completed_segment_count,
            before.completed_segment_count
        );
        assert_eq!(recovering.retained_bytes, before.retained_bytes);
        assert!(shared.request_save_boundary_at(50_000_000).is_err());

        shared.complete_dxgi_recovery(None, Duration::from_secs(2), 1);
        shared.mark_running();
        let recovered = shared.snapshot();
        assert_eq!(
            recovered.completed_segment_count,
            before.completed_segment_count
        );
        assert_eq!(recovered.retained_bytes, before.retained_bytes);
    }

    #[test]
    fn every_legacy_capture_mode_resolves_to_one_display_pipeline() {
        for requested in [
            CaptureMode::Auto,
            CaptureMode::GameCapture,
            CaptureMode::ScreenCapture,
        ] {
            for target_type in [CaptureTargetType::Window, CaptureTargetType::Monitor] {
                let plan = plan_capture(requested, target_type);
                assert_eq!(plan.active_mode, ActiveCaptureMode::DisplayCapture);
                assert_eq!(plan.active_mode.backend(), VideoBackend::FfmpegDdagrab);
                assert_eq!(
                    plan.selection_reason,
                    "Replay captures one persistent physical display"
                );
            }
        }
    }

    #[test]
    fn display_target_and_logical_owners_stay_stable_across_alt_tab_observations() {
        let shared = Arc::new(SharedReplay::new());
        shared.begin_with_plan(
            &request(120, 60),
            Some(PresentationClass::FullscreenLike),
            plan_capture(CaptureMode::Auto, CaptureTargetType::Window),
        );
        {
            let mut inner = shared.lock();
            inner.session_id = Some("persistent-display-session".to_string());
            inner.target_id = Some("window:game".to_string());
            inner
                .ring
                .push(completed_segment(Path::new("segment-1.mp4"), 1));
        }
        shared.mark_running();
        shared
            .callback_telemetry
            .ffmpeg_capture_processes
            .store(1, std::sync::atomic::Ordering::Relaxed);

        for _foreground_transition in [false, true, false, true] {
            let status = shared.snapshot();
            assert_eq!(
                status.session_id.as_deref(),
                Some("persistent-display-session")
            );
            assert_eq!(status.target_id.as_deref(), Some("window:game"));
            assert_eq!(
                status.active_capture_mode.as_deref(),
                Some("Display Capture")
            );
            assert_eq!(status.video_backend, "FFmpeg ddagrab");
            assert_eq!(status.completed_segment_count, 1);
            assert_eq!(status.native_resources.logical_replay_sessions, 1);
            assert_eq!(status.native_resources.dxgi_duplication_sessions, 0);
            assert_eq!(status.native_resources.ffmpeg_capture_processes, 1);
            assert_eq!(status.native_resources.wgc_capture_sessions, 0);
        }
        assert!(recovery_output_matches_selected_display(
            r"\\.\DISPLAY2",
            r"\\.\DISPLAY2"
        ));
        assert!(!recovery_output_matches_selected_display(
            r"\\.\DISPLAY2",
            r"\\.\DISPLAY1"
        ));
    }

    #[test]
    fn display_capture_health_must_succeed_before_replay_becomes_ready() {
        let shared = SharedReplay::new();
        shared.begin(&request(30, 60));
        let starting = shared.snapshot();
        assert_eq!(starting.state, ReplayLifecycleState::Starting);
        assert_eq!(starting.video_backend, "FFmpeg ddagrab");
        assert_eq!(starting.capture_health, "Probing");
        assert_eq!(
            starting.active_capture_mode.as_deref(),
            Some("Display Capture")
        );

        shared.set_capture_health(VideoBackend::FfmpegDdagrab, "Healthy", None);
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Starting);
        shared.mark_running();
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Running);
    }

    #[test]
    fn display_session_guard_releases_the_only_native_capture_owner() {
        let shared = Arc::new(SharedReplay::new());
        {
            let _dxgi = DxgiSessionGuard::new(&shared);
            let resources = shared.snapshot().native_resources;
            assert_eq!(resources.dxgi_duplication_sessions, 1);
            assert_eq!(resources.wgc_capture_sessions, 0);
        }
        let resources = shared.snapshot().native_resources;
        assert_eq!(resources.dxgi_duplication_sessions, 0);
        assert_eq!(resources.wgc_capture_sessions, 0);
    }

    #[test]
    fn repeated_start_delivery_never_creates_duplicate_replay_or_audio_workers() {
        assert_eq!(
            replay_start_disposition(ReplayLifecycleState::Starting, true),
            ReplayStartDisposition::AlreadyActive
        );
        assert_eq!(
            replay_start_disposition(ReplayLifecycleState::Running, true),
            ReplayStartDisposition::AlreadyActive
        );
        assert_eq!(
            replay_start_disposition(ReplayLifecycleState::Stopped, true),
            ReplayStartDisposition::WorkerStillStopping
        );

        let shared = SharedReplay::new();
        shared.begin(&request(120, 60));
        assert_eq!(
            shared.snapshot().native_resources.logical_replay_sessions,
            1
        );
        assert_eq!(shared.snapshot().native_resources.audio_workers, 0);
    }

    #[test]
    fn six_hour_session_indices_remain_exact_at_thirty_and_sixty_fps() {
        let six_hours_100ns = 6_i64 * 60 * 60 * 10_000_000;
        for frame_rate in [30, 60] {
            let expected = 6_u64 * 60 * 60 * u64::from(frame_rate);
            assert_eq!(
                expected_cfr_frame_index(0, six_hours_100ns, frame_rate),
                expected
            );
            assert_eq!(cfr_duration_100ns(expected, frame_rate), six_hours_100ns);
        }
    }

    #[test]
    fn high_refresh_delivery_is_throttled_to_configured_thirty_or_sixty_fps() {
        for configured_rate in [30, 60] {
            let mut watermark = None;
            let mut accepted = 0;
            for source_frame in 0..360 {
                let timestamp = i64::from(source_frame) * 27_778; // 360 Hz for one second.
                if let Some(next) =
                    accepted_source_frame_watermark(watermark, timestamp, configured_rate)
                {
                    watermark = Some(next);
                    accepted += 1;
                }
            }
            assert!(
                ((configured_rate - 1)..=(configured_rate + 1)).contains(&accepted),
                "{configured_rate} FPS accepted {accepted} frames from 360 Hz input"
            );
        }
    }

    #[test]
    fn rejects_unsupported_frame_rate() {
        assert!(validate_start_request(&request(30, 144)).is_err());
    }

    #[test]
    fn qpc_elapsed_time_drives_sixty_and_thirty_fps_indices() {
        let start = 1_000_000_000;
        assert_eq!(expected_cfr_frame_index(start, start + 10_000_000, 60), 60);
        assert_eq!(expected_cfr_frame_index(start, start + 10_000_000, 30), 30);
        assert_eq!(expected_cfr_frame_index(start, start + 5_000_000, 60), 30);
    }

    #[test]
    fn newest_applicable_source_update_wins_between_cfr_ticks() {
        let timestamps = [1_000_000, 1_050_000, 1_100_000, 1_300_000];
        assert_eq!(
            due_source_update_count(timestamps.into_iter(), 1_166_667),
            3
        );
    }

    #[test]
    fn unchanged_source_generation_is_an_intentional_hold() {
        assert!(source_generation_is_fresh(None, 7));
        assert!(source_generation_is_fresh(Some(6), 7));
        assert!(!source_generation_is_fresh(Some(7), 7));
    }

    #[test]
    fn realtime_segment_rotation_and_partial_duration_use_output_frames() {
        assert_eq!(segment_output_frame_capacity(60), 120);
        assert_eq!(segment_output_frame_capacity(30), 60);
        assert_eq!(cfr_duration_100ns(37, 60), 6_166_666);
    }

    #[test]
    fn static_stop_or_save_deadline_still_advances_output_timeline() {
        let start = 500_000_000;
        assert_eq!(expected_cfr_frame_index(start, start + 30_000_000, 60), 180);
        assert_eq!(
            expected_cfr_frame_index(start, start + 300_000_000, 60),
            1_800
        );
        assert_eq!(cfr_duration_100ns(1_800, 60), 300_000_000);
    }

    #[test]
    fn save_mid_segment_waits_for_an_ordinary_rotation_boundary() {
        assert!(normal_prewarm_due(60, 60));
        assert!(!normal_rotation_due(61, 60));
        assert!(normal_rotation_due(120, 60));
    }

    #[test]
    fn normal_prewarm_has_one_second_of_lead_for_observed_creation_time() {
        let prewarm_frame = 60;
        let boundary_frame = segment_output_frame_capacity(60);
        let lead_ms = (boundary_frame - prewarm_frame) as f64 / 60.0 * 1_000.0;
        assert_eq!(lead_ms, 1_000.0);
        assert!(lead_ms > 370.74);
    }

    #[test]
    fn queue_refusal_retries_the_same_cfr_position_before_advancing() {
        let frame_index = 731;
        let expected_qpc = super::cfr_frame_qpc(500_000_000, frame_index, 60);
        let mut recovery = QueueFullRecovery::default();
        recovery.record_refusal();
        assert_eq!(
            super::cfr_frame_qpc(500_000_000, frame_index, 60),
            expected_qpc
        );
        assert!(recovery.record_acceptance());
        assert!(!recovery.record_acceptance());
    }

    #[test]
    fn save_anchor_does_not_stop_capture_and_only_acknowledges_a_covering_boundary() {
        let shared = SharedReplay::new();
        shared.begin(&request(30, 60));
        {
            let mut inner = shared.lock();
            inner
                .ring
                .push(completed_segment(Path::new("segment-1.mp4"), 1));
        }
        shared.mark_running();

        let (first_id, _, anchor) = shared.request_save_boundary_at(25_000_000).unwrap();
        let first = shared.pending_save_boundary().unwrap();
        assert_eq!(first.request_id, first_id);
        assert!(!shared.should_stop());
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Running);
        assert!(!normal_rotation_due(75, 60));

        shared.acknowledge_save_boundary(first, 1, anchor - 1);
        assert_eq!(shared.pending_save_boundary(), Some(first));
        shared.acknowledge_save_boundary(first, 1, anchor);
        assert!(shared.pending_save_boundary().is_none());

        let (second_id, _, _) = shared.request_save_boundary_at(26_000_000).unwrap();
        assert!(second_id > first_id);
        assert!(!shared.should_stop());
    }

    #[test]
    fn audio_track_state_remains_active_across_save_request() {
        let root =
            std::env::temp_dir().join(format!("replay-save-audio-active-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let shared = SharedReplay::new();
        shared.begin(&request(30, 60));
        shared
            .audio
            .begin(
                &AudioReplayConfiguration {
                    tracks: vec![AudioTrackConfiguration {
                        role: AudioTrackRole::Game,
                        enabled: true,
                        source_kind: AudioSourceKind::Process,
                        process_id: Some(7),
                        endpoint_id: None,
                        source_label: Some("Game".into()),
                    }],
                },
                ReplaySessionClock::create().unwrap(),
                root.clone(),
                30,
            )
            .unwrap();
        {
            let mut inner = shared.lock();
            inner
                .ring
                .push(completed_segment(Path::new("segment-1.mp4"), 1));
        }
        shared.mark_running();
        let before = shared.audio.snapshot().tracks[0].state;

        shared.request_save_boundary_at(10_000_000).unwrap();

        assert_eq!(before, AudioTrackState::Preparing);
        assert_eq!(shared.audio.snapshot().tracks[0].state, before);
        assert!(!shared.should_stop());
        let _ = fs::remove_dir_all(root);
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
    fn source_frame_detach_rate_is_bounded_to_configured_capture_fps() {
        let mut watermark = None;
        let mut accepted = 0;
        for source_frame in 0..240 {
            let timestamp = i64::from(source_frame) * 41_667;
            if let Some(next) = accepted_source_frame_watermark(watermark, timestamp, 60) {
                watermark = Some(next);
                accepted += 1;
            }
        }
        assert!(
            (59..=61).contains(&accepted),
            "accepted {accepted} source frames"
        );

        assert_eq!(
            accepted_source_frame_watermark(None, 1_000_000, 60),
            Some(1_000_000)
        );
        assert_eq!(
            accepted_source_frame_watermark(Some(1_000_000), 1_083_334, 60),
            None
        );
        assert!(accepted_source_frame_watermark(Some(1_000_000), 1_166_665, 60).is_some());
        assert!(accepted_source_frame_watermark(Some(1_000_000), 1_333_332, 30).is_some());
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
            std::env::temp_dir().join(format!("slickclip-pin-test-{}", std::process::id()));
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
            ReplaySessionClock::create().unwrap(),
        );
        shared.mark_running();

        for sequence in 1..=4 {
            let path = directory.join(format!("segment-{sequence:06}.mp4"));
            fs::write(&path, b"test").unwrap();
            if sequence <= 3 {
                shared.complete_segment(completed_segment(&path, sequence));
            }
        }

        let snapshot = shared
            .pin_snapshot(3, 123, 60_000_000, 10.0, Instant::now())
            .unwrap();
        let first_path = directory.join("segment-000001.mp4");
        shared.complete_segment(completed_segment(&directory.join("segment-000004.mp4"), 4));
        assert!(first_path.exists());

        drop(snapshot);
        assert!(!first_path.exists());
        fs::remove_dir_all(directory).unwrap();
    }
}
