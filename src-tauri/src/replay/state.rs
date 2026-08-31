use serde::Serialize;

use super::audio::AudioReplayStatus;
use super::segment::CompletedSegment;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RotationLifecycleTrace {
    pub active_sequence_number: Option<u64>,
    pub next_sequence_number: Option<u64>,
    pub active_segment_first_frame_ms: Option<f64>,
    pub prewarm_requested_ms: Option<f64>,
    pub encoder_creation_started_ms: Option<f64>,
    pub encoder_creation_completed_ms: Option<f64>,
    pub prepared_ready_ms: Option<f64>,
    pub rotation_requested_ms: Option<f64>,
    pub swap_started_ms: Option<f64>,
    pub old_segment_queued_ms: Option<f64>,
    pub swap_completed_ms: Option<f64>,
    pub following_frame_arrived_ms: Option<f64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplayLifecycleState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Error,
}

impl ReplayLifecycleState {
    pub const fn can_start(self) -> bool {
        matches!(self, Self::Stopped | Self::Error)
    }

    pub const fn is_active(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping)
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayBufferStatus {
    pub state: ReplayLifecycleState,
    pub error_message: Option<String>,
    pub target_id: Option<String>,
    pub target_label: Option<String>,
    pub requested_encoder: Option<String>,
    pub actual_encoder: Option<String>,
    pub requested_capture_mode: String,
    pub active_capture_mode: Option<String>,
    pub presentation: Option<String>,
    pub video_backend: String,
    pub backend_selection_reason: Option<String>,
    pub capture_health: String,
    pub capture_fallback_reason: Option<String>,
    pub last_recovery_message: Option<String>,
    pub recovery_elapsed_ms: Option<f64>,
    pub recovery_recreation_attempts: u64,
    pub replay_duration_seconds: u32,
    pub expected_segment_duration_seconds: f64,
    pub frame_rate: u32,
    pub width: u32,
    pub height: u32,
    pub session_id: Option<String>,
    pub session_directory: Option<String>,
    pub completed_segment_count: usize,
    pub retained_duration_seconds: f64,
    pub retained_bytes: u64,
    pub pending_finalizations: usize,
    pub dropped_segments: u64,
    pub last_segment_duration_seconds: Option<f64>,
    pub last_rotation_gap_ms: Option<f64>,
    pub last_finalize_time_ms: Option<f64>,
    pub normal_frame_interval_ms: Option<f64>,
    pub last_source_frame_gap_ms: Option<f64>,
    pub worst_source_frame_gap_ms: Option<f64>,
    pub average_source_frame_gap_ms: Option<f64>,
    pub last_encoder_creation_ms: Option<f64>,
    pub worst_encoder_creation_ms: Option<f64>,
    pub average_encoder_creation_ms: Option<f64>,
    pub rotation_count: u64,
    pub frames_observed: u64,
    pub last_estimated_frames_missed: Option<u64>,
    pub estimated_frames_missed_total: u64,
    pub material_source_gap_count: u64,
    pub encoder_preparation_in_flight: bool,
    pub prepared_encoder_ready: bool,
    pub next_encoder_state: String,
    pub average_callback_duration_ms: Option<f64>,
    pub worst_callback_duration_ms: Option<f64>,
    pub average_send_frame_duration_ms: Option<f64>,
    pub worst_send_frame_duration_ms: Option<f64>,
    #[serde(rename = "sendFrameOver16_67Ms")]
    pub send_frame_over_16_67_ms: u64,
    #[serde(rename = "sendFrameOver33_33Ms")]
    pub send_frame_over_33_33_ms: u64,
    pub send_frame_over_50_ms: u64,
    pub send_frame_over_100_ms: u64,
    pub average_callback_lock_wait_ms: Option<f64>,
    pub worst_callback_lock_wait_ms: Option<f64>,
    pub average_rotation_evaluation_ms: Option<f64>,
    pub worst_rotation_evaluation_ms: Option<f64>,
    pub average_swap_duration_ms: Option<f64>,
    pub worst_swap_duration_ms: Option<f64>,
    pub average_callback_state_update_ms: Option<f64>,
    pub worst_callback_state_update_ms: Option<f64>,
    pub average_callback_filesystem_ms: Option<f64>,
    pub worst_callback_filesystem_ms: Option<f64>,
    pub callback_filesystem_operation_count: u64,
    pub owned_frame_copies: u64,
    pub average_gpu_copy_duration_ms: Option<f64>,
    pub worst_gpu_copy_duration_ms: Option<f64>,
    pub encoder_queue_depth: u64,
    pub maximum_encoder_queue_depth: u64,
    pub encoder_queue_capacity: u64,
    pub encoder_queue_full_events: u64,
    pub deliberately_dropped_frames: u64,
    pub video_timeline_start_qpc_100ns: Option<i64>,
    pub scheduler_current_output_frame_index: Option<u64>,
    pub scheduler_expected_output_frame_index: Option<u64>,
    pub scheduler_current_lateness_ms: Option<f64>,
    pub scheduler_worst_lateness_ms: Option<f64>,
    pub scheduler_catch_up_wakeups: u64,
    pub scheduler_max_catch_up_burst: u64,
    pub scheduler_catch_up_frames: u64,
    pub scheduler_rotation_catch_up_frames: u64,
    pub scheduler_save_pending_catch_up_frames: u64,
    pub scheduler_discontinuities: u64,
    pub scheduler_rebases: u64,
    pub dxgi_duplication_recreations: u64,
    pub ffmpeg_capture_restarts: u64,
    pub dxgi_startup: DxgiStartupStatus,
    pub queue_full_retry_attempts: u64,
    pub recovered_queue_full_frames: u64,
    pub last_rotation_lateness_before_ms: Option<f64>,
    pub last_rotation_lateness_after_ms: Option<f64>,
    pub fresh_output_frames: u64,
    pub held_output_frames: u64,
    pub superseded_source_updates: u64,
    pub missed_realtime_output_frames: u64,
    pub source_frames_detached: u64,
    pub source_frames_rate_limited: u64,
    pub source_frame_update_rate: Option<f64>,
    pub output_cfr_rate: Option<f64>,
    pub frame_pool_creation_method: String,
    pub frame_pool_buffer_count: u32,
    pub native_resources: ReplayNativeResourceStatus,
    pub rotation_lifecycle: RotationLifecycleTrace,
    pub recent_segments: Vec<CompletedSegment>,
    pub audio: AudioReplayStatus,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DxgiStartupStatus {
    pub output_identity: Option<String>,
    pub adapter_identity: Option<String>,
    pub desktop_x: Option<i32>,
    pub desktop_y: Option<i32>,
    pub output_width: Option<u32>,
    pub output_height: Option<u32>,
    pub acquire_results: u64,
    pub acquired_frames: u64,
    pub timeouts: u64,
    pub access_losses: u64,
    pub invalid_textures: u64,
    pub cpu_readbacks: u64,
    pub last_cpu_readback_ms: Option<f64>,
    pub valid_samples: u64,
    pub last_acquire_result: Option<String>,
    pub last_valid_frame_elapsed_ms: Option<f64>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayNativeResourceStatus {
    pub logical_replay_sessions: u64,
    pub wgc_capture_sessions: u64,
    pub dxgi_duplication_sessions: u64,
    pub d3d11_devices: u64,
    pub d3d11_device_contexts: u64,
    pub frame_pools: u64,
    pub media_foundation_encoders: u64,
    pub ffmpeg_capture_processes: u64,
    pub video_workers: u64,
    pub audio_workers: u64,
    pub system_cursor_visible: Option<bool>,
    pub system_cursor_suppressed: Option<bool>,
    pub capture_cursor_mode: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayCommandResult {
    pub success: bool,
    pub started_new_session: bool,
    pub status: ReplayBufferStatus,
    pub error_message: Option<String>,
}

impl ReplayCommandResult {
    pub fn success(status: ReplayBufferStatus) -> Self {
        Self {
            success: true,
            started_new_session: false,
            status,
            error_message: None,
        }
    }

    pub fn started(status: ReplayBufferStatus) -> Self {
        Self {
            success: true,
            started_new_session: true,
            status,
            error_message: None,
        }
    }

    pub fn failure(status: ReplayBufferStatus, error: impl Into<String>) -> Self {
        Self {
            success: false,
            started_new_session: false,
            status,
            error_message: Some(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ReplayLifecycleState;

    #[test]
    fn lifecycle_rejects_start_while_active() {
        assert!(ReplayLifecycleState::Stopped.can_start());
        assert!(ReplayLifecycleState::Error.can_start());
        assert!(!ReplayLifecycleState::Starting.can_start());
        assert!(!ReplayLifecycleState::Running.can_start());
        assert!(!ReplayLifecycleState::Stopping.can_start());
    }
}
