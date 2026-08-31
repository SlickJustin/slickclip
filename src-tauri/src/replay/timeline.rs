use serde::Serialize;

use super::segment::{CompletedSegment, VideoFrameTimingPoint};

pub const MEDIA_TIME_BASE_DENOMINATOR: i64 = 10_000_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSegmentPlaybackMap {
    pub sequence_number: u64,
    pub session_start_qpc_100ns: i64,
    pub session_end_qpc_100ns: i64,
    pub source_start_qpc_100ns: i64,
    pub source_last_frame_qpc_100ns: i64,
    pub encoded_start_pts_100ns: i64,
    pub encoded_end_pts_100ns: i64,
    pub encoded_duration_100ns: i64,
    pub clip_start_100ns: i64,
    pub clip_end_100ns: i64,
    pub frame_timing_points: Vec<VideoFrameTimingPoint>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureMappingKind {
    BeforeClip,
    Segment,
    AfterClip,
}

#[derive(Clone, Copy, Debug)]
pub struct MappedCaptureTime {
    pub clip_time_100ns: i64,
    pub kind: CaptureMappingKind,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedReplayTimeline {
    pub raw_capture_start_qpc_100ns: i64,
    pub raw_capture_end_qpc_100ns: i64,
    pub raw_capture_span_100ns: i64,
    pub clip_capture_start_qpc_100ns: i64,
    pub clip_capture_end_qpc_100ns: i64,
    pub clip_playback_start_100ns: i64,
    pub clip_playback_end_100ns: i64,
    pub clip_playback_duration_100ns: i64,
    pub encoded_time_base_numerator: u32,
    pub encoded_time_base_denominator: u32,
    pub timestamp_strategy: String,
    pub segment_maps: Vec<VideoSegmentPlaybackMap>,
}

impl SavedReplayTimeline {
    #[cfg(test)]
    pub(crate) fn test_interval(start_qpc_100ns: i64, duration_100ns: i64) -> Self {
        Self {
            raw_capture_start_qpc_100ns: start_qpc_100ns,
            raw_capture_end_qpc_100ns: start_qpc_100ns.saturating_add(duration_100ns),
            raw_capture_span_100ns: duration_100ns,
            clip_capture_start_qpc_100ns: start_qpc_100ns,
            clip_capture_end_qpc_100ns: start_qpc_100ns.saturating_add(duration_100ns),
            clip_playback_start_100ns: 0,
            clip_playback_end_100ns: duration_100ns,
            clip_playback_duration_100ns: duration_100ns,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: MEDIA_TIME_BASE_DENOMINATOR as u32,
            timestamp_strategy: "test interval".to_string(),
            segment_maps: vec![VideoSegmentPlaybackMap {
                sequence_number: 1,
                session_start_qpc_100ns: start_qpc_100ns,
                session_end_qpc_100ns: start_qpc_100ns.saturating_add(duration_100ns),
                source_start_qpc_100ns: start_qpc_100ns,
                source_last_frame_qpc_100ns: start_qpc_100ns.saturating_add(duration_100ns),
                encoded_start_pts_100ns: 0,
                encoded_end_pts_100ns: duration_100ns,
                encoded_duration_100ns: duration_100ns,
                clip_start_100ns: 0,
                clip_end_100ns: duration_100ns,
                frame_timing_points: Vec::new(),
            }],
        }
    }

    pub fn from_segments(segments: &[CompletedSegment]) -> Result<Self, String> {
        let first = segments.first().ok_or_else(|| {
            "Cannot build a saved-replay timeline without video segments.".to_string()
        })?;
        let mut clip_cursor = 0i64;
        let mut segment_maps = Vec::with_capacity(segments.len());

        for segment in segments {
            let expected_cfr_duration = ((i128::from(segment.frame_count) * 10_000_000)
                / i128::from(segment.frame_rate.max(1)))
                as i64;
            if segment.encoded_start_pts_100ns != 0
                || segment.encoded_duration_100ns <= 0
                || segment.encoded_end_pts_100ns != segment.encoded_duration_100ns
                || segment.encoded_duration_100ns != expected_cfr_duration
                || segment
                    .segment_session_end_qpc_100ns
                    .saturating_sub(segment.segment_session_start_qpc_100ns)
                    .abs_diff(expected_cfr_duration)
                    > 1
            {
                return Err(format!(
                    "Video segment {} has inconsistent encoded timeline metadata.",
                    segment.sequence_number
                ));
            }
            if segment.frame_timing_points.len() != segment.frame_count as usize
                || segment
                    .frame_timing_points
                    .first()
                    .map(|point| point.output_qpc_100ns)
                    != Some(segment.segment_session_start_qpc_100ns)
                || segment
                    .frame_timing_points
                    .last()
                    .map(|point| point.encoded_pts_100ns)
                    != Some(segment.encoded_last_frame_pts_100ns)
                || segment.frame_timing_points.iter().any(|point| {
                    point.encoded_pts_100ns
                        != ((i128::from(point.frame_index) * 10_000_000)
                            / i128::from(segment.frame_rate.max(1)))
                            as i64
                        || point.output_qpc_100ns.abs_diff(
                            segment
                                .segment_session_start_qpc_100ns
                                .saturating_add(point.encoded_pts_100ns),
                        ) > 1
                })
                || segment
                    .fresh_output_frame_count
                    .saturating_add(segment.held_output_frame_count)
                    != segment.frame_count
            {
                return Err(format!(
                    "Video segment {} has inconsistent source-QPC/CFR-PTS anchors.",
                    segment.sequence_number
                ));
            }
            if let Some(previous) = segment_maps.last() {
                let previous: &VideoSegmentPlaybackMap = previous;
                if segment.segment_session_start_qpc_100ns < previous.session_end_qpc_100ns {
                    return Err(format!(
                        "Video segments {} and {} overlap on the monotonic Replay timeline.",
                        previous.sequence_number, segment.sequence_number
                    ));
                }
            }
            let clip_end = clip_cursor.saturating_add(segment.encoded_duration_100ns);
            segment_maps.push(VideoSegmentPlaybackMap {
                sequence_number: segment.sequence_number,
                session_start_qpc_100ns: segment.segment_session_start_qpc_100ns,
                session_end_qpc_100ns: segment.segment_session_end_qpc_100ns,
                source_start_qpc_100ns: segment.first_frame_timestamp_100ns,
                source_last_frame_qpc_100ns: segment.last_frame_timestamp_100ns,
                encoded_start_pts_100ns: segment.encoded_start_pts_100ns,
                encoded_end_pts_100ns: segment.encoded_end_pts_100ns,
                encoded_duration_100ns: segment.encoded_duration_100ns,
                clip_start_100ns: clip_cursor,
                clip_end_100ns: clip_end,
                frame_timing_points: segment.frame_timing_points.clone(),
            });
            clip_cursor = clip_end;
        }

        let raw_capture_start = first.first_frame_timestamp_100ns;
        let raw_capture_end = segment_maps
            .last()
            .map(|segment| segment.source_last_frame_qpc_100ns)
            .unwrap_or(raw_capture_start);
        Ok(Self {
            raw_capture_start_qpc_100ns: raw_capture_start,
            raw_capture_end_qpc_100ns: raw_capture_end,
            raw_capture_span_100ns: raw_capture_end.saturating_sub(raw_capture_start),
            clip_capture_start_qpc_100ns: first.segment_session_start_qpc_100ns,
            clip_capture_end_qpc_100ns: segment_maps
                .last()
                .map_or(first.segment_session_end_qpc_100ns, |segment| segment.session_end_qpc_100ns),
            clip_playback_start_100ns: 0,
            clip_playback_end_100ns: clip_cursor,
            clip_playback_duration_100ns: clip_cursor,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: MEDIA_TIME_BASE_DENOMINATOR as u32,
            timestamp_strategy: "One monotonic Replay QPC timeline. FFmpeg segment-local PTS restarts at zero and stream-copy concat abuts encoded durations. Each segment retains its source QPC interval; bounded child-restart gaps are removed from every native WASAPI stem with the same piecewise mapping.".to_string(),
            segment_maps,
        })
    }

    pub fn map_capture_qpc(&self, qpc_100ns: i64) -> MappedCaptureTime {
        if qpc_100ns < self.clip_capture_start_qpc_100ns {
            return MappedCaptureTime {
                clip_time_100ns: qpc_100ns.saturating_sub(self.clip_capture_start_qpc_100ns),
                kind: CaptureMappingKind::BeforeClip,
            };
        }
        if qpc_100ns >= self.clip_capture_end_qpc_100ns {
            return MappedCaptureTime {
                clip_time_100ns: self
                    .clip_playback_duration_100ns
                    .saturating_add(qpc_100ns.saturating_sub(self.clip_capture_end_qpc_100ns)),
                kind: CaptureMappingKind::AfterClip,
            };
        }
        if let Some(segment) = self.segment_maps.iter().find(|segment| {
            qpc_100ns >= segment.session_start_qpc_100ns
                && qpc_100ns < segment.session_end_qpc_100ns
        }) {
            return MappedCaptureTime {
                clip_time_100ns: segment
                    .clip_start_100ns
                    .saturating_add(qpc_100ns.saturating_sub(segment.session_start_qpc_100ns)),
                kind: CaptureMappingKind::Segment,
            };
        }
        let next = self
            .segment_maps
            .iter()
            .find(|segment| segment.session_start_qpc_100ns > qpc_100ns);
        MappedCaptureTime {
            clip_time_100ns: next.map_or(self.clip_playback_duration_100ns, |segment| {
                segment.clip_start_100ns
            }),
            kind: CaptureMappingKind::Segment,
        }
    }

    pub fn video_pts_to_clip_100ns(&self, sequence_number: u64, pts_100ns: i64) -> Option<i64> {
        self.segment_maps
            .iter()
            .find(|segment| segment.sequence_number == sequence_number)
            .map(|segment| segment.clip_start_100ns.saturating_add(pts_100ns))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(sequence: u64, source_start: i64, duration: i64) -> CompletedSegment {
        let frame_count = ((i128::from(duration) * 60 + 9_999_999) / 10_000_000) as u64;
        let frame_timing_points = (0..frame_count)
            .map(|frame_index| VideoFrameTimingPoint {
                frame_index,
                output_qpc_100ns: source_start
                    + ((i128::from(frame_index) * 10_000_000) / 60) as i64,
                source_qpc_100ns: source_start
                    + ((i128::from(frame_index) * i128::from(duration)) / i128::from(frame_count))
                        as i64,
                encoded_pts_100ns: ((i128::from(frame_index) * 10_000_000) / 60) as i64,
                fresh_source: true,
            })
            .collect::<Vec<_>>();
        let last = frame_timing_points.last().unwrap();
        CompletedSegment {
            sequence_number: sequence,
            file_path: format!("segment-{sequence}.mp4"),
            start_timestamp_ms: 0,
            end_timestamp_ms: 0,
            actual_duration_ms: (duration / 10_000) as u64,
            segment_session_start_qpc_100ns: source_start,
            segment_session_end_qpc_100ns: source_start.saturating_add(duration),
            first_frame_timestamp_100ns: source_start,
            last_frame_timestamp_100ns: last.source_qpc_100ns,
            encoded_start_pts_100ns: 0,
            encoded_last_frame_pts_100ns: last.encoded_pts_100ns,
            encoded_end_pts_100ns: duration,
            encoded_duration_100ns: duration,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: 10_000_000,
            frame_timing_points,
            next_segment_first_frame_timestamp_100ns: None,
            source_frame_gap_ms: None,
            source_update_count: frame_count,
            fresh_output_frame_count: frame_count,
            held_output_frame_count: 0,
            frame_count,
            encoder_creation_time_ms: 0.0,
            encoder_creation_started_ms: 0.0,
            encoder_creation_completed_ms: 0.0,
            rotation_requested_ms: None,
            first_frame_submitted_ms: None,
            last_frame_submitted_ms: None,
            next_first_frame_submitted_ms: None,
            codec: "HEVC".into(),
            width: 2560,
            height: 1440,
            frame_rate: 60,
            file_size: 1,
            average_bitrate_mbps: 0.000004,
            finalized: true,
            finalization_time_ms: 0.0,
            rotation_gap_ms: None,
        }
    }

    #[test]
    fn realtime_qpc_span_matches_encoded_playback_duration() {
        let timeline = SavedReplayTimeline::from_segments(&[
            segment(1, 1_000_000_000, 20_000_000),
            segment(2, 1_020_000_000, 20_000_000),
        ])
        .unwrap();
        assert_eq!(
            timeline.clip_capture_end_qpc_100ns - timeline.clip_capture_start_qpc_100ns,
            40_000_000
        );
        assert_eq!(timeline.clip_playback_duration_100ns, 40_000_000);
    }

    #[test]
    fn qpc_and_segment_pts_map_to_clip_relative_time() {
        let timeline = SavedReplayTimeline::from_segments(&[
            segment(4, 100_000_000, 20_000_000),
            segment(5, 120_000_000, 10_000_000),
        ])
        .unwrap();
        assert_eq!(
            timeline.map_capture_qpc(105_000_000).clip_time_100ns,
            5_000_000
        );
        assert_eq!(
            timeline.video_pts_to_clip_100ns(5, 3_000_000),
            Some(23_000_000)
        );
    }

    #[test]
    fn source_delivery_gap_does_not_delete_realtime_playback() {
        let mut second = segment(2, 20_000_000, 20_000_000);
        for point in &mut second.frame_timing_points {
            point.source_qpc_100ns = point.source_qpc_100ns.saturating_add(5_000_000);
        }
        second.first_frame_timestamp_100ns += 5_000_000;
        second.last_frame_timestamp_100ns += 5_000_000;
        let timeline =
            SavedReplayTimeline::from_segments(&[segment(1, 0, 20_000_000), second]).unwrap();
        let gap = timeline.map_capture_qpc(22_500_000);
        assert_eq!(gap.kind, CaptureMappingKind::Segment);
        assert_eq!(gap.clip_time_100ns, 22_500_000);
        assert_eq!(timeline.clip_playback_duration_100ns, 40_000_000);
    }

    #[test]
    fn ffmpeg_child_restart_gap_stays_monotonic_and_is_removed_from_playback() {
        let first = segment(1, 100_000_000, 20_000_000);
        let second = segment(2, 130_000_000, 20_000_000);
        let timeline = SavedReplayTimeline::from_segments(&[first, second]).unwrap();
        assert_eq!(timeline.clip_playback_duration_100ns, 40_000_000);
        assert_eq!(timeline.clip_capture_end_qpc_100ns, 150_000_000);
        assert_eq!(timeline.raw_capture_span_100ns, 49_833_333);
        let gap = timeline.map_capture_qpc(125_000_000);
        assert_eq!(gap.kind, CaptureMappingKind::Segment);
        assert_eq!(gap.clip_time_100ns, 20_000_000);
        assert_eq!(
            timeline.map_capture_qpc(135_000_000).clip_time_100ns,
            25_000_000
        );
    }

    #[test]
    fn five_hundred_ms_static_source_gap_preserves_thirty_cfr_positions() {
        let mut item = segment(1, 0, 5_000_000);
        for (index, point) in item.frame_timing_points.iter_mut().enumerate() {
            point.source_qpc_100ns = if index + 1 == 30 { 5_000_000 } else { 0 };
            point.fresh_source = index == 0 || index + 1 == 30;
        }
        item.last_frame_timestamp_100ns = 5_000_000;
        item.source_update_count = 2;
        item.fresh_output_frame_count = 2;
        item.held_output_frame_count = 28;
        let timeline = SavedReplayTimeline::from_segments(&[item]).unwrap();
        let halfway = timeline.map_capture_qpc(2_500_000);
        assert_eq!(halfway.kind, CaptureMappingKind::Segment);
        assert_eq!(halfway.clip_time_100ns, 2_500_000);
        assert_eq!(timeline.segment_maps[0].frame_timing_points.len(), 30);
        assert_eq!(timeline.clip_playback_duration_100ns, 5_000_000);
    }

    #[test]
    fn partial_first_and_last_segments_keep_their_exact_durations() {
        let timeline = SavedReplayTimeline::from_segments(&[
            segment(8, 0, 7_500_000),
            segment(9, 7_500_000, 20_000_000),
            segment(10, 27_500_000, 4_000_000),
        ])
        .unwrap();
        assert_eq!(timeline.segment_maps[0].clip_end_100ns, 7_500_000);
        assert_eq!(timeline.segment_maps[2].clip_start_100ns, 27_500_000);
        assert_eq!(timeline.clip_playback_duration_100ns, 31_500_000);
    }

    #[test]
    fn thirty_second_request_may_include_a_boundary_segment() {
        let segments = (0..16)
            .map(|index| segment(index + 1, index as i64 * 19_166_666, 19_166_666))
            .collect::<Vec<_>>();
        let timeline = SavedReplayTimeline::from_segments(&segments).unwrap();
        assert_eq!(timeline.clip_playback_duration_100ns, 306_666_656);
        assert!(timeline.clip_playback_duration_100ns > 300_000_000);
    }
}
