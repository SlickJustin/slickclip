pub(crate) mod types;
mod windows_capture_backend;

use serde::Serialize;
use windows::core::Interface;
use windows::Storage::Streams::InMemoryRandomAccessStream;
use windows::Win32::Foundation::S_FALSE;
use windows::Win32::System::Com::{CoDecrementMTAUsage, CoIncrementMTAUsage, CO_MTA_USAGE_COOKIE};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows_capture::encoder::{
    AudioSettingsBuilder, ContainerSettingsBuilder, VideoEncoder, VideoSettingsBuilder,
    VideoSettingsSubType,
};

pub use types::{EncoderChoice, EncoderCodec, VideoEncoderBackend};
pub use windows_capture_backend::WindowsCaptureFileBackend;

const PROBE_WIDTH: u32 = 1920;
const PROBE_HEIGHT: u32 = 1080;
const PROBE_FRAME_RATE: u32 = 60;

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderInfo {
    id: String,
    display_name: String,
    codec: String,
    available: bool,
    reason_unavailable: Option<String>,
    recommended: bool,
    preferred: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EncoderCapabilitiesResult {
    success: bool,
    encoders: Vec<EncoderInfo>,
    automatic_encoder_id: Option<String>,
    detection_method: String,
    hardware_acceleration_requested: bool,
    hardware_encoding_verified: bool,
    error_message: Option<String>,
}

pub struct ResolvedEncoder {
    pub actual: EncoderCodec,
}

#[tauri::command]
pub async fn get_encoder_capabilities() -> EncoderCapabilitiesResult {
    match tauri::async_runtime::spawn_blocking(detect_encoder_capabilities).await {
        Ok(Ok(detection)) => detection.into_result(),
        Ok(Err(error)) => EncoderCapabilitiesResult::failure(error),
        Err(error) => EncoderCapabilitiesResult::failure(format!(
            "The encoder capability worker could not complete: {error}"
        )),
    }
}

pub fn resolve_encoder(choice: EncoderChoice) -> Result<ResolvedEncoder, String> {
    let detection = detect_encoder_capabilities()?;
    let actual = match choice {
        EncoderChoice::Automatic => detection.preferred.ok_or_else(|| {
            "No usable JustIn Replay video encoder passed runtime initialization. H.264, the compatibility fallback, is unavailable on this PC.".to_string()
        })?,
        EncoderChoice::Av1 => ensure_available(&detection, EncoderCodec::Av1)?,
        EncoderChoice::Hevc => ensure_available(&detection, EncoderCodec::Hevc)?,
        EncoderChoice::H264 => ensure_available(&detection, EncoderCodec::H264)?,
    };

    Ok(ResolvedEncoder { actual })
}

struct EncoderDetection {
    encoders: Vec<EncoderInfo>,
    preferred: Option<EncoderCodec>,
}

impl EncoderDetection {
    fn into_result(self) -> EncoderCapabilitiesResult {
        EncoderCapabilitiesResult {
            success: true,
            encoders: self.encoders,
            automatic_encoder_id: self.preferred.map(|encoder| encoder.id().to_string()),
            detection_method: format!(
                "windows-capture 2.0.1 in-memory MP4 encoder initialization and finalization at {PROBE_WIDTH}x{PROBE_HEIGHT}, {PROBE_FRAME_RATE} FPS"
            ),
            hardware_acceleration_requested: true,
            hardware_encoding_verified: false,
            error_message: None,
        }
    }
}

impl EncoderCapabilitiesResult {
    fn failure(error: impl Into<String>) -> Self {
        Self {
            success: false,
            encoders: unavailable_encoder_list("Capability detection could not run."),
            automatic_encoder_id: None,
            detection_method: format!(
                "windows-capture 2.0.1 in-memory MP4 encoder initialization and finalization at {PROBE_WIDTH}x{PROBE_HEIGHT}, {PROBE_FRAME_RATE} FPS"
            ),
            hardware_acceleration_requested: true,
            hardware_encoding_verified: false,
            error_message: Some(error.into()),
        }
    }
}

fn detect_encoder_capabilities() -> Result<EncoderDetection, String> {
    let _winrt = WinRtApartment::new()
        .map_err(|error| format!("Could not initialize WinRT for encoder detection: {error}"))?;

    let mut av1 = unavailable_info(
        EncoderChoice::Av1,
        "windows-capture 2.0.1 does not expose an AV1 video subtype, so JustIn Replay cannot initialize or use AV1 through the required capture library.",
    );
    let mut hevc = probe_encoder(EncoderChoice::Hevc, VideoSettingsSubType::HEVC);
    let mut h264 = probe_encoder(EncoderChoice::H264, VideoSettingsSubType::H264);

    let preferred = if av1.available {
        av1.preferred = true;
        Some(EncoderCodec::Av1)
    } else if hevc.available {
        hevc.preferred = true;
        Some(EncoderCodec::Hevc)
    } else if h264.available {
        h264.preferred = true;
        Some(EncoderCodec::H264)
    } else {
        None
    };

    let automatic = EncoderInfo {
        id: EncoderChoice::Automatic.id().to_string(),
        display_name: EncoderChoice::Automatic.display_name().to_string(),
        codec: EncoderChoice::Automatic.codec_name().to_string(),
        available: preferred.is_some(),
        reason_unavailable: preferred
            .is_none()
            .then(|| "No concrete codec passed runtime initialization.".to_string()),
        recommended: true,
        preferred: false,
    };

    Ok(EncoderDetection {
        encoders: vec![automatic, av1, hevc, h264],
        preferred,
    })
}

fn probe_encoder(choice: EncoderChoice, sub_type: VideoSettingsSubType) -> EncoderInfo {
    let probe_result = (|| {
        let stream = InMemoryRandomAccessStream::new()
            .map_err(|error| format!("Could not create the in-memory probe stream: {error}"))?;
        let mut encoder = VideoEncoder::new_from_stream(
            VideoSettingsBuilder::new(PROBE_WIDTH, PROBE_HEIGHT)
                .sub_type(sub_type)
                .frame_rate(PROBE_FRAME_RATE),
            AudioSettingsBuilder::new().disabled(true),
            ContainerSettingsBuilder::new(),
            stream
                .cast()
                .map_err(|error| format!("Could not open the in-memory probe stream: {error}"))?,
        )
        .map_err(|error| format!("Encoder initialization failed: {error}"))?;

        let frame_buffer = vec![0_u8; (PROBE_WIDTH * PROBE_HEIGHT * 4) as usize];
        encoder
            .send_frame_buffer(&frame_buffer, 0)
            .map_err(|error| {
                format!("Encoder rejected the first synthetic probe frame: {error}")
            })?;
        encoder
            .send_frame_buffer(&frame_buffer, 10_000_000 / i64::from(PROBE_FRAME_RATE))
            .map_err(|error| {
                format!("Encoder rejected the second synthetic probe frame: {error}")
            })?;

        encoder
            .finish()
            .map_err(|error| format!("Encoder finalization probe failed: {error}"))
    })();

    match probe_result {
        Ok(()) => available_info(choice),
        Err(error) => unavailable_info(choice, error),
    }
}

fn ensure_available(
    detection: &EncoderDetection,
    codec: EncoderCodec,
) -> Result<EncoderCodec, String> {
    let id = codec.id();
    let info = detection
        .encoders
        .iter()
        .find(|encoder| encoder.id == id)
        .ok_or_else(|| format!("Encoder capability information for {id} is missing."))?;

    if info.available {
        Ok(codec)
    } else {
        Err(format!(
            "{} is unavailable on this PC: {}",
            info.display_name,
            info.reason_unavailable
                .as_deref()
                .unwrap_or("runtime capability detection failed")
        ))
    }
}

fn available_info(choice: EncoderChoice) -> EncoderInfo {
    EncoderInfo {
        id: choice.id().to_string(),
        display_name: choice.display_name().to_string(),
        codec: choice.codec_name().to_string(),
        available: true,
        reason_unavailable: None,
        recommended: false,
        preferred: false,
    }
}

fn unavailable_info(choice: EncoderChoice, reason: impl Into<String>) -> EncoderInfo {
    EncoderInfo {
        id: choice.id().to_string(),
        display_name: choice.display_name().to_string(),
        codec: choice.codec_name().to_string(),
        available: false,
        reason_unavailable: Some(reason.into()),
        recommended: false,
        preferred: false,
    }
}

fn unavailable_encoder_list(reason: &str) -> Vec<EncoderInfo> {
    vec![
        EncoderInfo {
            id: EncoderChoice::Automatic.id().to_string(),
            display_name: EncoderChoice::Automatic.display_name().to_string(),
            codec: EncoderChoice::Automatic.codec_name().to_string(),
            available: false,
            reason_unavailable: Some(reason.to_string()),
            recommended: true,
            preferred: false,
        },
        unavailable_info(EncoderChoice::Av1, reason),
        unavailable_info(EncoderChoice::Hevc, reason),
        unavailable_info(EncoderChoice::H264, reason),
    ]
}

struct WinMtaCookie {
    cookie: CO_MTA_USAGE_COOKIE,
}

impl WinMtaCookie {
    fn new() -> windows::core::Result<Self> {
        Ok(Self {
            cookie: unsafe { CoIncrementMTAUsage()? },
        })
    }
}

impl Drop for WinMtaCookie {
    fn drop(&mut self) {
        let _ = unsafe { CoDecrementMTAUsage(self.cookie) };
    }
}

struct WinRtApartment {
    _cookie: WinMtaCookie,
}

impl WinRtApartment {
    fn new() -> windows::core::Result<Self> {
        let cookie = WinMtaCookie::new()?;
        if let Err(error) = unsafe { RoInitialize(RO_INIT_MULTITHREADED) } {
            if error.code() != S_FALSE {
                return Err(error);
            }
        }

        Ok(Self { _cookie: cookie })
    }
}

impl Drop for WinRtApartment {
    fn drop(&mut self) {
        unsafe { RoUninitialize() };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_encoder_probe_is_internally_consistent() {
        let detection = detect_encoder_capabilities().expect("encoder detection should run");
        for encoder in &detection.encoders {
            eprintln!(
                "{}: available={} reason={}",
                encoder.display_name,
                encoder.available,
                encoder.reason_unavailable.as_deref().unwrap_or("none")
            );
        }

        let automatic = detection
            .encoders
            .iter()
            .find(|encoder| encoder.id == EncoderChoice::Automatic.id())
            .expect("automatic capability should be present");
        assert_eq!(automatic.available, detection.preferred.is_some());

        let av1 = detection
            .encoders
            .iter()
            .find(|encoder| encoder.id == EncoderChoice::Av1.id())
            .expect("AV1 capability should be present");
        assert!(!av1.available, "windows-capture 2.0.1 has no AV1 subtype");
    }
}
