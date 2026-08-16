//! Temporary Stage 5.1 feasibility probe.
//!
//! This deliberately does not participate in the production capture path. It requests the
//! Windows 11 24H2 AV1 output subtype directly and transcodes synthetic BGRA frames into an
//! in-memory MP4 stream.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use windows::core::{Error as WindowsError, HSTRING};
use windows::Foundation::Metadata::ApiInformation;
use windows::Foundation::{TimeSpan, TypedEventHandler};
use windows::Media::Core::{
    MediaStreamSample, MediaStreamSource, MediaStreamSourceSampleRequestedEventArgs,
    MediaStreamSourceStartingEventArgs, VideoStreamDescriptor,
};
use windows::Media::MediaProperties::{
    ContainerEncodingProperties, MediaEncodingProfile, MediaEncodingSubtypes,
    VideoEncodingProperties,
};
use windows::Media::Transcoding::MediaTranscoder;
use windows::Security::Cryptography::CryptographicBuffer;
use windows::Storage::Streams::InMemoryRandomAccessStream;
use windows::Win32::Foundation::{E_FAIL, S_FALSE};
use windows::Win32::Media::MediaFoundation::{
    MFMediaType_Video, MFShutdown, MFStartup, MFTEnumEx, MFVideoFormat_AV1, MFSTARTUP_FULL,
    MFT_CATEGORY_VIDEO_ENCODER, MFT_ENUM_FLAG_HARDWARE, MFT_ENUM_FLAG_SORTANDFILTER,
    MFT_ENUM_FLAG_TRANSCODE_ONLY, MFT_REGISTER_TYPE_INFO, MF_VERSION,
};
use windows::Win32::System::Com::{
    CoDecrementMTAUsage, CoIncrementMTAUsage, CoTaskMemFree, CO_MTA_USAGE_COOKIE,
};
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};

const PROBE_WIDTH: u32 = 1920;
const PROBE_HEIGHT: u32 = 1080;
const PROBE_FRAME_RATE: u32 = 60;
const PROBE_BITRATE: u32 = 15_000_000;
const FRAME_DURATION: i64 = 10_000_000 / PROBE_FRAME_RATE as i64;

#[derive(Debug)]
struct Av1ProbeSuccess {
    subtype: String,
    encoded_bytes: u64,
    hardware_mft_count: Option<u32>,
    hardware_query_error: Option<String>,
}

fn run_av1_probe() -> Result<Av1ProbeSuccess, String> {
    let _winrt = WinRtApartment::new()
        .map_err(|error| format!("Could not initialize WinRT for the AV1 probe: {error}"))?;

    ensure_av1_api_is_present()?;
    let hardware_query = count_hardware_av1_encoder_mfts();
    let mut success = transcode_synthetic_frames()
        .map_err(|error| format!("AV1 transcode probe failed: {error}"))?;
    match hardware_query {
        Ok(count) => success.hardware_mft_count = Some(count),
        Err(error) => success.hardware_query_error = Some(error),
    }

    Ok(success)
}

fn ensure_av1_api_is_present() -> Result<(), String> {
    let checks = [
        (
            "Windows.Media.Core.CodecSubtypes",
            "VideoFormatAv1",
            "CodecSubtypes.VideoFormatAv1",
        ),
        (
            "Windows.Media.MediaProperties.MediaEncodingSubtypes",
            "Av1",
            "MediaEncodingSubtypes.Av1",
        ),
    ];

    for (type_name, property_name, display_name) in checks {
        let present = ApiInformation::IsPropertyPresent(
            &HSTRING::from(type_name),
            &HSTRING::from(property_name),
        )
        .map_err(|error| format!("Could not query {display_name}: {error}"))?;

        if !present {
            return Err(format!(
                "Windows does not expose {display_name}. The AV1 MediaTranscoder path requires Windows 11 24H2 (build 26100) or newer."
            ));
        }
    }

    Ok(())
}

fn transcode_synthetic_frames() -> windows::core::Result<Av1ProbeSuccess> {
    let av1_subtype = MediaEncodingSubtypes::Av1()?;
    let bgra_subtype = MediaEncodingSubtypes::Bgra8()?;
    let mp4_subtype = MediaEncodingSubtypes::Mpeg4()?;

    let output_video = VideoEncodingProperties::new()?;
    output_video.SetSubtype(&av1_subtype)?;
    output_video.SetBitrate(PROBE_BITRATE)?;
    output_video.SetWidth(PROBE_WIDTH)?;
    output_video.SetHeight(PROBE_HEIGHT)?;
    output_video.FrameRate()?.SetNumerator(PROBE_FRAME_RATE)?;
    output_video.FrameRate()?.SetDenominator(1)?;
    output_video.PixelAspectRatio()?.SetNumerator(1)?;
    output_video.PixelAspectRatio()?.SetDenominator(1)?;

    let container = ContainerEncodingProperties::new()?;
    container.SetSubtype(&mp4_subtype)?;

    let profile = MediaEncodingProfile::new()?;
    profile.SetVideo(&output_video)?;
    profile.SetContainer(&container)?;

    let input_video =
        VideoEncodingProperties::CreateUncompressed(&bgra_subtype, PROBE_WIDTH, PROBE_HEIGHT)?;
    let descriptor = VideoStreamDescriptor::Create(&input_video)?;
    let source = MediaStreamSource::CreateFromDescriptor(&descriptor)?;
    source.SetBufferTime(Duration::from_millis(30).into())?;

    let starting_token = source.Starting(&TypedEventHandler::<
        MediaStreamSource,
        MediaStreamSourceStartingEventArgs,
    >::new(|_, args| {
        let args = args
            .as_ref()
            .ok_or_else(|| callback_error("AV1 probe Starting callback received no arguments"))?;
        args.Request()?
            .SetActualStartPosition(TimeSpan { Duration: 0 })
    }))?;

    let frame_bytes = (PROBE_WIDTH * PROBE_HEIGHT * 4) as usize;
    let frames = Arc::new(Mutex::new(VecDeque::from([
        (vec![0_u8; frame_bytes], TimeSpan { Duration: 0 }),
        (
            vec![0_u8; frame_bytes],
            TimeSpan {
                Duration: FRAME_DURATION,
            },
        ),
        (
            vec![0_u8; frame_bytes],
            TimeSpan {
                Duration: FRAME_DURATION * 2,
            },
        ),
    ])));

    let sample_token = source.SampleRequested(&TypedEventHandler::<
        MediaStreamSource,
        MediaStreamSourceSampleRequestedEventArgs,
    >::new({
        let frames = Arc::clone(&frames);
        move |_, args| {
            let args = args.as_ref().ok_or_else(|| {
                callback_error("AV1 probe SampleRequested callback received no arguments")
            })?;
            let request = args.Request()?;
            let next_frame = frames
                .lock()
                .map_err(|_| callback_error("AV1 probe frame queue was poisoned"))?
                .pop_front();

            if let Some((bytes, timestamp)) = next_frame {
                let buffer = CryptographicBuffer::CreateFromByteArray(&bytes)?;
                let sample = MediaStreamSample::CreateFromBuffer(&buffer, timestamp)?;
                sample.SetDuration(TimeSpan {
                    Duration: FRAME_DURATION,
                })?;
                request.SetSample(&sample)?;
            } else {
                request.SetSample(None)?;
            }

            Ok(())
        }
    }))?;

    let output = InMemoryRandomAccessStream::new()?;
    let transcoder = MediaTranscoder::new()?;
    transcoder.SetHardwareAccelerationEnabled(true)?;

    let prepared = transcoder
        .PrepareMediaStreamSourceTranscodeAsync(&source, &output, &profile)?
        .join()?;
    if !prepared.CanTranscode()? {
        return Err(callback_error(format!(
            "MediaTranscoder rejected the AV1 profile: {:?}",
            prepared.FailureReason()?
        )));
    }

    prepared.TranscodeAsync()?.join()?;
    let encoded_bytes = output.Size()?;
    if encoded_bytes == 0 {
        return Err(callback_error(
            "MediaTranscoder completed but produced an empty AV1 output stream",
        ));
    }

    source.RemoveSampleRequested(sample_token)?;
    source.RemoveStarting(starting_token)?;

    Ok(Av1ProbeSuccess {
        subtype: av1_subtype.to_string_lossy(),
        encoded_bytes,
        hardware_mft_count: None,
        hardware_query_error: None,
    })
}

fn count_hardware_av1_encoder_mfts() -> Result<u32, String> {
    let _media_foundation = MediaFoundation::new()
        .map_err(|error| format!("Could not initialize Media Foundation: {error}"))?;
    let output_type = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_AV1,
    };
    let mut activations = std::ptr::null_mut();
    let mut count = 0;

    unsafe {
        MFTEnumEx(
            MFT_CATEGORY_VIDEO_ENCODER,
            MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_TRANSCODE_ONLY | MFT_ENUM_FLAG_SORTANDFILTER,
            None,
            Some(&output_type),
            &mut activations,
            &mut count,
        )
        .map_err(|error| format!("Could not enumerate hardware AV1 encoder MFTs: {error}"))?;

        if !activations.is_null() {
            let activation_slice = std::slice::from_raw_parts_mut(activations, count as usize);
            for activation in activation_slice {
                drop(activation.take());
            }
            CoTaskMemFree(Some(activations.cast()));
        }
    }

    Ok(count)
}

fn callback_error(message: impl AsRef<str>) -> WindowsError {
    WindowsError::new(E_FAIL, message.as_ref())
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

struct MediaFoundation;

impl MediaFoundation {
    fn new() -> windows::core::Result<Self> {
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_FULL)? };
        Ok(Self)
    }
}

impl Drop for MediaFoundation {
    fn drop(&mut self) {
        let _ = unsafe { MFShutdown() };
    }
}

#[test]
fn windows_24h2_av1_media_transcoder_probe() {
    match run_av1_probe() {
        Ok(result) => println!(
            "AV1_PROBE_RESULT: available=true subtype={} encoded_bytes={} hardware_mft_count={} hardware_query_error={} transcoder_hardware_verified=false",
            result.subtype,
            result.encoded_bytes,
            result
                .hardware_mft_count
                .map_or_else(|| "unknown".to_string(), |count| count.to_string()),
            result.hardware_query_error.as_deref().unwrap_or("none")
        ),
        Err(error) => {
            println!("AV1_PROBE_RESULT: available=false hardware_verified=false error={error}")
        }
    }
}
