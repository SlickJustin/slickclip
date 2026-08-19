use serde::Serialize;

use super::segment::{CompletedSegment, VideoFrameTimingPoint};

pub const MEDIA_TIME_BASE_DENOMINATOR: i64 = 10_000_000;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoSegmentPlaybackMap {
    pub sequence_number: u64,
    pub source_start_qpc_100ns: i64,
    pub source_last_frame_qpc_100ns: i64,
    pub source_playback_end_qpc_100ns: i64,
    pub encoded_start_pts_100ns: i64,
    pub encoded_end_pts_100ns: i64,
    pub encoded_duration_100ns: i64,
    pub clip_start_100ns: i64,
    pub clip_end_100ns: i64,
    pub frame_timing_points: Vec<VideoFrameTimingPoint>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscardedCaptureGap {
    pub previous_sequence_number: u64,
    pub next_sequence_number: u64,
    pub source_start_qpc_100ns: i64,
    pub source_end_qpc_100ns: i64,
    pub duration_100ns: i64,
    pub collapsed_at_clip_100ns: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureMappingKind {
    BeforeClip,
    Segment,
    DiscardedBoundaryGap,
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
    pub clip_playback_start_100ns: i64,
    pub clip_playback_end_100ns: i64,
    pub clip_playback_duration_100ns: i64,
    pub encoded_time_base_numerator: u32,
    pub encoded_time_base_denominator: u32,
    pub timestamp_strategy: String,
    pub discarded_capture_gap_count: usize,
    pub discarded_capture_gap_duration_100ns: i64,
    pub segment_maps: Vec<VideoSegmentPlaybackMap>,
    pub discarded_capture_gaps: Vec<DiscardedCaptureGap>,
}

impl SavedReplayTimeline {
    pub fn from_segments(segments: &[CompletedSegment]) -> Result<Self, String> {
        let first = segments.first().ok_or_else(|| {
            "Cannot build a saved-replay timeline without video segments.".to_string()
        })?;
        let mut clip_cursor = 0i64;
        let mut segment_maps = Vec::with_capacity(segments.len());
        let mut discarded_capture_gaps = Vec::new();

        for segment in segments {
            let expected_cfr_duration = ((i128::from(segment.frame_count) * 10_000_000)
                / i128::from(segment.frame_rate.max(1)))
                as i64;
            if segment.encoded_start_pts_100ns != 0
                || segment.encoded_duration_100ns <= 0
                || segment.encoded_end_pts_100ns != segment.encoded_duration_100ns
                || segment.encoded_duration_100ns != expected_cfr_duration
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
                    .map(|point| point.source_qpc_100ns)
                    != Some(segment.first_frame_timestamp_100ns)
                || segment
                    .frame_timing_points
                    .last()
                    .map(|point| point.source_qpc_100ns)
                    != Some(segment.last_frame_timestamp_100ns)
                || segment.frame_timing_points.iter().any(|point| {
                    point.encoded_pts_100ns
                        != ((i128::from(point.frame_index) * 10_000_000)
                            / i128::from(segment.frame_rate.max(1)))
                            as i64
                })
            {
                return Err(format!(
                    "Video segment {} has inconsistent source-QPC/CFR-PTS anchors.",
                    segment.sequence_number
                ));
            }
            let terminal_frame_duration = segment
                .frame_timing_points
                .last()
                .map(|point| {
                    segment
                        .encoded_duration_100ns
                        .saturating_sub(point.encoded_pts_100ns)
                })
                .unwrap_or_else(|| 10_000_000 / i64::from(segment.frame_rate.max(1)));
            let source_playback_end = segment
                .last_frame_timestamp_100ns
                .saturating_add(terminal_frame_duration);
            if let Some(previous) = segment_maps.last() {
                let previous: &VideoSegmentPlaybackMap = previous;
                if segment.first_frame_timestamp_100ns > previous.source_playback_end_qpc_100ns {
                    let duration = segment
                        .first_frame_timestamp_100ns
                        .saturating_sub(previous.source_playback_end_qpc_100ns);
                    discarded_capture_gaps.push(DiscardedCaptureGap {
                        previous_sequence_number: previous.sequence_number,
                        next_sequence_number: segment.sequence_number,
                        source_start_qpc_100ns: previous.source_playback_end_qpc_100ns,
                        source_end_qpc_100ns: segment.first_frame_timestamp_100ns,
                        duration_100ns: duration,
                        collapsed_at_clip_100ns: clip_cursor,
                    });
                }
            }
            let clip_end = clip_cursor.saturating_add(segment.encoded_duration_100ns);
            segment_maps.push(VideoSegmentPlaybackMap {
                sequence_number: segment.sequence_number,
                source_start_qpc_100ns: segment.first_frame_timestamp_100ns,
                source_last_frame_qpc_100ns: segment.last_frame_timestamp_100ns,
                source_playback_end_qpc_100ns: source_playback_end,
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
            .map(|segment| segment.source_playback_end_qpc_100ns)
            .unwrap_or(raw_capture_start);
        let discarded_duration = discarded_capture_gaps
            .iter()
            .map(|gap| gap.duration_100ns)
            .sum();
        Ok(Self {
            raw_capture_start_qpc_100ns: raw_capture_start,
            raw_capture_end_qpc_100ns: raw_capture_end,
            raw_capture_span_100ns: raw_capture_end.saturating_sub(raw_capture_start),
            clip_playback_start_100ns: 0,
            clip_playback_end_100ns: clip_cursor,
            clip_playback_duration_100ns: clip_cursor,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: MEDIA_TIME_BASE_DENOMINATOR as u32,
            timestamp_strategy: "Media Transcoder emits configured-rate CFR output. Each accepted frame stores its WGC source QPC and CFR PTS (frame_index / frame_rate); FFmpeg concat abuts independently zero-based segments at cumulative encoded duration".to_string(),
            discarded_capture_gap_count: discarded_capture_gaps.len(),
            discarded_capture_gap_duration_100ns: discarded_duration,
            segment_maps,
            discarded_capture_gaps,
        })
    }

    pub fn map_capture_qpc(&self, qpc_100ns: i64) -> MappedCaptureTime {
        let Some(first) = self.segment_maps.first() else {
            return MappedCaptureTime {
                clip_time_100ns: 0,
                kind: CaptureMappingKind::BeforeClip,
            };
        };
        if qpc_100ns < first.source_start_qpc_100ns {
            return MappedCaptureTime {
                clip_time_100ns: qpc_100ns.saturating_sub(first.source_start_qpc_100ns),
                kind: CaptureMappingKind::BeforeClip,
            };
        }
        for (index, segment) in self.segment_maps.iter().enumerate() {
            if qpc_100ns <= segment.source_playback_end_qpc_100ns {
                return MappedCaptureTime {
                    clip_time_100ns: segment
                        .clip_start_100ns
                        .saturating_add(map_within_segment(segment, qpc_100ns)),
                    kind: CaptureMappingKind::Segment,
                };
            }
            if let Some(next) = self.segment_maps.get(index + 1) {
                if qpc_100ns < next.source_start_qpc_100ns {
                    return MappedCaptureTime {
                        clip_time_100ns: segment.clip_end_100ns,
                        kind: CaptureMappingKind::DiscardedBoundaryGap,
                    };
                }
            }
        }
        let last = self.segment_maps.last().expect("nonempty timeline");
        MappedCaptureTime {
            clip_time_100ns: last
                .clip_end_100ns
                .saturating_add(qpc_100ns.saturating_sub(last.source_playback_end_qpc_100ns)),
            kind: CaptureMappingKind::AfterClip,
        }
    }

    pub fn video_pts_to_clip_100ns(&self, sequence_number: u64, pts_100ns: i64) -> Option<i64> {
        self.segment_maps
            .iter()
            .find(|segment| segment.sequence_number == sequence_number)
            .map(|segment| segment.clip_start_100ns.saturating_add(pts_100ns))
    }
}

fn map_within_segment(segment: &VideoSegmentPlaybackMap, qpc_100ns: i64) -> i64 {
    let Some(first) = segment.frame_timing_points.first() else {
        return 0;
    };
    if qpc_100ns <= first.source_qpc_100ns {
        return first.encoded_pts_100ns;
    }
    for points in segment.frame_timing_points.windows(2) {
        let left = &points[0];
        let right = &points[1];
        if qpc_100ns <= right.source_qpc_100ns {
            return interpolate(
                qpc_100ns,
                left.source_qpc_100ns,
                right.source_qpc_100ns,
                left.encoded_pts_100ns,
                right.encoded_pts_100ns,
            );
        }
    }
    let last = segment
        .frame_timing_points
        .last()
        .expect("nonempty anchors");
    interpolate(
        qpc_100ns.min(segment.source_playback_end_qpc_100ns),
        last.source_qpc_100ns,
        segment.source_playback_end_qpc_100ns,
        last.encoded_pts_100ns,
        segment.encoded_end_pts_100ns,
    )
}

fn interpolate(
    value: i64,
    source_start: i64,
    source_end: i64,
    target_start: i64,
    target_end: i64,
) -> i64 {
    let source_span = source_end.saturating_sub(source_start);
    if source_span <= 0 {
        return target_end;
    }
    let numerator = i128::from(value.saturating_sub(source_start))
        * i128::from(target_end.saturating_sub(target_start));
    target_start.saturating_add((numerator / i128::from(source_span)) as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn segment(sequence: u64, source_start: i64, duration: i64) -> CompletedSegment {
        let frame_count = ((i128::from(duration) * 60 + 9_999_999) / 10_000_000) as u64;
        let frame_timing_points = (0..frame_count)
            .map(|frame_index| VideoFrameTimingPoint {
                frame_index,
                source_qpc_100ns: source_start
                    + ((i128::from(frame_index) * i128::from(duration)) / i128::from(frame_count))
                        as i64,
                encoded_pts_100ns: ((i128::from(frame_index) * 10_000_000) / 60) as i64,
            })
            .collect::<Vec<_>>();
        let last = frame_timing_points.last().unwrap();
        CompletedSegment {
            sequence_number: sequence,
            file_path: format!("segment-{sequence}.mp4"),
            start_timestamp_ms: 0,
            end_timestamp_ms: 0,
            actual_duration_ms: (duration / 10_000) as u64,
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
            finalized: true,
            finalization_time_ms: 0.0,
            rotation_gap_ms: None,
        }
    }

    #[test]
    fn raw_qpc_span_can_exceed_encoded_playback_duration() {
        let timeline = SavedReplayTimeline::from_segments(&[
            segment(1, 1_000_000_000, 20_000_000),
            segment(2, 1_025_000_000, 20_000_000),
        ])
        .unwrap();
        assert_eq!(timeline.raw_capture_span_100ns, 45_000_000);
        assert_eq!(timeline.clip_playback_duration_100ns, 40_000_000);
        assert_eq!(timeline.discarded_capture_gap_duration_100ns, 5_000_000);
    }

    #[test]
    fn qpc_and_segment_pts_map_to_clip_relative_time() {
        let timeline = SavedReplayTimeline::from_segments(&[
            segment(4, 100_000_000, 20_000_000),
            segment(5, 125_000_000, 10_000_000),
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
    fn five_hundred_ms_boundary_gap_collapses_without_extending_playback() {
        let timeline = SavedReplayTimeline::from_segments(&[
            segment(1, 0, 20_000_000),
            segment(2, 25_000_000, 20_000_000),
        ])
        .unwrap();
        let gap = timeline.map_capture_qpc(22_500_000);
        assert_eq!(gap.kind, CaptureMappingKind::DiscardedBoundaryGap);
        assert_eq!(gap.clip_time_100ns, 20_000_000);
        assert_eq!(timeline.clip_playback_duration_100ns, 40_000_000);
    }

    #[test]
    fn five_hundred_ms_intra_segment_source_gap_maps_to_one_cfr_interval() {
        let mut item = segment(1, 0, 10_000_000);
        item.frame_count = 2;
        item.encoded_duration_100ns = 333_333;
        item.encoded_end_pts_100ns = 333_333;
        item.encoded_last_frame_pts_100ns = 166_666;
        item.frame_timing_points = vec![
            VideoFrameTimingPoint {
                frame_index: 0,
                source_qpc_100ns: 0,
                encoded_pts_100ns: 0,
            },
            VideoFrameTimingPoint {
                frame_index: 1,
                source_qpc_100ns: 5_000_000,
                encoded_pts_100ns: 166_666,
            },
        ];
        item.last_frame_timestamp_100ns = 5_000_000;
        let timeline = SavedReplayTimeline::from_segments(&[item]).unwrap();
        let halfway = timeline.map_capture_qpc(2_500_000);
        assert_eq!(halfway.kind, CaptureMappingKind::Segment);
        assert!((halfway.clip_time_100ns - 83_333).abs() <= 1);
        assert_eq!(timeline.clip_playback_duration_100ns, 333_333);
    }

    #[test]
    fn partial_first_and_last_segments_keep_their_exact_durations() {
        let timeline = SavedReplayTimeline::from_segments(&[
            segment(8, 0, 7_500_000),
            segment(9, 8_000_000, 20_000_000),
            segment(10, 30_000_000, 4_000_000),
        ])
        .unwrap();
        assert_eq!(timeline.segment_maps[0].clip_end_100ns, 7_500_000);
        assert_eq!(timeline.segment_maps[2].clip_start_100ns, 27_500_000);
        assert_eq!(timeline.clip_playback_duration_100ns, 31_500_000);
    }

    #[test]
    fn thirty_second_request_may_include_a_boundary_segment() {
        let segments = (0..16)
            .map(|index| segment(index + 1, index as i64 * 21_000_000, 19_166_666))
            .collect::<Vec<_>>();
        let timeline = SavedReplayTimeline::from_segments(&segments).unwrap();
        assert_eq!(timeline.clip_playback_duration_100ns, 306_666_656);
        assert!(timeline.clip_playback_duration_100ns > 300_000_000);
    }
}
