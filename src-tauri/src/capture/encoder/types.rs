use std::error::Error;
use std::fmt;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use windows_capture::encoder::DetachedFrame;
use windows_capture::frame::Frame;

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EncoderChoice {
    Automatic,
    Av1,
    Hevc,
    H264,
}

impl EncoderChoice {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Automatic => "automatic",
            Self::Av1 => "av1",
            Self::Hevc => "hevc",
            Self::H264 => "h264",
        }
    }

    pub const fn result_name(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Av1 => "AV1",
            Self::Hevc => "HEVC",
            Self::H264 => "H.264",
        }
    }

    pub(super) const fn codec_name(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Av1 => "AV1",
            Self::Hevc => "HEVC",
            Self::H264 => "H.264",
        }
    }

    pub(super) const fn display_name(self) -> &'static str {
        match self {
            Self::Automatic => "Automatic",
            Self::Av1 => "AV1",
            Self::Hevc => "HEVC / H.265",
            Self::H264 => "H.264 / AVC",
        }
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum EncoderCodec {
    Av1,
    Hevc,
    H264,
}

impl EncoderCodec {
    pub const fn id(self) -> &'static str {
        match self {
            Self::Av1 => "av1",
            Self::Hevc => "hevc",
            Self::H264 => "h264",
        }
    }

    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Av1 => "AV1",
            Self::Hevc => "HEVC",
            Self::H264 => "H.264",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CodecInitializationData {
    /// A stable backend-defined name such as `av1C`, `hvcC`, `avcC`, or `annex-b-parameters`.
    pub format: String,
    pub bytes: Vec<u8>,
}

/// Codec-independent encoded output intended for the future replay-buffer boundary.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EncodedVideoSample {
    pub payload: Vec<u8>,
    pub presentation_timestamp_100ns: i64,
    pub duration_100ns: Option<i64>,
    pub clean_point: Option<bool>,
    pub codec: EncoderCodec,
    pub width: u32,
    pub height: u32,
    pub initialization: Option<CodecInitializationData>,
    pub sequence_number: u64,
    pub discontinuity: bool,
}

#[derive(Clone, Copy, Debug)]
pub struct EncoderFrameTelemetry {
    pub queued: bool,
    pub gpu_copy_duration: Option<Duration>,
    pub queue_depth: u64,
    pub queue_capacity: usize,
}

#[allow(dead_code)]
pub struct EncodedFrameOutput {
    pub samples: Vec<EncodedVideoSample>,
    pub telemetry: EncoderFrameTelemetry,
}

#[derive(Debug)]
pub struct EncoderBackendError(String);

impl EncoderBackendError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for EncoderBackendError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for EncoderBackendError {}

/// Encoder boundary used by capture producers. A direct hardware backend can emit zero or more
/// generic encoded samples for each captured frame without exposing Media Foundation objects to
/// its future replay-buffer consumer.
pub trait VideoEncoderBackend: Send {
    fn encode_frame(
        &mut self,
        frame: &mut Frame<'_>,
    ) -> Result<EncodedFrameOutput, EncoderBackendError>;

    fn encode_detached_frame(
        &mut self,
        frame: &DetachedFrame,
        presentation_timestamp_100ns: i64,
    ) -> Result<EncodedFrameOutput, EncoderBackendError>;

    fn finish(self: Box<Self>) -> Result<Vec<EncodedVideoSample>, EncoderBackendError>;
}
