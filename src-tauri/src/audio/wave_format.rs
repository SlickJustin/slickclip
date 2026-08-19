use std::slice;

use windows::core::{Error as WindowsError, GUID, HRESULT};
use windows::Win32::Foundation::E_NOTIMPL;
use windows::Win32::Media::Audio::{IAudioClient, WAVEFORMATEX};
use windows::Win32::Media::KernelStreaming::KSDATAFORMAT_SUBTYPE_PCM;
use windows::Win32::Media::MediaFoundation::MEDIASUBTYPE_IEEE_FLOAT;
use windows::Win32::System::Com::CoTaskMemFree;

use super::types::{AudioError, AudioErrorCode, AudioFormatMetadata};

const WAVE_FORMAT_PCM: u16 = 0x0001;
const WAVE_FORMAT_IEEE_FLOAT: u16 = 0x0003;
const WAVE_FORMAT_EXTENSIBLE: u16 = 0xfffe;
const CHANNELS_STEREO: u16 = 2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProcessClientFormatCandidate {
    Float48KhzStereo,
    Pcm48KhzStereo,
    Pcm44KhzStereo,
}

pub const PROCESS_CLIENT_FORMAT_CANDIDATES: [ProcessClientFormatCandidate; 3] = [
    ProcessClientFormatCandidate::Float48KhzStereo,
    ProcessClientFormatCandidate::Pcm48KhzStereo,
    ProcessClientFormatCandidate::Pcm44KhzStereo,
];

impl ProcessClientFormatCandidate {
    pub fn build(self) -> CaptureWaveFormat {
        match self {
            Self::Float48KhzStereo => CaptureWaveFormat::wave_format_ex(
                WAVE_FORMAT_IEEE_FLOAT,
                48_000,
                CHANNELS_STEREO,
                32,
                "IEEE float",
                false,
            ),
            Self::Pcm48KhzStereo => CaptureWaveFormat::wave_format_ex(
                WAVE_FORMAT_PCM,
                48_000,
                CHANNELS_STEREO,
                16,
                "PCM integer",
                true,
            ),
            Self::Pcm44KhzStereo => CaptureWaveFormat::wave_format_ex(
                WAVE_FORMAT_PCM,
                44_100,
                CHANNELS_STEREO,
                16,
                "PCM integer",
                true,
            ),
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Float48KhzStereo => "48,000 Hz stereo 32-bit IEEE float",
            Self::Pcm48KhzStereo => "48,000 Hz stereo 16-bit PCM",
            Self::Pcm44KhzStereo => "44,100 Hz stereo 16-bit PCM",
        }
    }
}

#[derive(Clone)]
pub struct CaptureWaveFormat {
    pub bytes: Vec<u8>,
    pub metadata: AudioFormatMetadata,
    pub is_pcm: bool,
}

impl CaptureWaveFormat {
    pub fn endpoint_mix_format(audio_client: &IAudioClient) -> Result<Self, AudioError> {
        let format_pointer = unsafe { audio_client.GetMixFormat() }
            .map_err(|error| mix_format_error("read the WASAPI endpoint mix format", error))?;
        Self::from_task_mem(format_pointer)
    }

    pub fn process_mix_format_diagnostic(
        audio_client: &IAudioClient,
    ) -> Result<String, AudioError> {
        match unsafe { audio_client.GetMixFormat() } {
            Ok(format_pointer) => {
                let format = Self::from_task_mem(format_pointer)?;
                Ok(format!(
                    "Available: {} (diagnostic only; explicit client format is still requested)",
                    format.summary()
                ))
            }
            Err(error) if is_expected_process_mix_format_not_implemented(error.code()) => Ok(
                "Not implemented (0x80004001); handled with explicit client-format negotiation"
                    .to_string(),
            ),
            Err(error) => Err(mix_format_error(
                "read the process-loopback GetMixFormat diagnostic",
                error,
            )),
        }
    }

    pub fn as_wave_format_ptr(&self) -> *const WAVEFORMATEX {
        self.bytes.as_ptr().cast()
    }

    pub fn summary(&self) -> String {
        format!(
            "{} Hz / {} channels / {}-bit {}",
            self.metadata.sample_rate,
            self.metadata.channel_count,
            self.metadata.bits_per_sample,
            self.metadata.sample_format
        )
    }

    fn wave_format_ex(
        format_tag: u16,
        sample_rate: u32,
        channel_count: u16,
        bits_per_sample: u16,
        sample_format: &str,
        is_pcm: bool,
    ) -> Self {
        let block_align = channel_count * (bits_per_sample / 8);
        let average_bytes_per_second = sample_rate * u32::from(block_align);
        let mut bytes = Vec::with_capacity(std::mem::size_of::<WAVEFORMATEX>());
        bytes.extend_from_slice(&format_tag.to_le_bytes());
        bytes.extend_from_slice(&channel_count.to_le_bytes());
        bytes.extend_from_slice(&sample_rate.to_le_bytes());
        bytes.extend_from_slice(&average_bytes_per_second.to_le_bytes());
        bytes.extend_from_slice(&block_align.to_le_bytes());
        bytes.extend_from_slice(&bits_per_sample.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());

        Self {
            bytes,
            is_pcm,
            metadata: AudioFormatMetadata {
                sample_format: sample_format.to_string(),
                format_tag,
                sample_rate,
                channel_count,
                bits_per_sample,
                valid_bits_per_sample: None,
                block_align,
                average_bytes_per_second,
                channel_mask: None,
                sub_format: None,
            },
        }
    }

    fn from_task_mem(format_pointer: *mut WAVEFORMATEX) -> Result<Self, AudioError> {
        let allocation = TaskMemWaveFormat::new(format_pointer)?;
        let format = allocation.descriptor();
        let format_tag = format.wFormatTag;
        let channel_count = format.nChannels;
        let sample_rate = format.nSamplesPerSec;
        let average_bytes_per_second = format.nAvgBytesPerSec;
        let block_align = format.nBlockAlign;
        let bits_per_sample = format.wBitsPerSample;
        let extra_size = format.cbSize as usize;
        let byte_count = std::mem::size_of::<WAVEFORMATEX>() + extra_size;
        if byte_count > 256 || sample_rate == 0 || channel_count == 0 || block_align == 0 {
            return Err(AudioError::new(
                AudioErrorCode::CaptureInitializationFailed,
                "WASAPI returned an invalid or unsupported mix-format descriptor.",
            ));
        }
        let bytes = allocation.copy_bytes(byte_count);

        let mut valid_bits_per_sample = None;
        let mut channel_mask = None;
        let mut sub_format = None;
        let mut is_pcm = false;
        let sample_format = if format_tag == WAVE_FORMAT_PCM {
            is_pcm = true;
            "PCM integer".to_string()
        } else if format_tag == WAVE_FORMAT_IEEE_FLOAT {
            "IEEE float".to_string()
        } else if format_tag == WAVE_FORMAT_EXTENSIBLE && bytes.len() >= 40 {
            valid_bits_per_sample = Some(u16::from_le_bytes([bytes[18], bytes[19]]));
            channel_mask = Some(u32::from_le_bytes([
                bytes[20], bytes[21], bytes[22], bytes[23],
            ]));
            let guid = unsafe { (bytes[24..40].as_ptr() as *const GUID).read_unaligned() };
            sub_format = Some(format!("{guid:?}"));
            if guid == KSDATAFORMAT_SUBTYPE_PCM {
                is_pcm = true;
                "PCM integer (extensible)".to_string()
            } else if guid == MEDIASUBTYPE_IEEE_FLOAT {
                "IEEE float (extensible)".to_string()
            } else {
                "WAVEFORMATEXTENSIBLE".to_string()
            }
        } else {
            format!("Windows format tag 0x{format_tag:04x}")
        };

        Ok(Self {
            bytes,
            is_pcm,
            metadata: AudioFormatMetadata {
                sample_format,
                format_tag,
                sample_rate,
                channel_count,
                bits_per_sample,
                valid_bits_per_sample,
                block_align,
                average_bytes_per_second,
                channel_mask,
                sub_format,
            },
        })
    }
}

pub fn is_expected_process_mix_format_not_implemented(code: HRESULT) -> bool {
    code == E_NOTIMPL
}

fn mix_format_error(context: &str, error: WindowsError) -> AudioError {
    AudioError::new(
        AudioErrorCode::CaptureInitializationFailed,
        format!("Could not {context}: {error}"),
    )
}

/// Owns the COM task-allocator result from IAudioClient::GetMixFormat.
/// It is copied before this guard drops, and CoTaskMemFree runs exactly once.
struct TaskMemWaveFormat(*mut WAVEFORMATEX);

impl TaskMemWaveFormat {
    fn new(pointer: *mut WAVEFORMATEX) -> Result<Self, AudioError> {
        if pointer.is_null() {
            return Err(AudioError::new(
                AudioErrorCode::CaptureInitializationFailed,
                "WASAPI returned no shared-mode mix format.",
            ));
        }
        Ok(Self(pointer))
    }

    fn descriptor(&self) -> WAVEFORMATEX {
        unsafe { self.0.read_unaligned() }
    }

    fn copy_bytes(&self, byte_count: usize) -> Vec<u8> {
        unsafe { slice::from_raw_parts(self.0.cast::<u8>(), byte_count) }.to_vec()
    }
}

impl Drop for TaskMemWaveFormat {
    fn drop(&mut self) {
        unsafe { CoTaskMemFree(Some(self.0.cast())) };
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::mem::{align_of, size_of};
    use std::sync::atomic::{AtomicU64, Ordering};

    use windows::Win32::Foundation::{E_FAIL, E_NOTIMPL};
    use windows::Win32::Media::Audio::WAVEFORMATEX;

    use super::{
        is_expected_process_mix_format_not_implemented, ProcessClientFormatCandidate,
        PROCESS_CLIENT_FORMAT_CANDIDATES,
    };
    use crate::audio::wav::WavWriter;

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    #[test]
    fn wave_format_binding_matches_the_packed_windows_abi() {
        assert_eq!(size_of::<WAVEFORMATEX>(), 18);
        assert_eq!(align_of::<WAVEFORMATEX>(), 1);
    }

    #[test]
    fn process_client_format_fallback_order_is_bounded_and_explicit() {
        assert_eq!(
            PROCESS_CLIENT_FORMAT_CANDIDATES,
            [
                ProcessClientFormatCandidate::Float48KhzStereo,
                ProcessClientFormatCandidate::Pcm48KhzStereo,
                ProcessClientFormatCandidate::Pcm44KhzStereo,
            ]
        );
    }

    #[test]
    fn process_client_formats_have_correct_rates_and_byte_geometry() {
        let expected = [
            (48_000, 32, 8, 384_000, 3),
            (48_000, 16, 4, 192_000, 1),
            (44_100, 16, 4, 176_400, 1),
        ];
        for (candidate, expected) in PROCESS_CLIENT_FORMAT_CANDIDATES.into_iter().zip(expected) {
            let format = candidate.build();
            assert_eq!(format.metadata.sample_rate, expected.0);
            assert_eq!(format.metadata.channel_count, 2);
            assert_eq!(format.metadata.bits_per_sample, expected.1);
            assert_eq!(format.metadata.block_align, expected.2);
            assert_eq!(format.metadata.average_bytes_per_second, expected.3);
            assert_eq!(format.metadata.format_tag, expected.4);
            assert_eq!(format.bytes.len(), size_of::<WAVEFORMATEX>());
        }
    }

    #[test]
    fn only_process_get_mix_format_e_notimpl_is_expected() {
        assert!(is_expected_process_mix_format_not_implemented(E_NOTIMPL));
        assert!(!is_expected_process_mix_format_not_implemented(E_FAIL));
    }

    #[test]
    fn wav_headers_preserve_every_supported_process_client_format() {
        for candidate in PROCESS_CLIENT_FORMAT_CANDIDATES {
            let format = candidate.build();
            let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
            let path = std::env::temp_dir().join(format!(
                "justin-replay-process-format-{}-{id}.wav",
                std::process::id()
            ));
            let file = fs::File::create(&path).unwrap();
            let mut writer = WavWriter::create(
                file,
                &format.bytes,
                format.metadata.block_align,
                !format.is_pcm,
            )
            .unwrap();
            writer
                .write_packet(&vec![0; format.metadata.block_align as usize])
                .unwrap();
            writer.finalize().unwrap();

            let bytes = fs::read(&path).unwrap();
            assert_eq!(&bytes[12..16], b"fmt ");
            assert_eq!(u32::from_le_bytes(bytes[16..20].try_into().unwrap()), 18);
            assert_eq!(&bytes[20..38], format.bytes.as_slice());
            let data_offset = bytes
                .windows(4)
                .position(|window| window == b"data")
                .expect("WAV data chunk");
            assert_eq!(
                u32::from_le_bytes(bytes[data_offset + 4..data_offset + 8].try_into().unwrap()),
                u32::from(format.metadata.block_align)
            );
            let _ = fs::remove_file(path);
        }
    }
}
