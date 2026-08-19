use serde::Serialize;
use std::collections::VecDeque;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoFrameTimingPoint {
    pub frame_index: u64,
    pub output_qpc_100ns: i64,
    pub source_qpc_100ns: i64,
    pub encoded_pts_100ns: i64,
    pub fresh_source: bool,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedSegment {
    pub sequence_number: u64,
    pub file_path: String,
    pub start_timestamp_ms: u64,
    pub end_timestamp_ms: u64,
    pub actual_duration_ms: u64,
    pub segment_session_start_qpc_100ns: i64,
    pub segment_session_end_qpc_100ns: i64,
    pub first_frame_timestamp_100ns: i64,
    pub last_frame_timestamp_100ns: i64,
    pub encoded_start_pts_100ns: i64,
    pub encoded_last_frame_pts_100ns: i64,
    pub encoded_end_pts_100ns: i64,
    pub encoded_duration_100ns: i64,
    pub encoded_time_base_numerator: u32,
    pub encoded_time_base_denominator: u32,
    pub frame_timing_points: Vec<VideoFrameTimingPoint>,
    pub next_segment_first_frame_timestamp_100ns: Option<i64>,
    pub source_frame_gap_ms: Option<f64>,
    pub source_update_count: u64,
    pub fresh_output_frame_count: u64,
    pub held_output_frame_count: u64,
    pub frame_count: u64,
    pub encoder_creation_time_ms: f64,
    pub encoder_creation_started_ms: f64,
    pub encoder_creation_completed_ms: f64,
    pub rotation_requested_ms: Option<f64>,
    pub first_frame_submitted_ms: Option<f64>,
    pub last_frame_submitted_ms: Option<f64>,
    pub next_first_frame_submitted_ms: Option<f64>,
    pub codec: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub file_size: u64,
    pub average_bitrate_mbps: f64,
    pub finalized: bool,
    pub finalization_time_ms: f64,
    pub rotation_gap_ms: Option<f64>,
}

pub fn average_bitrate_mbps(file_size: u64, duration_100ns: i64) -> Option<f64> {
    (duration_100ns > 0)
        .then(|| file_size as f64 * 8.0 * 10_000_000.0 / duration_100ns as f64 / 1_000_000.0)
}

pub struct SegmentRing {
    replay_window_100ns: i64,
    segments: VecDeque<CompletedSegment>,
}

impl SegmentRing {
    pub fn new(replay_window_seconds: u32) -> Self {
        Self {
            replay_window_100ns: i64::from(replay_window_seconds) * 10_000_000,
            segments: VecDeque::new(),
        }
    }

    pub fn push(&mut self, segment: CompletedSegment) -> Vec<CompletedSegment> {
        self.segments.push_back(segment);
        let mut evicted = Vec::new();

        while self.segments.len() > 1 {
            let Some(front) = self.segments.front() else {
                break;
            };
            let remaining_duration = self
                .total_duration_100ns()
                .saturating_sub(front.encoded_duration_100ns);
            if remaining_duration < self.replay_window_100ns {
                break;
            }

            if let Some(removed) = self.segments.pop_front() {
                evicted.push(removed);
            }
        }

        evicted
    }

    pub fn len(&self) -> usize {
        self.segments.len()
    }

    pub fn total_duration_ms(&self) -> u64 {
        u64::try_from(self.total_duration_100ns().max(0) / 10_000).unwrap_or(u64::MAX)
    }

    pub fn total_duration_100ns(&self) -> i64 {
        self.segments
            .iter()
            .map(|segment| segment.encoded_duration_100ns)
            .fold(0i64, i64::saturating_add)
    }

    pub fn total_bytes(&self) -> u64 {
        self.segments.iter().map(|segment| segment.file_size).sum()
    }

    pub fn recent(&self, limit: usize) -> Vec<CompletedSegment> {
        self.segments.iter().rev().take(limit).cloned().collect()
    }

    pub fn contains_sequence(&self, sequence_number: u64) -> bool {
        self.segments
            .iter()
            .any(|segment| segment.sequence_number == sequence_number)
    }

    pub fn select_suffix_through(
        &self,
        final_sequence_number: u64,
        requested_duration_ms: u64,
    ) -> Vec<CompletedSegment> {
        let mut selected = Vec::new();
        let mut duration_100ns = 0_i64;
        let requested_duration_100ns = i64::try_from(requested_duration_ms)
            .unwrap_or(i64::MAX)
            .saturating_mul(10_000);

        for segment in self
            .segments
            .iter()
            .rev()
            .filter(|segment| segment.sequence_number <= final_sequence_number)
        {
            selected.push(segment.clone());
            duration_100ns = duration_100ns.saturating_add(segment.encoded_duration_100ns);
            if duration_100ns >= requested_duration_100ns {
                break;
            }
        }

        selected.reverse();
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::{average_bitrate_mbps, CompletedSegment, SegmentRing, VideoFrameTimingPoint};

    fn segment(sequence_number: u64, duration_ms: u64) -> CompletedSegment {
        CompletedSegment {
            sequence_number,
            file_path: format!("segment-{sequence_number:06}.mp4"),
            start_timestamp_ms: sequence_number * duration_ms,
            end_timestamp_ms: (sequence_number + 1) * duration_ms,
            actual_duration_ms: duration_ms,
            segment_session_start_qpc_100ns: 0,
            segment_session_end_qpc_100ns: i64::try_from(duration_ms * 10_000).unwrap(),
            first_frame_timestamp_100ns: 0,
            last_frame_timestamp_100ns: i64::try_from(duration_ms * 10_000).unwrap(),
            encoded_start_pts_100ns: 0,
            encoded_last_frame_pts_100ns: i64::try_from(duration_ms * 10_000).unwrap(),
            encoded_end_pts_100ns: i64::try_from(duration_ms * 10_000).unwrap(),
            encoded_duration_100ns: i64::try_from(duration_ms * 10_000).unwrap(),
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: 10_000_000,
            frame_timing_points: vec![VideoFrameTimingPoint {
                frame_index: 0,
                output_qpc_100ns: 0,
                source_qpc_100ns: 0,
                encoded_pts_100ns: 0,
                fresh_source: true,
            }],
            next_segment_first_frame_timestamp_100ns: None,
            source_frame_gap_ms: None,
            source_update_count: 1,
            fresh_output_frame_count: 1,
            held_output_frame_count: (duration_ms / 16).saturating_sub(1),
            frame_count: duration_ms / 16,
            encoder_creation_time_ms: 10.0,
            encoder_creation_started_ms: 0.0,
            encoder_creation_completed_ms: 10.0,
            rotation_requested_ms: None,
            first_frame_submitted_ms: Some(0.0),
            last_frame_submitted_ms: Some(duration_ms as f64),
            next_first_frame_submitted_ms: None,
            codec: "H.264".to_string(),
            width: 1920,
            height: 1080,
            frame_rate: 60,
            file_size: 1_000,
            average_bitrate_mbps: average_bitrate_mbps(
                1_000,
                i64::try_from(duration_ms * 10_000).unwrap(),
            )
            .unwrap(),
            finalized: true,
            finalization_time_ms: 10.0,
            rotation_gap_ms: Some(2.0),
        }
    }

    #[test]
    fn retention_keeps_only_the_requested_suffix_plus_one_boundary_segment() {
        let mut ring = SegmentRing::new(5);
        let mut evicted = Vec::new();
        for sequence in 1..=4 {
            evicted.extend(ring.push(segment(sequence, 2_000)));
        }

        assert_eq!(ring.len(), 3);
        assert_eq!(ring.total_duration_ms(), 6_000);
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].file_path, "segment-000001.mp4");
    }

    #[test]
    fn retention_does_not_evict_before_the_window_is_filled() {
        let mut ring = SegmentRing::new(30);
        for sequence in 1..=8 {
            assert!(ring.push(segment(sequence, 2_000)).is_empty());
        }

        assert_eq!(ring.len(), 8);
        assert_eq!(ring.total_duration_ms(), 16_000);
    }

    #[test]
    fn selection_is_chronological_and_includes_the_boundary_segment() {
        let mut ring = SegmentRing::new(30);
        for sequence in 1..=8 {
            ring.push(segment(sequence, 2_000));
        }

        let selected = ring.select_suffix_through(7, 5_000);
        let sequences = selected
            .iter()
            .map(|segment| segment.sequence_number)
            .collect::<Vec<_>>();
        assert_eq!(sequences, vec![5, 6, 7]);
    }

    #[test]
    fn selection_uses_all_available_segments_for_a_partial_buffer() {
        let mut ring = SegmentRing::new(120);
        for sequence in 1..=4 {
            ring.push(segment(sequence, 2_000));
        }

        let selected = ring.select_suffix_through(4, 120_000);
        assert_eq!(selected.len(), 4);
        assert_eq!(
            selected
                .iter()
                .map(|segment| segment.actual_duration_ms)
                .sum::<u64>(),
            8_000
        );
    }

    #[test]
    fn bitrate_uses_exact_file_bytes_and_encoded_duration() {
        assert_eq!(average_bitrate_mbps(15_000_000, 80_000_000), Some(15.0));
        assert_eq!(average_bitrate_mbps(1, 0), None);
    }
}
