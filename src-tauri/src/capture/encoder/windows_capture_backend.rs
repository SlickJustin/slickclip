use std::path::Path;

use windows_capture::encoder::{
    AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
    VideoSettingsSubType,
};
use windows_capture::frame::Frame;

use super::types::{
    EncodedFrameOutput, EncodedVideoSample, EncoderBackendError, EncoderCodec,
    EncoderFrameTelemetry, VideoEncoderBackend,
};

pub const ENCODER_FRAME_QUEUE_CAPACITY: usize = 8;

/// Existing file-oriented encoder, isolated behind the common backend boundary.
///
/// windows-capture owns the MediaTranscoder and MP4 stream, so this backend intentionally returns
/// no individual encoded samples. A future direct Media Foundation backend will return
/// `EncodedVideoSample` values from the same trait methods.
pub struct WindowsCaptureFileBackend {
    encoder: Option<VideoEncoder>,
}

impl WindowsCaptureFileBackend {
    pub fn create(
        output_path: &Path,
        codec: EncoderCodec,
        width: u32,
        height: u32,
        frame_rate: u32,
    ) -> Result<Box<dyn VideoEncoderBackend>, EncoderBackendError> {
        let subtype = match codec {
            EncoderCodec::Av1 => {
                return Err(EncoderBackendError::new(
                    "AV1 is not exposed by windows-capture 2.0.1.",
                ));
            }
            EncoderCodec::Hevc => VideoSettingsSubType::HEVC,
            EncoderCodec::H264 => VideoSettingsSubType::H264,
        };
        let video_settings = VideoSettingsBuilder::new(width, height)
            .sub_type(subtype)
            .frame_rate(frame_rate)
            .frame_queue_capacity(ENCODER_FRAME_QUEUE_CAPACITY);
        let encoder = VideoEncoder::new(
            video_settings,
            AudioSettingsBuilder::new().disabled(true),
            ContainerSettingsBuilder::new(),
            output_path,
        )
        .map_err(|error| EncoderBackendError::new(error.to_string()))?;

        Ok(Box::new(Self {
            encoder: Some(encoder),
        }))
    }
}

impl VideoEncoderBackend for WindowsCaptureFileBackend {
    fn encode_frame(
        &mut self,
        frame: &mut Frame<'_>,
    ) -> Result<EncodedFrameOutput, EncoderBackendError> {
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| EncoderBackendError::new("The capture encoder was already finalized"))?;
        let result = encoder
            .send_frame_with_result(frame)
            .map_err(|error| EncoderBackendError::new(error.to_string()))?;
        let queue_capacity = encoder.telemetry().snapshot().queue_capacity;

        Ok(EncodedFrameOutput {
            samples: Vec::new(),
            telemetry: EncoderFrameTelemetry {
                queued: result.queued,
                gpu_copy_duration: result.gpu_copy_duration,
                queue_depth: result.queue_depth,
                queue_capacity,
            },
        })
    }

    fn finish(mut self: Box<Self>) -> Result<Vec<EncodedVideoSample>, EncoderBackendError> {
        self.encoder
            .take()
            .ok_or_else(|| EncoderBackendError::new("The capture encoder was already finalized"))?
            .finish()
            .map_err(|error| EncoderBackendError::new(error.to_string()))?;

        Ok(Vec::new())
    }
}
