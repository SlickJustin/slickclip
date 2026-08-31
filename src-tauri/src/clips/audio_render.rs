use std::fs::{self, File, OpenOptions};
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::audio::{AudioFormatMetadata, WavWriter};
use crate::replay::{AudioSnapshotTrack, AudioTrackRole, AudioTrackState, SavedReplayTimeline};

const MEDIA_TIME_BASE: i128 = 10_000_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioRenderDiagnostics {
    pub track_role: AudioTrackRole,
    pub selected_segment_sequence_numbers: Vec<u64>,
    pub source_format: AudioFormatMetadata,
    pub source_frames_available: u64,
    pub frames_trimmed_before: u64,
    pub frames_trimmed_after: u64,
    pub leading_silence_frames: u64,
    pub trailing_silence_frames: u64,
    pub rendered_frame_count: u64,
    pub rendered_duration_seconds: f64,
    pub rendered_wav_size: u64,
    pub warnings: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct RenderedAudioTrack {
    pub track_role: AudioTrackRole,
    pub path: PathBuf,
    pub diagnostics: AudioRenderDiagnostics,
}

pub fn render_audio_tracks(
    tracks: &[AudioSnapshotTrack],
    timeline: &SavedReplayTimeline,
    workspace: &Path,
) -> Result<Vec<RenderedAudioTrack>, String> {
    let mut ordered = tracks
        .iter()
        .filter(|track| !track.segments.is_empty())
        .collect::<Vec<_>>();
    ordered.sort_by_key(|track| track.track_role);

    let mut rendered = Vec::with_capacity(ordered.len());
    for track in ordered {
        rendered.push(render_track(track, timeline, workspace)?);
    }
    Ok(rendered)
}

fn render_track(
    track: &AudioSnapshotTrack,
    timeline: &SavedReplayTimeline,
    workspace: &Path,
) -> Result<RenderedAudioTrack, String> {
    let first = track.segments.first().ok_or_else(|| {
        format!(
            "{:?} has no finalized WAV material in the selected replay.",
            track.track_role
        )
    })?;
    let format = first.format.clone();
    validate_format(&format)?;
    if track.format.as_ref().is_some_and(|value| value != &format) {
        return Err(format!(
            "{:?} snapshot format does not match its first WAV segment.",
            track.track_role
        ));
    }

    let target_frames =
        duration_to_frames(timeline.clip_playback_duration_100ns, format.sample_rate)?;
    let target_bytes = usize::try_from(
        u128::from(target_frames)
            .checked_mul(u128::from(format.block_align))
            .ok_or_else(|| "Rendered audio size overflowed.".to_string())?,
    )
    .map_err(|_| "Rendered audio is too large for this process.".to_string())?;
    let mut output_bytes = vec![0u8; target_bytes];
    let mut source_frames_available = 0u64;
    let mut frames_trimmed_before = 0u64;
    let mut frames_trimmed_after = 0u64;
    let mut first_written_frame = None::<u64>;
    let mut last_written_frame = None::<u64>;
    let mut previous_sequence = None;
    let mut previous_source_end_frame = None::<i64>;
    let mut first_format_chunk = None::<Vec<u8>>;
    let mut warnings = Vec::new();
    if track.source_state != AudioTrackState::Running {
        warnings.push(format!(
            "Source state at snapshot was {:?}{}.",
            track.source_state,
            track
                .source_error_message
                .as_deref()
                .map(|error| format!(": {error}"))
                .unwrap_or_default()
        ));
    }

    for segment in &track.segments {
        if segment.track_role != track.track_role {
            return Err(format!(
                "Audio segment {} belongs to {:?}, not {:?}.",
                segment.sequence_number, segment.track_role, track.track_role
            ));
        }
        if !segment.finalized || !segment.is_consistent() {
            return Err(format!(
                "{:?} audio segment {} is not a consistent finalized WAV.",
                track.track_role, segment.sequence_number
            ));
        }
        if segment.format != format {
            return Err(format!(
                "{:?} audio changed format at segment {}. Raw frames cannot be concatenated safely.",
                track.track_role, segment.sequence_number
            ));
        }
        if previous_sequence.is_some_and(|value| segment.sequence_number <= value) {
            return Err(format!(
                "{:?} audio segments are not in increasing sequence order at {}.",
                track.track_role, segment.sequence_number
            ));
        }
        previous_sequence = Some(segment.sequence_number);

        let parsed = parse_wav(Path::new(&segment.file_path))?;
        validate_fmt_chunk(&parsed.format_chunk, &format)?;
        if let Some(expected) = &first_format_chunk {
            if expected != &parsed.format_chunk {
                return Err(format!(
                    "{:?} WAV format chunk changed at segment {}.",
                    track.track_role, segment.sequence_number
                ));
            }
        } else {
            first_format_chunk = Some(parsed.format_chunk.clone());
        }
        let actual_file_size = fs::metadata(&segment.file_path)
            .map_err(|error| {
                format!(
                    "Could not inspect finalized audio segment '{}': {error}",
                    segment.file_path
                )
            })?
            .len();
        if actual_file_size != segment.file_size {
            return Err(format!(
                "Audio segment {} size changed after finalization (expected {}, found {}).",
                segment.sequence_number, segment.file_size, actual_file_size
            ));
        }
        if parsed.data.len() % usize::from(format.block_align) != 0 {
            return Err(format!(
                "Audio segment {} data is not block aligned.",
                segment.sequence_number
            ));
        }
        let segment_frames = u64::try_from(parsed.data.len() / usize::from(format.block_align))
            .map_err(|_| "WAV frame count overflowed.".to_string())?;
        if segment_frames != segment.written_sample_frames {
            return Err(format!(
                "Audio segment {} metadata reports {} frames but its WAV contains {}.",
                segment.sequence_number, segment.written_sample_frames, segment_frames
            ));
        }
        source_frames_available = source_frames_available.saturating_add(segment_frames);

        let segment_start_frame = time_delta_to_frames(
            segment
                .start_qpc_100ns
                .saturating_sub(timeline.clip_capture_start_qpc_100ns),
            format.sample_rate,
        )?;
        if let Some(previous_end) = previous_source_end_frame {
            let difference = segment_start_frame.saturating_sub(previous_end);
            if difference.abs() > 1 {
                warnings.push(format!(
                    "Segments before {} have an interior {}-frame {}.",
                    segment.sequence_number,
                    difference.abs(),
                    if difference > 0 { "gap" } else { "overlap" }
                ));
            }
        }
        previous_source_end_frame = Some(
            segment_start_frame.saturating_add(i64::try_from(segment_frames).unwrap_or(i64::MAX)),
        );

        if segment.discontinuity_count > 0
            || segment.timestamp_error_count > 0
            || segment.dropped_packet_count > 0
            || segment.dropped_frame_count > 0
        {
            warnings.push(format!(
                "Segment {} reports discontinuities {}, timestamp errors {}, dropped packets {}, dropped frames {}.",
                segment.sequence_number,
                segment.discontinuity_count,
                segment.timestamp_error_count,
                segment.dropped_packet_count,
                segment.dropped_frame_count
            ));
        }

        let segment_duration_100ns = frames_to_duration(segment_frames, format.sample_rate)?;
        let segment_end_qpc_100ns = segment
            .start_qpc_100ns
            .saturating_add(segment_duration_100ns);
        let mut copied_from_segment = 0u64;
        for video in &timeline.segment_maps {
            let intersection_start = segment.start_qpc_100ns.max(video.session_start_qpc_100ns);
            let intersection_end = segment_end_qpc_100ns.min(video.session_end_qpc_100ns);
            if intersection_end <= intersection_start {
                continue;
            }
            let source_start = duration_to_frames(
                intersection_start.saturating_sub(segment.start_qpc_100ns),
                format.sample_rate,
            )?
            .min(segment_frames);
            let destination_start = duration_to_frames(
                video.clip_start_100ns.saturating_add(
                    intersection_start.saturating_sub(video.session_start_qpc_100ns),
                ),
                format.sample_rate,
            )?
            .min(target_frames);
            let intersection_frames = duration_to_frames(
                intersection_end.saturating_sub(intersection_start),
                format.sample_rate,
            )?;
            let copied_frames = intersection_frames
                .min(segment_frames.saturating_sub(source_start))
                .min(target_frames.saturating_sub(destination_start));
            if copied_frames == 0 {
                continue;
            }
            let block_align = usize::from(format.block_align);
            let source_byte_start = usize::try_from(source_start)
                .ok()
                .and_then(|value| value.checked_mul(block_align))
                .ok_or_else(|| "Source WAV byte offset overflowed.".to_string())?;
            let destination_byte_start = usize::try_from(destination_start)
                .ok()
                .and_then(|value| value.checked_mul(block_align))
                .ok_or_else(|| "Rendered WAV byte offset overflowed.".to_string())?;
            let byte_count = usize::try_from(copied_frames)
                .ok()
                .and_then(|value| value.checked_mul(block_align))
                .ok_or_else(|| "Rendered WAV copy size overflowed.".to_string())?;
            output_bytes[destination_byte_start..destination_byte_start + byte_count]
                .copy_from_slice(&parsed.data[source_byte_start..source_byte_start + byte_count]);
            copied_from_segment = copied_from_segment.saturating_add(copied_frames);
            first_written_frame = Some(
                first_written_frame
                    .map(|value| value.min(destination_start))
                    .unwrap_or(destination_start),
            );
            let written_end = destination_start.saturating_add(copied_frames);
            last_written_frame = Some(
                last_written_frame
                    .map(|value| value.max(written_end))
                    .unwrap_or(written_end),
            );
        }
        let not_copied = segment_frames.saturating_sub(copied_from_segment.min(segment_frames));
        let before_end = segment_end_qpc_100ns.min(timeline.clip_capture_start_qpc_100ns);
        let before_frames = if before_end > segment.start_qpc_100ns {
            duration_to_frames(
                before_end.saturating_sub(segment.start_qpc_100ns),
                format.sample_rate,
            )?
            .min(not_copied)
        } else {
            0
        };
        frames_trimmed_before = frames_trimmed_before.saturating_add(before_frames);
        frames_trimmed_after =
            frames_trimmed_after.saturating_add(not_copied.saturating_sub(before_frames));
    }

    let leading_silence_frames = first_written_frame
        .unwrap_or(target_frames)
        .min(target_frames);
    let trailing_silence_frames = target_frames.saturating_sub(last_written_frame.unwrap_or(0));
    if leading_silence_frames > 0 {
        warnings.push(format!(
            "Inserted {leading_silence_frames} leading silence frames because source material begins after clip zero."
        ));
    }
    if trailing_silence_frames > 0 {
        warnings.push(format!(
            "Inserted {trailing_silence_frames} trailing silence frames because source material ends before the video."
        ));
    }

    let output_path = workspace.join(format!(
        "{}-rendered.wav",
        track.track_role.directory_name()
    ));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&output_path)
        .map_err(|error| {
            format!(
                "Could not create rendered audio '{}': {error}",
                output_path.display()
            )
        })?;
    let mut writer = WavWriter::create(
        file,
        first_format_chunk
            .as_deref()
            .ok_or_else(|| "No WAV format chunk was available.".to_string())?,
        format.block_align,
        !format.sample_format.starts_with("PCM"),
    )
    .map_err(|error| error.message)?;
    writer
        .write_packet(&output_bytes)
        .map_err(|error| error.message)?;
    writer.finalize().map_err(|error| error.message)?;
    let rendered_wav_size = fs::metadata(&output_path)
        .map_err(|error| format!("Could not inspect rendered WAV: {error}"))?
        .len();

    Ok(RenderedAudioTrack {
        track_role: track.track_role,
        path: output_path,
        diagnostics: AudioRenderDiagnostics {
            track_role: track.track_role,
            selected_segment_sequence_numbers: track
                .segments
                .iter()
                .map(|segment| segment.sequence_number)
                .collect(),
            source_format: format.clone(),
            source_frames_available,
            frames_trimmed_before,
            frames_trimmed_after,
            leading_silence_frames,
            trailing_silence_frames,
            rendered_frame_count: target_frames,
            rendered_duration_seconds: target_frames as f64 / f64::from(format.sample_rate),
            rendered_wav_size,
            warnings,
        },
    })
}

fn validate_format(format: &AudioFormatMetadata) -> Result<(), String> {
    if format.sample_rate == 0
        || format.channel_count == 0
        || format.block_align == 0
        || format.bits_per_sample == 0
    {
        return Err("Audio snapshot contains an invalid source format.".to_string());
    }
    Ok(())
}

fn duration_to_frames(duration_100ns: i64, sample_rate: u32) -> Result<u64, String> {
    if duration_100ns < 0 {
        return Err("Audio render duration cannot be negative.".to_string());
    }
    u64::try_from(round_time_product(i128::from(duration_100ns), sample_rate))
        .map_err(|_| "Audio render frame count overflowed.".to_string())
}

fn frames_to_duration(frames: u64, sample_rate: u32) -> Result<i64, String> {
    if sample_rate == 0 {
        return Err("Audio sample rate cannot be zero.".to_string());
    }
    Ok(
        ((i128::from(frames) * MEDIA_TIME_BASE) / i128::from(sample_rate))
            .clamp(0, i128::from(i64::MAX)) as i64,
    )
}

fn time_delta_to_frames(delta_100ns: i64, sample_rate: u32) -> Result<i64, String> {
    i64::try_from(round_time_product(i128::from(delta_100ns), sample_rate))
        .map_err(|_| "Audio placement frame index overflowed.".to_string())
}

fn round_time_product(time_100ns: i128, sample_rate: u32) -> i128 {
    let product = time_100ns.saturating_mul(i128::from(sample_rate));
    if product >= 0 {
        product.saturating_add(MEDIA_TIME_BASE / 2) / MEDIA_TIME_BASE
    } else {
        -product.saturating_neg().saturating_add(MEDIA_TIME_BASE / 2) / MEDIA_TIME_BASE
    }
}

struct ParsedWav {
    format_chunk: Vec<u8>,
    data: Vec<u8>,
}

fn parse_wav(path: &Path) -> Result<ParsedWav, String> {
    let mut file = File::open(path)
        .map_err(|error| format!("Could not open finalized WAV '{}': {error}", path.display()))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read finalized WAV '{}': {error}", path.display()))?;
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(format!("'{}' is not a RIFF/WAVE file.", path.display()));
    }
    let declared_size = usize::try_from(u32::from_le_bytes(bytes[4..8].try_into().unwrap()))
        .unwrap_or(usize::MAX)
        .saturating_add(8);
    if declared_size != bytes.len() {
        return Err(format!(
            "WAV '{}' has an inconsistent RIFF size.",
            path.display()
        ));
    }

    let mut offset = 12usize;
    let mut format_chunk = None;
    let mut data = None;
    while offset.saturating_add(8) <= bytes.len() {
        let identifier = &bytes[offset..offset + 4];
        let size = usize::try_from(u32::from_le_bytes(
            bytes[offset + 4..offset + 8].try_into().unwrap(),
        ))
        .unwrap_or(usize::MAX);
        let chunk_start = offset + 8;
        let chunk_end = chunk_start
            .checked_add(size)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| format!("WAV '{}' contains a truncated chunk.", path.display()))?;
        if identifier == b"fmt " {
            format_chunk = Some(bytes[chunk_start..chunk_end].to_vec());
        } else if identifier == b"data" {
            data = Some(bytes[chunk_start..chunk_end].to_vec());
        }
        offset = chunk_end.saturating_add(size % 2);
    }
    let format_chunk = format_chunk
        .filter(|value| value.len() >= 16)
        .ok_or_else(|| format!("WAV '{}' has no valid fmt chunk.", path.display()))?;
    let data = data.ok_or_else(|| format!("WAV '{}' has no data chunk.", path.display()))?;
    Ok(ParsedWav { format_chunk, data })
}

fn validate_fmt_chunk(chunk: &[u8], expected: &AudioFormatMetadata) -> Result<(), String> {
    if chunk.len() < 16 {
        return Err("WAV fmt chunk is truncated.".to_string());
    }
    let format_tag = u16::from_le_bytes(chunk[0..2].try_into().unwrap());
    let channels = u16::from_le_bytes(chunk[2..4].try_into().unwrap());
    let sample_rate = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
    let average_bytes = u32::from_le_bytes(chunk[8..12].try_into().unwrap());
    let block_align = u16::from_le_bytes(chunk[12..14].try_into().unwrap());
    let bits = u16::from_le_bytes(chunk[14..16].try_into().unwrap());
    if (
        format_tag,
        channels,
        sample_rate,
        average_bytes,
        block_align,
        bits,
    ) != (
        expected.format_tag,
        expected.channel_count,
        expected.sample_rate,
        expected.average_bytes_per_second,
        expected.block_align,
        expected.bits_per_sample,
    ) {
        return Err("WAV fmt chunk does not match finalized segment metadata.".to_string());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use crate::replay::{AudioTrackState, CompletedAudioSegment};

    use super::*;

    static TEST_ID: AtomicU64 = AtomicU64::new(0);

    fn test_directory(name: &str) -> PathBuf {
        let id = TEST_ID.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "stage11-audio-render-{name}-{}-{id}",
            std::process::id()
        ))
    }

    fn format() -> AudioFormatMetadata {
        AudioFormatMetadata {
            sample_format: "PCM integer".into(),
            format_tag: 1,
            sample_rate: 48_000,
            channel_count: 2,
            bits_per_sample: 16,
            valid_bits_per_sample: Some(16),
            block_align: 4,
            average_bytes_per_second: 192_000,
            channel_mask: None,
            sub_format: None,
        }
    }

    fn write_segment(
        directory: &Path,
        sequence: u64,
        start_qpc: i64,
        frames: u64,
    ) -> CompletedAudioSegment {
        fs::create_dir_all(directory).unwrap();
        let path = directory.join(format!("segment-{sequence}.wav"));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .unwrap();
        let fmt = [1, 0, 2, 0, 0x80, 0xbb, 0, 0, 0, 0xee, 2, 0, 4, 0, 16, 0];
        let mut writer = WavWriter::create(file, &fmt, 4, false).unwrap();
        writer
            .write_packet(&vec![sequence as u8; frames as usize * 4])
            .unwrap();
        writer.finalize().unwrap();
        let end_qpc =
            start_qpc + i64::try_from((u128::from(frames) * 10_000_000) / 48_000).unwrap();
        CompletedAudioSegment {
            track_role: AudioTrackRole::Game,
            source_identifier: "42".into(),
            process_id: Some(42),
            endpoint_id: None,
            sequence_number: sequence,
            file_path: path.to_string_lossy().into_owned(),
            format: format(),
            start_qpc_100ns: start_qpc,
            end_qpc_100ns: end_qpc,
            start_session_100ns: start_qpc,
            end_session_100ns: end_qpc,
            first_device_position: None,
            last_device_position: None,
            captured_sample_frames: frames,
            written_sample_frames: frames,
            actual_duration_ms: frames as f64 * 1_000.0 / 48_000.0,
            packet_count: 1,
            silent_packet_count: 0,
            discontinuity_count: 0,
            timestamp_error_count: 0,
            dropped_packet_count: 0,
            dropped_frame_count: 0,
            finalized: true,
            file_size: fs::metadata(path).unwrap().len(),
        }
    }

    fn snapshot(segments: Vec<CompletedAudioSegment>) -> AudioSnapshotTrack {
        AudioSnapshotTrack {
            track_role: AudioTrackRole::Game,
            source_state: AudioTrackState::Running,
            source_error_message: None,
            format: Some(format()),
            segments,
        }
    }

    #[test]
    fn exact_front_trim_and_final_frame_count() {
        let directory = test_directory("front-trim");
        let segment = write_segment(&directory, 1, -10_000_000, 96_000);
        let timeline = SavedReplayTimeline::test_interval(0, 10_000_000);
        let rendered = render_track(&snapshot(vec![segment]), &timeline, &directory).unwrap();
        assert_eq!(rendered.diagnostics.frames_trimmed_before, 48_000);
        assert_eq!(rendered.diagnostics.rendered_frame_count, 48_000);
        assert_eq!(rendered.diagnostics.leading_silence_frames, 0);
        assert_eq!(rendered.diagnostics.trailing_silence_frames, 0);
        let parsed = parse_wav(&rendered.path).unwrap();
        assert_eq!(parsed.data.len(), 48_000 * 4);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn late_start_and_early_end_become_silence() {
        let directory = test_directory("silence");
        let segment = write_segment(&directory, 1, 2_500_000, 24_000);
        let timeline = SavedReplayTimeline::test_interval(0, 10_000_000);
        let rendered = render_track(&snapshot(vec![segment]), &timeline, &directory).unwrap();
        assert_eq!(rendered.diagnostics.leading_silence_frames, 12_000);
        assert_eq!(rendered.diagnostics.trailing_silence_frames, 12_000);
        assert_eq!(rendered.diagnostics.rendered_frame_count, 48_000);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn multiple_wavs_are_concatenated_in_sequence() {
        let directory = test_directory("multiple");
        let first = write_segment(&directory, 1, 0, 24_000);
        let second = write_segment(&directory, 2, 5_000_000, 24_000);
        let timeline = SavedReplayTimeline::test_interval(0, 10_000_000);
        let rendered = render_track(&snapshot(vec![first, second]), &timeline, &directory).unwrap();
        let parsed = parse_wav(&rendered.path).unwrap();
        assert_eq!(&parsed.data[0..4], &[1, 1, 1, 1]);
        assert_eq!(&parsed.data[24_000 * 4..24_000 * 4 + 4], &[2, 2, 2, 2]);
        assert!(rendered.diagnostics.warnings.is_empty());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn ffmpeg_restart_gap_is_removed_from_native_audio_on_the_same_timeline() {
        let directory = test_directory("restart-gap");
        let audio = write_segment(&directory, 1, 0, 144_000);
        let mut timeline = SavedReplayTimeline::test_interval(0, 10_000_000);
        let mut second = timeline.segment_maps[0].clone();
        second.sequence_number = 2;
        second.session_start_qpc_100ns = 20_000_000;
        second.session_end_qpc_100ns = 30_000_000;
        second.source_start_qpc_100ns = 20_000_000;
        second.source_last_frame_qpc_100ns = 30_000_000;
        second.clip_start_100ns = 10_000_000;
        second.clip_end_100ns = 20_000_000;
        timeline.segment_maps.push(second);
        timeline.clip_capture_end_qpc_100ns = 30_000_000;
        timeline.clip_playback_end_100ns = 20_000_000;
        timeline.clip_playback_duration_100ns = 20_000_000;
        let rendered = render_track(&snapshot(vec![audio]), &timeline, &directory).unwrap();
        assert_eq!(rendered.diagnostics.rendered_frame_count, 96_000);
        assert_eq!(rendered.diagnostics.frames_trimmed_after, 48_000);
        assert_eq!(rendered.diagnostics.leading_silence_frames, 0);
        assert_eq!(rendered.diagnostics.trailing_silence_frames, 0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn healthy_thirty_seconds_is_exactly_1_440_000_frames() {
        assert_eq!(duration_to_frames(300_000_000, 48_000).unwrap(), 1_440_000);
    }

    #[test]
    fn wav_parser_finds_data_after_nonstandard_chunks() {
        let directory = test_directory("chunks");
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("chunks.wav");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&50u32.to_le_bytes());
        bytes.extend_from_slice(b"WAVE");
        bytes.extend_from_slice(b"JUNK");
        bytes.extend_from_slice(&2u32.to_le_bytes());
        bytes.extend_from_slice(&[7, 8]);
        bytes.extend_from_slice(b"fmt ");
        bytes.extend_from_slice(&16u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 0, 2, 0, 0x80, 0xbb, 0, 0, 0, 0xee, 2, 0, 4, 0, 16, 0]);
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&4u32.to_le_bytes());
        bytes.extend_from_slice(&[1, 2, 3, 4]);
        fs::write(&path, bytes).unwrap();
        let parsed = parse_wav(&path).unwrap();
        assert_eq!(parsed.data, [1, 2, 3, 4]);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn format_mismatch_is_rejected() {
        let directory = test_directory("format-mismatch");
        let first = write_segment(&directory, 1, 0, 24_000);
        let mut second = write_segment(&directory, 2, 5_000_000, 24_000);
        second.format.sample_rate = 44_100;
        let timeline = SavedReplayTimeline::test_interval(0, 10_000_000);
        assert!(render_track(&snapshot(vec![first, second]), &timeline, &directory).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
