use serde::Serialize;

pub const PROCESS_LOOPBACK_MINIMUM_BUILD: u32 = 20_348;
pub const AUDIO_TEST_DURATION_SECONDS: u64 = 10;

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioError {
    pub code: AudioErrorCode,
    pub message: String,
}

impl AudioError {
    pub fn new(code: AudioErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AudioErrorCode {
    MicrophoneUnavailable,
    EndpointUnavailable,
    MicrophoneAccessDenied,
    ProcessUnavailable,
    ProcessExited,
    ProcessLoopbackUnsupported,
    ProcessLoopbackActivationFailed,
    AudioServiceUnavailable,
    DeviceInvalidated,
    CaptureInitializationFailed,
    CaptureFailed,
    WavOutputFailed,
    CaptureAlreadyRunning,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneEndpoint {
    pub id: String,
    pub friendly_name: String,
    pub is_default_multimedia: bool,
    pub is_default_communications: bool,
    pub state: String,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationAudioProcess {
    pub process_id: u32,
    pub display_name: String,
    pub process_name: String,
    pub executable_path: Option<String>,
    pub session_display_names: Vec<String>,
    pub session_count: u32,
    pub render_endpoint_count: u32,
    pub session_state: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessLoopbackCapability {
    pub available: bool,
    pub windows_build: Option<u32>,
    pub minimum_windows_build: u32,
    pub status: String,
    pub error: Option<AudioError>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MicrophoneListResult {
    pub success: bool,
    pub devices: Vec<MicrophoneEndpoint>,
    pub error: Option<AudioError>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ApplicationAudioListResult {
    pub success: bool,
    pub applications: Vec<ApplicationAudioProcess>,
    pub capability: ProcessLoopbackCapability,
    pub error: Option<AudioError>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProcessActivationProbeResult {
    pub success: bool,
    pub process_id: u32,
    pub message: String,
    pub error: Option<AudioError>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioFormatMetadata {
    pub sample_format: String,
    pub format_tag: u16,
    pub sample_rate: u32,
    pub channel_count: u16,
    pub bits_per_sample: u16,
    pub valid_bits_per_sample: Option<u16>,
    pub block_align: u16,
    pub average_bytes_per_second: u32,
    pub channel_mask: Option<u32>,
    pub sub_format: Option<String>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AudioFormatDiagnostics {
    pub get_mix_format_status: String,
    pub format_role: String,
}

impl AudioFormatMetadata {
    pub fn duration_ms_for_frames(&self, frames: u64) -> f64 {
        if self.sample_rate == 0 {
            return 0.0;
        }
        frames as f64 * 1_000.0 / self.sample_rate as f64
    }
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTimingTelemetry {
    pub monotonic_capture_start_qpc: i64,
    pub monotonic_capture_end_qpc: i64,
    pub qpc_frequency: i64,
    pub actual_wall_clock_duration_ms: f64,
    pub expected_duration_from_captured_frames_ms: f64,
    pub expected_duration_in_wav_ms: f64,
    pub captured_sample_frames: u64,
    pub written_sample_frames: u64,
    pub audio_packet_count: u64,
    pub silent_packet_count: u64,
    pub discontinuity_count: u64,
    pub timestamp_error_count: u64,
    pub first_device_position: Option<u64>,
    pub last_device_position: Option<u64>,
    pub first_qpc_position_100ns: Option<u64>,
    pub last_qpc_position_100ns: Option<u64>,
    pub queue_capacity_packets: usize,
    pub maximum_queue_depth: usize,
    pub queue_full_events: u64,
    pub deliberately_dropped_packets: u64,
    pub deliberately_dropped_frames: u64,
}

#[derive(Clone, Copy, Debug, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AudioCaptureState {
    #[default]
    Idle,
    Preparing,
    Recording,
    Finalizing,
    Completed,
    Error,
}

impl AudioCaptureState {
    pub const fn is_active(self) -> bool {
        matches!(self, Self::Preparing | Self::Recording | Self::Finalizing)
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AudioCaptureKind {
    Microphone,
    ProcessLoopback,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioCaptureStatus {
    pub state: AudioCaptureState,
    pub kind: Option<AudioCaptureKind>,
    pub target_label: Option<String>,
    pub output_path: Option<String>,
    pub format: Option<AudioFormatMetadata>,
    pub format_diagnostics: Option<AudioFormatDiagnostics>,
    pub timing: Option<AudioTimingTelemetry>,
    pub error: Option<AudioError>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioCaptureCommandResult {
    pub success: bool,
    pub status: AudioCaptureStatus,
    pub error: Option<AudioError>,
}

#[cfg(test)]
mod tests {
    use super::{AudioCaptureState, AudioFormatMetadata};

    #[test]
    fn sample_count_duration_uses_the_reported_rate() {
        let format = AudioFormatMetadata {
            sample_format: "PCM integer".to_string(),
            format_tag: 1,
            sample_rate: 48_000,
            channel_count: 2,
            bits_per_sample: 16,
            valid_bits_per_sample: Some(16),
            block_align: 4,
            average_bytes_per_second: 192_000,
            channel_mask: None,
            sub_format: None,
        };
        assert_eq!(format.duration_ms_for_frames(48_000), 1_000.0);
        assert_eq!(format.duration_ms_for_frames(24_000), 500.0);
        let json = serde_json::to_value(&format).unwrap();
        assert_eq!(json["sampleRate"], 48_000);
        assert_eq!(json["channelCount"], 2);
        assert_eq!(json["sampleFormat"], "PCM integer");
    }

    #[test]
    fn only_in_progress_capture_states_are_active() {
        assert!(AudioCaptureState::Preparing.is_active());
        assert!(AudioCaptureState::Recording.is_active());
        assert!(AudioCaptureState::Finalizing.is_active());
        assert!(!AudioCaptureState::Idle.is_active());
        assert!(!AudioCaptureState::Completed.is_active());
        assert!(!AudioCaptureState::Error.is_active());
    }
}
