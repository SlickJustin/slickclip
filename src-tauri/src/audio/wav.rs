use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use super::types::{AudioError, AudioErrorCode};

const MAX_WAV_DATA_BYTES: u64 = u32::MAX as u64 - 64;

pub struct WavWriter {
    file: File,
    data_size_offset: u64,
    fact_sample_count_offset: Option<u64>,
    data_bytes: u64,
    block_align: u16,
}

impl WavWriter {
    pub fn create(
        file: File,
        format_chunk: &[u8],
        block_align: u16,
        requires_fact_chunk: bool,
    ) -> Result<Self, AudioError> {
        if format_chunk.is_empty() || format_chunk.len() > u32::MAX as usize || block_align == 0 {
            return Err(wav_error("The WASAPI format chunk has an invalid size."));
        }
        let mut writer = Self {
            file,
            data_size_offset: 0,
            fact_sample_count_offset: None,
            data_bytes: 0,
            block_align,
        };
        writer.write_header(format_chunk, requires_fact_chunk)?;
        Ok(writer)
    }

    fn write_header(
        &mut self,
        format_chunk: &[u8],
        requires_fact_chunk: bool,
    ) -> Result<(), AudioError> {
        self.file.write_all(b"RIFF").map_err(io_error)?;
        self.file.write_all(&0u32.to_le_bytes()).map_err(io_error)?;
        self.file.write_all(b"WAVEfmt ").map_err(io_error)?;
        self.file
            .write_all(&(format_chunk.len() as u32).to_le_bytes())
            .map_err(io_error)?;
        self.file.write_all(format_chunk).map_err(io_error)?;
        if format_chunk.len() % 2 != 0 {
            self.file.write_all(&[0]).map_err(io_error)?;
        }
        if requires_fact_chunk {
            self.file.write_all(b"fact").map_err(io_error)?;
            self.file.write_all(&4u32.to_le_bytes()).map_err(io_error)?;
            self.fact_sample_count_offset = Some(self.file.stream_position().map_err(io_error)?);
            self.file.write_all(&0u32.to_le_bytes()).map_err(io_error)?;
        }
        self.file.write_all(b"data").map_err(io_error)?;
        self.data_size_offset = self.file.stream_position().map_err(io_error)?;
        self.file.write_all(&0u32.to_le_bytes()).map_err(io_error)?;
        Ok(())
    }

    pub fn write_packet(&mut self, bytes: &[u8]) -> Result<(), AudioError> {
        let next_size = self.data_bytes.saturating_add(bytes.len() as u64);
        if next_size > MAX_WAV_DATA_BYTES {
            return Err(wav_error("The WAV test exceeded the RIFF 4 GB size limit."));
        }
        self.file.write_all(bytes).map_err(io_error)?;
        self.data_bytes = next_size;
        Ok(())
    }

    pub fn finalize(mut self) -> Result<u64, AudioError> {
        if self.data_bytes % 2 != 0 {
            self.file.write_all(&[0]).map_err(io_error)?;
        }
        let file_size = self.file.seek(SeekFrom::End(0)).map_err(io_error)?;
        let riff_size = u32::try_from(file_size.saturating_sub(8))
            .map_err(|_| wav_error("The completed WAV file is too large for RIFF."))?;
        self.file.seek(SeekFrom::Start(4)).map_err(io_error)?;
        self.file
            .write_all(&riff_size.to_le_bytes())
            .map_err(io_error)?;
        self.file
            .seek(SeekFrom::Start(self.data_size_offset))
            .map_err(io_error)?;
        self.file
            .write_all(&(self.data_bytes as u32).to_le_bytes())
            .map_err(io_error)?;
        if let Some(offset) = self.fact_sample_count_offset {
            let sample_frames = u32::try_from(self.data_bytes / self.block_align as u64)
                .map_err(|_| wav_error("The WAV sample-frame count exceeds RIFF limits."))?;
            self.file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
            self.file
                .write_all(&sample_frames.to_le_bytes())
                .map_err(io_error)?;
        }
        self.file.flush().map_err(io_error)?;
        Ok(self.data_bytes)
    }
}

pub fn reserve_wav_path(
    directory: &Path,
    prefix: &str,
    timestamp: &str,
) -> Result<(PathBuf, File), AudioError> {
    std::fs::create_dir_all(directory).map_err(io_error)?;
    let safe_prefix = safe_filename_component(prefix);
    for suffix in 1..=999u32 {
        let name = if suffix == 1 {
            format!("{safe_prefix}-{timestamp}.wav")
        } else {
            format!("{safe_prefix}-{timestamp}-{suffix}.wav")
        };
        let path = directory.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((path, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(io_error(error)),
        }
    }
    Err(wav_error(
        "Could not reserve a collision-safe audio test filename.",
    ))
}

pub fn safe_filename_component(value: &str) -> String {
    let mut result = String::new();
    let mut previous_separator = false;
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
            result.push(character);
            previous_separator = false;
        } else if !result.is_empty() && !previous_separator {
            result.push('-');
            previous_separator = true;
        }
        if result.len() >= 48 {
            break;
        }
    }
    let result = result.trim_matches('-').to_string();
    if result.is_empty() {
        "audio".to_string()
    } else {
        result
    }
}

pub fn utc_file_timestamp() -> Result<String, AudioError> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| wav_error(format!("The system clock is invalid: {error}")))?
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let seconds_in_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_date_from_days(days);
    let hour = seconds_in_day / 3_600;
    let minute = (seconds_in_day % 3_600) / 60;
    let second = seconds_in_day % 60;
    Ok(format!(
        "{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}"
    ))
}

fn civil_date_from_days(days_since_unix_epoch: i64) -> (i64, i64, i64) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    (year, month, day)
}

fn io_error(error: std::io::Error) -> AudioError {
    wav_error(format!("WAV output failed: {error}"))
}

fn wav_error(message: impl Into<String>) -> AudioError {
    AudioError::new(AudioErrorCode::WavOutputFailed, message)
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::{reserve_wav_path, safe_filename_component, WavWriter};

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn test_directory() -> std::path::PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!("slickclip-wav-test-{}-{id}", std::process::id()))
    }

    #[test]
    fn sanitizes_process_names_for_safe_filenames() {
        assert_eq!(safe_filename_component("Discord.exe"), "Discord-exe");
        assert_eq!(safe_filename_component("../../bad:*name"), "bad-name");
        assert_eq!(safe_filename_component("***"), "audio");
    }

    #[test]
    fn finalizes_riff_and_data_sizes() {
        let directory = test_directory();
        let (path, file) = reserve_wav_path(&directory, "mic", "20260101-000000").unwrap();
        let format = [1, 0, 2, 0, 0x80, 0xbb, 0, 0, 0, 0xee, 2, 0, 4, 0, 16, 0];
        let mut writer = WavWriter::create(file, &format, 4, false).unwrap();
        writer.write_packet(&[1, 2, 3, 4]).unwrap();
        assert_eq!(writer.finalize().unwrap(), 4);

        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(bytes[4..8].try_into().unwrap()), 40);
        assert_eq!(&bytes[36..40], b"data");
        assert_eq!(u32::from_le_bytes(bytes[40..44].try_into().unwrap()), 4);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn non_pcm_wav_includes_and_finalizes_the_fact_chunk() {
        let directory = test_directory();
        let (path, file) = reserve_wav_path(&directory, "float", "stamp").unwrap();
        let format = [3, 0, 2, 0, 0x80, 0xbb, 0, 0, 0, 0xdc, 5, 0, 8, 0, 32, 0];
        let mut writer = WavWriter::create(file, &format, 8, true).unwrap();
        writer.write_packet(&[0; 16]).unwrap();
        writer.finalize().unwrap();

        let bytes = fs::read(&path).unwrap();
        assert_eq!(&bytes[36..40], b"fact");
        assert_eq!(u32::from_le_bytes(bytes[44..48].try_into().unwrap()), 2);
        assert_eq!(&bytes[48..52], b"data");
        assert_eq!(u32::from_le_bytes(bytes[52..56].try_into().unwrap()), 16);
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn collision_reservation_uses_a_numbered_suffix() {
        let directory = test_directory();
        let (first, first_file) = reserve_wav_path(&directory, "mic", "stamp").unwrap();
        drop(first_file);
        let (second, second_file) = reserve_wav_path(&directory, "mic", "stamp").unwrap();
        drop(second_file);
        assert_eq!(first.file_name().unwrap(), "mic-stamp.wav");
        assert_eq!(second.file_name().unwrap(), "mic-stamp-2.wav");
        let _ = fs::remove_dir_all(directory);
    }
}
