use serde::Serialize;
use std::collections::VecDeque;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedSegment {
    pub sequence_number: u64,
    pub file_path: String,
    pub start_timestamp_ms: u64,
    pub end_timestamp_ms: u64,
    pub actual_duration_ms: u64,
    pub first_frame_timestamp_100ns: i64,
    pub last_frame_timestamp_100ns: i64,
    pub next_segment_first_frame_timestamp_100ns: Option<i64>,
    pub source_frame_gap_ms: Option<f64>,
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
    pub finalized: bool,
    pub finalization_time_ms: f64,
    pub rotation_gap_ms: Option<f64>,
}

pub struct SegmentRing {
    replay_window_ms: u64,
    segments: VecDeque<CompletedSegment>,
}

impl SegmentRing {
    pub fn new(replay_window_seconds: u32) -> Self {
        Self {
            replay_window_ms: u64::from(replay_window_seconds) * 1_000,
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
                .total_duration_ms()
                .saturating_sub(front.actual_duration_ms);
            if remaining_duration < self.replay_window_ms {
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
        self.segments
            .iter()
            .map(|segment| segment.actual_duration_ms)
            .sum()
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
        let mut duration_ms = 0_u64;

        for segment in self
            .segments
            .iter()
            .rev()
            .filter(|segment| segment.sequence_number <= final_sequence_number)
        {
            selected.push(segment.clone());
            duration_ms = duration_ms.saturating_add(segment.actual_duration_ms);
            if duration_ms >= requested_duration_ms {
                break;
            }
        }

        selected.reverse();
        selected
    }
}

#[cfg(test)]
mod tests {
    use super::{CompletedSegment, SegmentRing};

    fn segment(sequence_number: u64, duration_ms: u64) -> CompletedSegment {
        CompletedSegment {
            sequence_number,
            file_path: format!("segment-{sequence_number:06}.mp4"),
            start_timestamp_ms: sequence_number * duration_ms,
            end_timestamp_ms: (sequence_number + 1) * duration_ms,
            actual_duration_ms: duration_ms,
            first_frame_timestamp_100ns: 0,
            last_frame_timestamp_100ns: i64::try_from(duration_ms * 10_000).unwrap(),
            next_segment_first_frame_timestamp_100ns: None,
            source_frame_gap_ms: None,
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
}
