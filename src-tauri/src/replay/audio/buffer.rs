use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use serde::Serialize;

use super::clock::{ReplayClockStatus, ReplaySessionClock};
use super::segment::{AudioSegmentRing, CompletedAudioSegment};
use super::{AudioReplayConfiguration, AudioTrackConfiguration, AudioTrackRole, AudioTrackState};
use crate::audio::AudioFormatMetadata;
use crate::replay::timeline::{CaptureMappingKind, SavedReplayTimeline};

pub const AUDIO_PACKET_QUEUE_CAPACITY: usize = 64;

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrackStatus {
    pub role: AudioTrackRole,
    pub enabled: bool,
    pub state: AudioTrackState,
    pub source_identifier: Option<String>,
    pub source_label: Option<String>,
    pub process_id: Option<u32>,
    pub endpoint_id: Option<String>,
    pub format: Option<AudioFormatMetadata>,
    pub error_message: Option<String>,
    pub track_start_offset_ms: Option<f64>,
    pub latest_audio_position_ms: Option<f64>,
    pub first_device_position: Option<u64>,
    pub latest_device_position: Option<u64>,
    pub retained_duration_seconds: f64,
    pub segment_count: usize,
    pub total_retained_bytes: u64,
    pub packet_count: u64,
    pub silent_packet_count: u64,
    pub discontinuity_count: u64,
    pub timestamp_error_count: u64,
    pub current_queue_depth: usize,
    pub maximum_queue_depth: usize,
    pub queue_capacity: usize,
    pub queue_full_events: u64,
    pub dropped_packets: u64,
    pub dropped_sample_frames: u64,
    pub captured_sample_frames: u64,
    pub written_sample_frames: u64,
    pub expected_duration_from_samples_seconds: f64,
    pub qpc_elapsed_duration_seconds: Option<f64>,
    pub sample_qpc_difference_ms: Option<f64>,
    pub estimated_clock_drift_ppm: Option<f64>,
    pub writer_write_time_ms: f64,
    pub writer_finalize_time_ms: f64,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioReplayStatus {
    pub clock: ReplayClockStatus,
    pub tracks: Vec<AudioTrackStatus>,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSnapshotPlan {
    pub track_role: AudioTrackRole,
    pub raw_video_start_qpc_100ns: i64,
    pub raw_video_end_qpc_100ns: i64,
    pub raw_video_span_ms: f64,
    pub clip_capture_start_qpc_100ns: i64,
    pub clip_capture_end_qpc_100ns: i64,
    pub clip_playback_start_ms: f64,
    pub clip_playback_end_ms: f64,
    pub clip_playback_duration_ms: f64,
    pub raw_audio_start_qpc_100ns: Option<i64>,
    pub raw_audio_end_qpc_100ns: Option<i64>,
    pub mapped_playback_start_ms: Option<f64>,
    pub mapped_playback_end_ms: Option<f64>,
    pub mapped_start_region: Option<String>,
    pub mapped_end_region: Option<String>,
    pub leading_uncovered_ms: f64,
    pub trailing_uncovered_ms: f64,
    pub trim_before_clip_ms: f64,
    pub trim_after_clip_ms: f64,
    pub final_clip_coverage_ms: f64,
    pub material_uncovered_threshold_ms: f64,
    pub has_material_uncovered_audio: bool,
    pub warning: Option<String>,
    pub segment_count: usize,
    pub segment_sequence_numbers: Vec<u64>,
}

struct AudioCoverage {
    leading_uncovered: i64,
    trailing_uncovered: i64,
    trim_before: i64,
    trim_after: i64,
    coverage: i64,
    material: bool,
}

const MATERIAL_UNCOVERED_100NS: i64 = 500_000;

fn calculate_audio_coverage(
    mapped_start: Option<i64>,
    mapped_end: Option<i64>,
    clip_duration: i64,
) -> AudioCoverage {
    let leading_uncovered = mapped_start.unwrap_or(clip_duration).max(0);
    let trailing_uncovered = clip_duration.saturating_sub(mapped_end.unwrap_or(0)).max(0);
    let trim_before = mapped_start.unwrap_or(0).saturating_neg().max(0);
    let trim_after = mapped_end.unwrap_or(0).saturating_sub(clip_duration).max(0);
    let coverage = clip_duration
        .saturating_sub(leading_uncovered)
        .saturating_sub(trailing_uncovered)
        .max(0);
    AudioCoverage {
        leading_uncovered,
        trailing_uncovered,
        trim_before,
        trim_after,
        coverage,
        material: leading_uncovered > MATERIAL_UNCOVERED_100NS
            || trailing_uncovered > MATERIAL_UNCOVERED_100NS,
    }
}

pub struct TrackShared {
    pub configuration: AudioTrackConfiguration,
    pub directory: PathBuf,
    inner: Mutex<TrackInner>,
}

pub struct TrackInner {
    pub status: AudioTrackStatus,
    pub ring: AudioSegmentRing,
    pub first_packet_qpc_100ns: Option<i64>,
}

impl TrackShared {
    pub fn lock(&self) -> MutexGuard<'_, TrackInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn set_prepared(&self, label: String, format: AudioFormatMetadata) {
        let mut inner = self.lock();
        inner.status.source_label = Some(label);
        inner.status.format = Some(format);
        inner.status.state = AudioTrackState::Prepared;
    }

    pub fn set_running(&self, offset_ms: f64) {
        let mut inner = self.lock();
        inner.status.state = AudioTrackState::Running;
        inner.status.track_start_offset_ms = Some(offset_ms);
    }

    pub fn set_terminal(&self, state: AudioTrackState, message: Option<String>) {
        let mut inner = self.lock();
        inner.status.state = state;
        inner.status.error_message = message;
        inner.status.current_queue_depth = 0;
    }

    pub fn complete_segment(&self, segment: CompletedAudioSegment) -> Result<(), String> {
        if !segment.is_consistent() {
            return Err(format!(
                "Audio segment #{} for {:?} failed metadata validation.",
                segment.sequence_number, segment.track_role
            ));
        }
        let mut inner = self.lock();
        inner.status.written_sample_frames = inner
            .status
            .written_sample_frames
            .saturating_add(segment.written_sample_frames);
        inner.ring.push(segment, &self.directory)?;
        update_retention(&mut inner);
        Ok(())
    }

    pub fn status(&self) -> AudioTrackStatus {
        let mut inner = self.lock();
        update_retention(&mut inner);
        inner.status.clone()
    }
}

fn update_retention(inner: &mut TrackInner) {
    inner.status.retained_duration_seconds = inner.ring.retained_duration_seconds();
    inner.status.segment_count = inner.ring.len();
    inner.status.total_retained_bytes = inner.ring.total_bytes();
}

pub struct AudioReplayShared {
    clock: Mutex<Option<ReplaySessionClock>>,
    tracks: Mutex<BTreeMap<AudioTrackRole, Arc<TrackShared>>>,
}

impl AudioReplayShared {
    pub fn new() -> Self {
        Self {
            clock: Mutex::new(None),
            tracks: Mutex::new(BTreeMap::new()),
        }
    }

    pub fn begin(
        &self,
        configuration: &AudioReplayConfiguration,
        clock: ReplaySessionClock,
        session_directory: PathBuf,
        replay_duration_seconds: u32,
    ) -> Result<(), String> {
        let mut tracks = BTreeMap::new();
        for config in &configuration.tracks {
            if tracks.contains_key(&config.role) {
                return Err(format!(
                    "Only one {:?} audio track may be configured per Replay session.",
                    config.role
                ));
            }
            let directory = session_directory
                .join("audio")
                .join(config.role.directory_name());
            std::fs::create_dir_all(&directory).map_err(|error| {
                format!(
                    "Could not create audio track directory '{}': {error}",
                    directory.display()
                )
            })?;
            let source_identifier = config.source_identifier();
            let status = AudioTrackStatus {
                role: config.role,
                enabled: config.enabled,
                state: if config.enabled {
                    AudioTrackState::Preparing
                } else {
                    AudioTrackState::Disabled
                },
                source_identifier,
                source_label: config.source_label.clone(),
                process_id: config.process_id,
                endpoint_id: config.endpoint_id.clone(),
                queue_capacity: AUDIO_PACKET_QUEUE_CAPACITY,
                ..Default::default()
            };
            tracks.insert(
                config.role,
                Arc::new(TrackShared {
                    configuration: config.clone(),
                    directory,
                    inner: Mutex::new(TrackInner {
                        status,
                        ring: AudioSegmentRing::new(replay_duration_seconds),
                        first_packet_qpc_100ns: None,
                    }),
                }),
            );
        }
        *self.clock.lock().unwrap_or_else(|p| p.into_inner()) = Some(clock);
        *self.tracks.lock().unwrap_or_else(|p| p.into_inner()) = tracks;
        Ok(())
    }

    pub fn reset(&self) {
        *self.clock.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.tracks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    pub fn enabled_tracks(&self) -> Vec<Arc<TrackShared>> {
        self.tracks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .filter(|track| track.configuration.enabled)
            .cloned()
            .collect()
    }

    pub fn snapshot(&self) -> AudioReplayStatus {
        let clock = self
            .clock
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .as_ref()
            .map(ReplaySessionClock::status)
            .unwrap_or_default();
        let tracks = self
            .tracks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|track| track.status())
            .collect();
        AudioReplayStatus { clock, tracks }
    }

    pub fn plan_and_pin(
        self: &Arc<Self>,
        timeline: &SavedReplayTimeline,
    ) -> (Vec<AudioSnapshotPlan>, AudioSnapshotPinGuard) {
        let tracks = self
            .tracks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .cloned()
            .collect::<Vec<_>>();
        let mut releases = Vec::new();
        let mut plans = Vec::new();
        for track in tracks
            .into_iter()
            .filter(|track| track.configuration.enabled)
        {
            let selected = track.lock().ring.select_and_pin(
                timeline.clip_capture_start_qpc_100ns,
                timeline.clip_capture_end_qpc_100ns,
            );
            let sequences = selected
                .iter()
                .map(|segment| segment.sequence_number)
                .collect::<Vec<_>>();
            let selected_start = selected.first().map(|segment| segment.start_qpc_100ns);
            let selected_end = selected.last().map(|segment| segment.end_qpc_100ns);
            let start_mapping = selected_start.map(|value| timeline.map_capture_qpc(value));
            let end_mapping = selected_end.map(|value| timeline.map_capture_qpc(value));
            let mapped_start = start_mapping.map(|value| value.clip_time_100ns);
            let mapped_end = end_mapping.map(|value| value.clip_time_100ns);
            let clip_duration = timeline.clip_playback_duration_100ns;
            let coverage = calculate_audio_coverage(mapped_start, mapped_end, clip_duration);
            let warning = coverage.material.then(|| format!(
                "{:?} audio does not cover the final playback window: {:.3} ms leading and {:.3} ms trailing uncovered.",
                track.configuration.role,
                coverage.leading_uncovered as f64 / 10_000.0,
                coverage.trailing_uncovered as f64 / 10_000.0,
            ));
            plans.push(AudioSnapshotPlan {
                track_role: track.configuration.role,
                raw_video_start_qpc_100ns: timeline.raw_capture_start_qpc_100ns,
                raw_video_end_qpc_100ns: timeline.raw_capture_end_qpc_100ns,
                raw_video_span_ms: timeline.raw_capture_span_100ns as f64 / 10_000.0,
                clip_capture_start_qpc_100ns: timeline.clip_capture_start_qpc_100ns,
                clip_capture_end_qpc_100ns: timeline.clip_capture_end_qpc_100ns,
                clip_playback_start_ms: 0.0,
                clip_playback_end_ms: clip_duration as f64 / 10_000.0,
                clip_playback_duration_ms: clip_duration as f64 / 10_000.0,
                raw_audio_start_qpc_100ns: selected_start,
                raw_audio_end_qpc_100ns: selected_end,
                mapped_playback_start_ms: mapped_start.map(|value| value as f64 / 10_000.0),
                mapped_playback_end_ms: mapped_end.map(|value| value as f64 / 10_000.0),
                mapped_start_region: start_mapping
                    .map(|value| mapping_kind_name(value.kind).to_string()),
                mapped_end_region: end_mapping
                    .map(|value| mapping_kind_name(value.kind).to_string()),
                leading_uncovered_ms: coverage.leading_uncovered as f64 / 10_000.0,
                trailing_uncovered_ms: coverage.trailing_uncovered as f64 / 10_000.0,
                trim_before_clip_ms: coverage.trim_before as f64 / 10_000.0,
                trim_after_clip_ms: coverage.trim_after as f64 / 10_000.0,
                final_clip_coverage_ms: coverage.coverage as f64 / 10_000.0,
                material_uncovered_threshold_ms: MATERIAL_UNCOVERED_100NS as f64 / 10_000.0,
                has_material_uncovered_audio: coverage.material,
                warning,
                segment_count: selected.len(),
                segment_sequence_numbers: sequences.clone(),
            });
            releases.push((track, sequences));
        }
        (plans, AudioSnapshotPinGuard { releases })
    }
}

fn mapping_kind_name(kind: CaptureMappingKind) -> &'static str {
    match kind {
        CaptureMappingKind::BeforeClip => "beforeClip",
        CaptureMappingKind::Segment => "encodedSegment",
        CaptureMappingKind::AfterClip => "afterClip",
    }
}

pub struct AudioSnapshotPinGuard {
    releases: Vec<(Arc<TrackShared>, Vec<u64>)>,
}

impl Drop for AudioSnapshotPinGuard {
    fn drop(&mut self) {
        for (track, sequences) in &self.releases {
            track.lock().ring.release(sequences, &track.directory);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::segment::CompletedAudioSegment;
    use super::super::{AudioSourceKind, AudioTrackConfiguration};
    use super::*;
    use crate::audio::AudioFormatMetadata;
    use crate::replay::timeline::{SavedReplayTimeline, VideoSegmentPlaybackMap};

    #[test]
    fn independent_roles_create_independent_rings() {
        let root = std::env::temp_dir().join(format!("replay-audio-rings-{}", std::process::id()));
        let clock = ReplaySessionClock::create().unwrap();
        let shared = AudioReplayShared::new();
        shared
            .begin(
                &AudioReplayConfiguration {
                    tracks: vec![
                        AudioTrackConfiguration {
                            role: AudioTrackRole::Game,
                            enabled: true,
                            source_kind: AudioSourceKind::Process,
                            process_id: Some(1),
                            endpoint_id: None,
                            source_label: None,
                        },
                        AudioTrackConfiguration {
                            role: AudioTrackRole::Microphone,
                            enabled: true,
                            source_kind: AudioSourceKind::Microphone,
                            process_id: None,
                            endpoint_id: Some("mic".into()),
                            source_label: None,
                        },
                    ],
                },
                clock,
                root.clone(),
                30,
            )
            .unwrap();
        assert_eq!(shared.enabled_tracks().len(), 2);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn source_ended_state_is_track_local() {
        let root = std::env::temp_dir().join(format!("replay-audio-ended-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let shared = AudioReplayShared::new();
        shared
            .begin(
                &AudioReplayConfiguration {
                    tracks: vec![
                        AudioTrackConfiguration {
                            role: AudioTrackRole::Game,
                            enabled: true,
                            source_kind: AudioSourceKind::Process,
                            process_id: Some(1),
                            endpoint_id: None,
                            source_label: None,
                        },
                        AudioTrackConfiguration {
                            role: AudioTrackRole::Microphone,
                            enabled: true,
                            source_kind: AudioSourceKind::Microphone,
                            process_id: None,
                            endpoint_id: Some("mic".into()),
                            source_label: None,
                        },
                    ],
                },
                ReplaySessionClock::create().unwrap(),
                root.clone(),
                30,
            )
            .unwrap();
        let tracks = shared.enabled_tracks();
        tracks
            .iter()
            .find(|track| track.configuration.role == AudioTrackRole::Game)
            .unwrap()
            .set_terminal(AudioTrackState::Ended, Some("process exited".into()));
        let status = shared.snapshot();
        assert_eq!(
            status
                .tracks
                .iter()
                .find(|track| track.role == AudioTrackRole::Game)
                .unwrap()
                .state,
            AudioTrackState::Ended
        );
        assert_eq!(
            status
                .tracks
                .iter()
                .find(|track| track.role == AudioTrackRole::Microphone)
                .unwrap()
                .state,
            AudioTrackState::Preparing
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn snapshot_planning_uses_the_common_qpc_window() {
        let root = std::env::temp_dir().join(format!("replay-audio-plan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let shared = Arc::new(AudioReplayShared::new());
        shared
            .begin(
                &AudioReplayConfiguration {
                    tracks: vec![AudioTrackConfiguration {
                        role: AudioTrackRole::Game,
                        enabled: true,
                        source_kind: AudioSourceKind::Process,
                        process_id: Some(7),
                        endpoint_id: None,
                        source_label: None,
                    }],
                },
                ReplaySessionClock::create().unwrap(),
                root.clone(),
                30,
            )
            .unwrap();
        let track = shared.enabled_tracks().remove(0);
        let path = track.directory.join("segment-000001.wav");
        std::fs::write(&path, [0u8; 45]).unwrap();
        let format = AudioFormatMetadata {
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
        };
        track
            .complete_segment(CompletedAudioSegment {
                track_role: AudioTrackRole::Game,
                source_identifier: "7".into(),
                process_id: Some(7),
                endpoint_id: None,
                sequence_number: 1,
                file_path: path.to_string_lossy().into_owned(),
                format,
                start_qpc_100ns: 10_000_000,
                end_qpc_100ns: 30_000_000,
                start_session_100ns: 10_000_000,
                end_session_100ns: 30_000_000,
                first_device_position: Some(48_000),
                last_device_position: Some(144_000),
                captured_sample_frames: 96_000,
                written_sample_frames: 96_000,
                actual_duration_ms: 2_000.0,
                packet_count: 1,
                silent_packet_count: 0,
                discontinuity_count: 0,
                timestamp_error_count: 0,
                dropped_packet_count: 0,
                dropped_frame_count: 0,
                finalized: true,
                file_size: 45,
            })
            .unwrap();
        let timeline = SavedReplayTimeline {
            raw_capture_start_qpc_100ns: 15_000_000,
            raw_capture_end_qpc_100ns: 25_000_000,
            raw_capture_span_100ns: 10_000_000,
            clip_capture_start_qpc_100ns: 15_000_000,
            clip_capture_end_qpc_100ns: 25_000_000,
            clip_playback_start_100ns: 0,
            clip_playback_end_100ns: 10_000_000,
            clip_playback_duration_100ns: 10_000_000,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: 10_000_000,
            timestamp_strategy: "test".into(),
            segment_maps: vec![VideoSegmentPlaybackMap {
                sequence_number: 1,
                session_start_qpc_100ns: 15_000_000,
                session_end_qpc_100ns: 25_000_000,
                source_start_qpc_100ns: 15_000_000,
                source_last_frame_qpc_100ns: 24_833_334,
                encoded_start_pts_100ns: 0,
                encoded_end_pts_100ns: 10_000_000,
                encoded_duration_100ns: 10_000_000,
                clip_start_100ns: 0,
                clip_end_100ns: 10_000_000,
                frame_timing_points: vec![crate::replay::segment::VideoFrameTimingPoint {
                    frame_index: 0,
                    output_qpc_100ns: 15_000_000,
                    source_qpc_100ns: 15_000_000,
                    encoded_pts_100ns: 0,
                    fresh_source: true,
                }],
            }],
        };
        let (plans, pins) = shared.plan_and_pin(&timeline);
        assert_eq!(plans[0].segment_count, 1);
        assert_eq!(plans[0].final_clip_coverage_ms, 1_000.0);
        assert_eq!(plans[0].mapped_playback_start_ms, Some(-500.0));
        assert_eq!(plans[0].mapped_playback_end_ms, Some(1_500.0));
        assert_eq!(plans[0].trim_before_clip_ms, 500.0);
        assert_eq!(plans[0].trim_after_clip_ms, 500.0);
        drop(pins);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn audio_before_and_after_clip_is_reported_as_trim_material() {
        let coverage = calculate_audio_coverage(Some(-2_000_000), Some(302_000_000), 300_000_000);
        assert_eq!(coverage.trim_before, 2_000_000);
        assert_eq!(coverage.trim_after, 2_000_000);
        assert_eq!(coverage.leading_uncovered, 0);
        assert_eq!(coverage.trailing_uncovered, 0);
        assert!(!coverage.material);
    }

    #[test]
    fn genuinely_missing_audio_reports_exact_leading_and_trailing_coverage() {
        let coverage = calculate_audio_coverage(Some(1_000_000), Some(297_500_000), 300_000_000);
        assert_eq!(coverage.leading_uncovered, 1_000_000);
        assert_eq!(coverage.trailing_uncovered, 2_500_000);
        assert_eq!(coverage.coverage, 296_500_000);
        assert!(coverage.material);
    }
}
