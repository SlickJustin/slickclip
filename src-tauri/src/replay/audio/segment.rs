use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::audio::AudioFormatMetadata;

use super::AudioTrackRole;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedAudioSegment {
    pub track_role: AudioTrackRole,
    pub source_identifier: String,
    pub process_id: Option<u32>,
    pub endpoint_id: Option<String>,
    pub sequence_number: u64,
    pub file_path: String,
    pub format: AudioFormatMetadata,
    pub start_qpc_100ns: i64,
    pub end_qpc_100ns: i64,
    pub start_session_100ns: i64,
    pub end_session_100ns: i64,
    pub first_device_position: Option<u64>,
    pub last_device_position: Option<u64>,
    pub captured_sample_frames: u64,
    pub written_sample_frames: u64,
    pub actual_duration_ms: f64,
    pub packet_count: u64,
    pub silent_packet_count: u64,
    pub discontinuity_count: u64,
    pub timestamp_error_count: u64,
    pub dropped_packet_count: u64,
    pub dropped_frame_count: u64,
    pub finalized: bool,
    pub file_size: u64,
}

impl CompletedAudioSegment {
    pub fn is_consistent(&self) -> bool {
        self.finalized
            && self.written_sample_frames > 0
            && self.captured_sample_frames >= self.written_sample_frames
            && self.format.block_align > 0
            && self.file_size > 44
            && self.end_qpc_100ns >= self.start_qpc_100ns
            && self.end_session_100ns >= self.start_session_100ns
            && (self.actual_duration_ms
                - self
                    .format
                    .duration_ms_for_frames(self.written_sample_frames))
            .abs()
                < 0.02
    }

    pub fn overlaps(&self, start_qpc_100ns: i64, end_qpc_100ns: i64) -> bool {
        self.end_qpc_100ns > start_qpc_100ns && self.start_qpc_100ns < end_qpc_100ns
    }
}

#[derive(Default)]
pub struct AudioSegmentRing {
    replay_window_100ns: i64,
    segments: VecDeque<CompletedAudioSegment>,
    pins: HashMap<u64, usize>,
    deferred: HashMap<u64, PathBuf>,
}

impl AudioSegmentRing {
    pub fn new(replay_duration_seconds: u32) -> Self {
        Self {
            replay_window_100ns: i64::from(replay_duration_seconds) * 10_000_000,
            ..Default::default()
        }
    }

    pub fn push(
        &mut self,
        segment: CompletedAudioSegment,
        track_directory: &Path,
    ) -> Result<(), String> {
        self.segments.push_back(segment);
        let newest_end = self
            .segments
            .back()
            .map(|value| value.end_qpc_100ns)
            .unwrap_or(0);
        let cutoff = newest_end.saturating_sub(self.replay_window_100ns);
        // Keep the single segment that crosses the retention boundary.
        while self.segments.len() > 1
            && self
                .segments
                .front()
                .is_some_and(|front| front.end_qpc_100ns <= cutoff)
        {
            let expired = self.segments.pop_front().expect("front exists");
            let path = PathBuf::from(&expired.file_path);
            if self.pins.contains_key(&expired.sequence_number) {
                self.deferred.insert(expired.sequence_number, path);
            } else {
                remove_audio_file(&path, track_directory)?;
            }
        }
        Ok(())
    }

    pub fn select_and_pin(&mut self, start: i64, end: i64) -> Vec<CompletedAudioSegment> {
        let selected = self
            .segments
            .iter()
            .filter(|segment| segment.overlaps(start, end))
            .cloned()
            .collect::<Vec<_>>();
        for segment in &selected {
            *self.pins.entry(segment.sequence_number).or_insert(0) += 1;
        }
        selected
    }

    pub fn newest_end_qpc_100ns(&self) -> Option<i64> {
        self.segments.back().map(|segment| segment.end_qpc_100ns)
    }

    pub fn sequence_covering_end(&self, required_end_qpc_100ns: i64) -> Option<u64> {
        self.segments
            .iter()
            .find(|segment| segment.end_qpc_100ns >= required_end_qpc_100ns)
            .map(|segment| segment.sequence_number)
    }

    pub fn release(&mut self, sequences: &[u64], track_directory: &Path) {
        for sequence in sequences {
            let remove = match self.pins.get_mut(sequence) {
                Some(count) if *count > 1 => {
                    *count -= 1;
                    false
                }
                Some(_) => true,
                None => false,
            };
            if remove {
                self.pins.remove(sequence);
                if let Some(path) = self.deferred.remove(sequence) {
                    let _ = remove_audio_file(&path, track_directory);
                }
            }
        }
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }
    pub fn total_bytes(&self) -> u64 {
        self.segments.iter().map(|s| s.file_size).sum()
    }
    pub fn retained_duration_seconds(&self) -> f64 {
        match (self.segments.front(), self.segments.back()) {
            (Some(first), Some(last)) => {
                (last.end_qpc_100ns - first.start_qpc_100ns).max(0) as f64 / 10_000_000.0
            }
            _ => 0.0,
        }
    }
}

fn remove_audio_file(path: &Path, track_directory: &Path) -> Result<(), String> {
    if path.parent() != Some(track_directory) {
        return Err(format!(
            "Audio retention refused to delete a file outside '{}': '{}'",
            track_directory.display(),
            path.display()
        ));
    }
    fs::remove_file(path).map_err(|error| {
        format!(
            "Could not evict audio segment '{}': {error}",
            path.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn format() -> AudioFormatMetadata {
        AudioFormatMetadata {
            sample_format: "Float".into(),
            format_tag: 3,
            sample_rate: 48_000,
            channel_count: 2,
            bits_per_sample: 32,
            valid_bits_per_sample: Some(32),
            block_align: 8,
            average_bytes_per_second: 384_000,
            channel_mask: None,
            sub_format: None,
        }
    }
    fn segment(sequence: u64, start_seconds: i64) -> CompletedAudioSegment {
        CompletedAudioSegment {
            track_role: AudioTrackRole::Game,
            source_identifier: "42".into(),
            process_id: Some(42),
            endpoint_id: None,
            sequence_number: sequence,
            file_path: format!("segment-{sequence}.wav"),
            format: format(),
            start_qpc_100ns: start_seconds * 10_000_000,
            end_qpc_100ns: (start_seconds + 2) * 10_000_000,
            start_session_100ns: start_seconds * 10_000_000,
            end_session_100ns: (start_seconds + 2) * 10_000_000,
            first_device_position: Some(start_seconds as u64 * 48_000),
            last_device_position: Some((start_seconds as u64 + 2) * 48_000),
            captured_sample_frames: 96_000,
            written_sample_frames: 96_000,
            actual_duration_ms: 2_000.0,
            packet_count: 10,
            silent_packet_count: 0,
            discontinuity_count: 0,
            timestamp_error_count: 0,
            dropped_packet_count: 0,
            dropped_frame_count: 0,
            finalized: true,
            file_size: 768_044,
        }
    }

    #[test]
    fn metadata_consistency_and_overlap_are_exact() {
        let item = segment(1, 10);
        assert!(item.is_consistent());
        assert!(item.overlaps(11 * 10_000_000, 13 * 10_000_000));
        assert!(!item.overlaps(12 * 10_000_000, 14 * 10_000_000));
    }

    #[test]
    fn window_selection_handles_partial_boundaries() {
        let mut ring = AudioSegmentRing::new(30);
        ring.segments
            .extend([segment(1, 0), segment(2, 2), segment(3, 4)]);
        let selected = ring.select_and_pin(15_000_000, 45_000_000);
        assert_eq!(
            selected
                .iter()
                .map(|s| s.sequence_number)
                .collect::<Vec<_>>(),
            [1, 2, 3]
        );
    }

    #[test]
    fn retention_evicts_files_but_keeps_one_boundary_segment() {
        let directory =
            std::env::temp_dir().join(format!("audio-retention-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let mut ring = AudioSegmentRing::new(5);
        for sequence in 1..=4 {
            let path = directory.join(format!("segment-{sequence}.wav"));
            fs::write(&path, [0u8; 45]).unwrap();
            let mut item = segment(sequence, (sequence as i64 - 1) * 2);
            item.file_path = path.to_string_lossy().into_owned();
            item.file_size = 45;
            ring.push(item, &directory).unwrap();
        }
        assert_eq!(ring.len(), 3);
        assert!(!directory.join("segment-1.wav").exists());
        assert!(directory.join("segment-2.wav").exists());
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn pinned_eviction_is_deferred_until_snapshot_release() {
        let directory = std::env::temp_dir().join(format!("audio-pins-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let mut ring = AudioSegmentRing::new(2);
        let first_path = directory.join("segment-1.wav");
        fs::write(&first_path, [0u8; 45]).unwrap();
        let mut first = segment(1, 0);
        first.file_path = first_path.to_string_lossy().into_owned();
        first.file_size = 45;
        ring.push(first, &directory).unwrap();
        let selected = ring.select_and_pin(0, 20_000_000);
        for sequence in 2..=3 {
            let path = directory.join(format!("segment-{sequence}.wav"));
            fs::write(&path, [0u8; 45]).unwrap();
            let mut item = segment(sequence, (sequence as i64 - 1) * 2);
            item.file_path = path.to_string_lossy().into_owned();
            item.file_size = 45;
            ring.push(item, &directory).unwrap();
        }
        assert!(first_path.exists());
        ring.release(
            &selected
                .iter()
                .map(|item| item.sequence_number)
                .collect::<Vec<_>>(),
            &directory,
        );
        assert!(!first_path.exists());
        let _ = fs::remove_dir_all(directory);
    }
}
