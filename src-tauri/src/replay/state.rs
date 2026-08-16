use serde::Serialize;

use super::segment::CompletedSegment;

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
    pub recent_segments: Vec<CompletedSegment>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayCommandResult {
    pub success: bool,
    pub status: ReplayBufferStatus,
    pub error_message: Option<String>,
}

impl ReplayCommandResult {
    pub fn success(status: ReplayBufferStatus) -> Self {
        Self {
            success: true,
            status,
            error_message: None,
        }
    }

    pub fn failure(status: ReplayBufferStatus, error: impl Into<String>) -> Self {
        Self {
            success: false,
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
