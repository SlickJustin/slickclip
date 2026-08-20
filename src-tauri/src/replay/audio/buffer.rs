use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant};

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
    pub newest_captured_audio_qpc_100ns: Option<i64>,
    pub newest_written_audio_qpc_100ns: Option<i64>,
    pub newest_finalized_audio_qpc_100ns: Option<i64>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioReplayStatus {
    pub clock: ReplayClockStatus,
    pub tracks: Vec<AudioTrackStatus>,
    pub save_barriers: Vec<AudioSaveBarrierTelemetry>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSaveBarrierTelemetry {
    pub track_role: AudioTrackRole,
    pub source_state: AudioTrackState,
    pub source_error_message: Option<String>,
    pub required_video_end_qpc_100ns: i64,
    pub captured_through_qpc_100ns: Option<i64>,
    pub written_through_qpc_100ns: Option<i64>,
    pub finalized_through_qpc_100ns_before_wait: Option<i64>,
    pub finalized_through_qpc_100ns_after_wait: Option<i64>,
    pub wait_duration_ms: f64,
    pub explicit_writer_cut_requested: bool,
    pub satisfying_segment_sequence: Option<u64>,
    pub timed_out: bool,
    pub error_message: Option<String>,
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

#[derive(Clone, Debug)]
pub struct AudioSnapshotTrack {
    pub track_role: AudioTrackRole,
    pub source_state: AudioTrackState,
    pub source_error_message: Option<String>,
    pub format: Option<AudioFormatMetadata>,
    pub segments: Vec<CompletedAudioSegment>,
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
    changed: Condvar,
}

#[derive(Clone, Copy)]
struct AudioSaveCutRequest {
    barrier_id: u64,
    required_end_qpc_100ns: i64,
    requested_at: Instant,
}

#[derive(Clone, Copy)]
struct AudioSaveCutSatisfaction {
    barrier_id: u64,
    required_end_qpc_100ns: i64,
    segment_sequence: u64,
    wait_duration: Duration,
}

pub struct TrackInner {
    pub status: AudioTrackStatus,
    pub ring: AudioSegmentRing,
    pub first_packet_qpc_100ns: Option<i64>,
    pending_save_cut: Option<AudioSaveCutRequest>,
    last_save_cut_satisfaction: Option<AudioSaveCutSatisfaction>,
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
        self.changed.notify_all();
    }

    pub fn set_running(&self, offset_ms: f64) {
        let mut inner = self.lock();
        inner.status.state = AudioTrackState::Running;
        inner.status.track_start_offset_ms = Some(offset_ms);
        self.changed.notify_all();
    }

    pub fn set_terminal(&self, state: AudioTrackState, message: Option<String>) {
        let mut inner = self.lock();
        inner.status.state = state;
        inner.status.error_message = message;
        inner.status.current_queue_depth = 0;
        inner.pending_save_cut = None;
        self.changed.notify_all();
    }

    pub fn record_written_through(&self, qpc_100ns: i64) {
        let mut inner = self.lock();
        inner.status.newest_written_audio_qpc_100ns = Some(
            inner
                .status
                .newest_written_audio_qpc_100ns
                .unwrap_or(i64::MIN)
                .max(qpc_100ns),
        );
    }

    fn request_save_cut(&self, barrier_id: u64, required_end_qpc_100ns: i64) -> bool {
        let mut inner = self.lock();
        if inner
            .ring
            .newest_end_qpc_100ns()
            .is_some_and(|end| end >= required_end_qpc_100ns)
            || inner.status.state != AudioTrackState::Running
        {
            return false;
        }
        inner.pending_save_cut = Some(AudioSaveCutRequest {
            barrier_id,
            required_end_qpc_100ns,
            requested_at: Instant::now(),
        });
        inner.last_save_cut_satisfaction = None;
        self.changed.notify_all();
        true
    }

    pub fn should_cut_after_packet(&self, written_through_qpc_100ns: i64) -> bool {
        self.lock()
            .pending_save_cut
            .is_some_and(|request| written_through_qpc_100ns >= request.required_end_qpc_100ns)
    }

    fn cancel_save_cut(&self, barrier_id: u64) {
        let mut inner = self.lock();
        if inner
            .pending_save_cut
            .is_some_and(|request| request.barrier_id == barrier_id)
        {
            inner.pending_save_cut = None;
        }
        self.changed.notify_all();
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
        let segment_end = segment.end_qpc_100ns;
        let segment_sequence = segment.sequence_number;
        inner.ring.push(segment, &self.directory)?;
        inner.status.newest_finalized_audio_qpc_100ns = Some(
            inner
                .status
                .newest_finalized_audio_qpc_100ns
                .unwrap_or(i64::MIN)
                .max(segment_end),
        );
        if let Some(request) = inner.pending_save_cut {
            if segment_end >= request.required_end_qpc_100ns {
                inner.pending_save_cut = None;
                inner.last_save_cut_satisfaction = Some(AudioSaveCutSatisfaction {
                    barrier_id: request.barrier_id,
                    required_end_qpc_100ns: request.required_end_qpc_100ns,
                    segment_sequence,
                    wait_duration: request.requested_at.elapsed(),
                });
            }
        }
        update_retention(&mut inner);
        self.changed.notify_all();
        Ok(())
    }

    pub fn status(&self) -> AudioTrackStatus {
        let mut inner = self.lock();
        update_retention(&mut inner);
        inner.status.clone()
    }

    fn wait_for_barrier(&self, barrier_id: u64, required_end_qpc_100ns: i64, deadline: Instant) {
        let mut inner = self.lock();
        loop {
            let covered = inner
                .ring
                .newest_end_qpc_100ns()
                .is_some_and(|end| end >= required_end_qpc_100ns);
            if covered || inner.status.state != AudioTrackState::Running {
                return;
            }
            if Instant::now() >= deadline {
                return;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            let (next, _) = self
                .changed
                .wait_timeout(inner, remaining)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            inner = next;
            if !inner
                .pending_save_cut
                .is_some_and(|request| request.barrier_id == barrier_id)
            {
                return;
            }
        }
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
    next_save_barrier_id: AtomicU64,
    last_save_barriers: Mutex<Vec<AudioSaveBarrierTelemetry>>,
}

pub struct AudioSnapshotBarrierResult {
    pub plans: Vec<AudioSnapshotPlan>,
    pub tracks: Vec<AudioSnapshotTrack>,
    pub pins: AudioSnapshotPinGuard,
    pub barriers: Vec<AudioSaveBarrierTelemetry>,
    pub wait_duration: Duration,
}

#[derive(Debug)]
pub struct AudioSaveBarrierFailure {
    pub message: String,
}

struct PendingTrackSnapshot {
    track: Arc<TrackShared>,
    preliminary_segments: Vec<CompletedAudioSegment>,
    preliminary_sequences: Vec<u64>,
    finalized_before: Option<i64>,
    cut_requested: bool,
    wait_started: Instant,
}

impl AudioReplayShared {
    pub fn new() -> Self {
        Self {
            clock: Mutex::new(None),
            tracks: Mutex::new(BTreeMap::new()),
            next_save_barrier_id: AtomicU64::new(0),
            last_save_barriers: Mutex::new(Vec::new()),
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
                        pending_save_cut: None,
                        last_save_cut_satisfaction: None,
                    }),
                    changed: Condvar::new(),
                }),
            );
        }
        *self.clock.lock().unwrap_or_else(|p| p.into_inner()) = Some(clock);
        *self.tracks.lock().unwrap_or_else(|p| p.into_inner()) = tracks;
        self.last_save_barriers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        Ok(())
    }

    pub fn reset(&self) {
        *self.clock.lock().unwrap_or_else(|p| p.into_inner()) = None;
        self.tracks
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
        self.last_save_barriers
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
        let save_barriers = self
            .last_save_barriers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone();
        AudioReplayStatus {
            clock,
            tracks,
            save_barriers,
        }
    }

    pub fn wait_for_coverage_and_plan(
        self: &Arc<Self>,
        timeline: &SavedReplayTimeline,
        timeout: Duration,
    ) -> Result<AudioSnapshotBarrierResult, AudioSaveBarrierFailure> {
        let barrier_started = Instant::now();
        let barrier_id = self.next_save_barrier_id.fetch_add(1, Ordering::Relaxed) + 1;
        let required_start = timeline.clip_capture_start_qpc_100ns;
        let required_end = timeline.clip_capture_end_qpc_100ns;
        let tracks = self.enabled_tracks();
        let mut pending = Vec::new();

        // Pin every currently finalized overlapping segment before waiting. A
        // rolling eviction may remove its ring metadata, but the pinned file and
        // this immutable clone remain available to the pending Save.
        for track in tracks {
            let (preliminary_segments, preliminary_sequences, finalized_before) = {
                let mut inner = track.lock();
                let selected = inner.ring.select_and_pin(required_start, required_end);
                let sequences = selected
                    .iter()
                    .map(|segment| segment.sequence_number)
                    .collect::<Vec<_>>();
                (selected, sequences, inner.ring.newest_end_qpc_100ns())
            };
            let wait_started = Instant::now();
            let cut_requested = track.request_save_cut(barrier_id, required_end);
            pending.push(PendingTrackSnapshot {
                track,
                preliminary_segments,
                preliminary_sequences,
                finalized_before,
                cut_requested,
                wait_started,
            });
        }

        let deadline = Instant::now() + timeout;
        for item in &pending {
            if item.cut_requested {
                item.track
                    .wait_for_barrier(barrier_id, required_end, deadline);
            }
        }

        let mut barriers = pending
            .iter()
            .map(|item| barrier_telemetry(item, barrier_id, required_end))
            .collect::<Vec<_>>();
        let mut failures = Vec::new();
        for barrier in &mut barriers {
            let covered = barrier
                .finalized_through_qpc_100ns_after_wait
                .is_some_and(|end| end >= required_end);
            if barrier.source_state == AudioTrackState::Running && !covered {
                barrier.timed_out = true;
                barrier.error_message = Some(format!(
                    "{:?} audio did not finalize through required video QPC {} within {:.1} ms (captured {:?}, written {:?}, finalized {:?}).",
                    barrier.track_role,
                    required_end,
                    timeout.as_secs_f64() * 1_000.0,
                    barrier.captured_through_qpc_100ns,
                    barrier.written_through_qpc_100ns,
                    barrier.finalized_through_qpc_100ns_after_wait,
                ));
                failures.push(barrier.error_message.clone().unwrap_or_default());
            } else if matches!(
                barrier.source_state,
                AudioTrackState::Preparing | AudioTrackState::Prepared
            ) && !covered
            {
                barrier.error_message = Some(format!(
                    "{:?} audio was not running when Save required QPC {}.",
                    barrier.track_role, required_end
                ));
                failures.push(barrier.error_message.clone().unwrap_or_default());
            } else if !covered {
                barrier.error_message = barrier.source_error_message.clone().or_else(|| {
                    Some(format!(
                        "{:?} audio ended before required video QPC {}.",
                        barrier.track_role, required_end
                    ))
                });
            }
        }
        self.store_save_barriers(&barriers);

        if !failures.is_empty() {
            for item in &pending {
                item.track.cancel_save_cut(barrier_id);
                item.track
                    .lock()
                    .ring
                    .release(&item.preliminary_sequences, &item.track.directory);
            }
            return Err(AudioSaveBarrierFailure {
                message: failures.join(" "),
            });
        }

        let mut releases = Vec::new();
        let mut plans = Vec::new();
        let mut snapshot_tracks = Vec::new();
        for item in pending {
            let selected_after = item
                .track
                .lock()
                .ring
                .select_and_pin(required_start, required_end);
            let selected_after_sequences = selected_after
                .iter()
                .map(|segment| segment.sequence_number)
                .collect::<Vec<_>>();
            let selected = merge_audio_segments(item.preliminary_segments, selected_after);
            plans.push(build_snapshot_plan(&item.track, timeline, &selected));
            let status = item.track.status();
            snapshot_tracks.push(AudioSnapshotTrack {
                track_role: item.track.configuration.role,
                source_state: status.state,
                source_error_message: status.error_message,
                format: status.format,
                segments: selected,
            });
            releases.push((Arc::clone(&item.track), item.preliminary_sequences));
            releases.push((item.track, selected_after_sequences));
        }

        Ok(AudioSnapshotBarrierResult {
            plans,
            tracks: snapshot_tracks,
            pins: AudioSnapshotPinGuard { releases },
            barriers,
            wait_duration: barrier_started.elapsed(),
        })
    }

    fn store_save_barriers(&self, barriers: &[AudioSaveBarrierTelemetry]) {
        *self
            .last_save_barriers
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = barriers.to_vec();
    }

    pub fn clear_save_barriers(&self) {
        self.last_save_barriers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }

    #[cfg(test)]
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
            plans.push(build_snapshot_plan(&track, timeline, &selected));
            releases.push((track, sequences));
        }
        (plans, AudioSnapshotPinGuard { releases })
    }
}

fn barrier_telemetry(
    item: &PendingTrackSnapshot,
    barrier_id: u64,
    required_end_qpc_100ns: i64,
) -> AudioSaveBarrierTelemetry {
    let inner = item.track.lock();
    let satisfaction = inner.last_save_cut_satisfaction.filter(|value| {
        value.barrier_id == barrier_id && value.required_end_qpc_100ns == required_end_qpc_100ns
    });
    AudioSaveBarrierTelemetry {
        track_role: item.track.configuration.role,
        source_state: inner.status.state,
        source_error_message: inner.status.error_message.clone(),
        required_video_end_qpc_100ns: required_end_qpc_100ns,
        captured_through_qpc_100ns: inner.status.newest_captured_audio_qpc_100ns,
        written_through_qpc_100ns: inner.status.newest_written_audio_qpc_100ns,
        finalized_through_qpc_100ns_before_wait: item.finalized_before,
        finalized_through_qpc_100ns_after_wait: inner.ring.newest_end_qpc_100ns(),
        wait_duration_ms: satisfaction
            .map(|value| value.wait_duration)
            .unwrap_or_else(|| item.wait_started.elapsed())
            .as_secs_f64()
            * 1_000.0,
        explicit_writer_cut_requested: item.cut_requested,
        satisfying_segment_sequence: satisfaction
            .map(|value| value.segment_sequence)
            .or_else(|| inner.ring.sequence_covering_end(required_end_qpc_100ns)),
        timed_out: false,
        error_message: None,
    }
}

fn merge_audio_segments(
    before: Vec<CompletedAudioSegment>,
    after: Vec<CompletedAudioSegment>,
) -> Vec<CompletedAudioSegment> {
    let mut merged = BTreeMap::new();
    for segment in before.into_iter().chain(after) {
        merged.insert(segment.sequence_number, segment);
    }
    merged.into_values().collect()
}

fn build_snapshot_plan(
    track: &TrackShared,
    timeline: &SavedReplayTimeline,
    selected: &[CompletedAudioSegment],
) -> AudioSnapshotPlan {
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
    AudioSnapshotPlan {
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
        mapped_start_region: start_mapping.map(|value| mapping_kind_name(value.kind).to_string()),
        mapped_end_region: end_mapping.map(|value| mapping_kind_name(value.kind).to_string()),
        leading_uncovered_ms: coverage.leading_uncovered as f64 / 10_000.0,
        trailing_uncovered_ms: coverage.trailing_uncovered as f64 / 10_000.0,
        trim_before_clip_ms: coverage.trim_before as f64 / 10_000.0,
        trim_after_clip_ms: coverage.trim_after as f64 / 10_000.0,
        final_clip_coverage_ms: coverage.coverage as f64 / 10_000.0,
        material_uncovered_threshold_ms: MATERIAL_UNCOVERED_100NS as f64 / 10_000.0,
        has_material_uncovered_audio: coverage.material,
        warning,
        segment_count: selected.len(),
        segment_sequence_numbers: selected
            .iter()
            .map(|segment| segment.sequence_number)
            .collect(),
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
    use std::thread;

    use super::super::segment::CompletedAudioSegment;
    use super::super::{AudioSourceKind, AudioTrackConfiguration};
    use super::*;
    use crate::audio::AudioFormatMetadata;
    use crate::replay::timeline::{SavedReplayTimeline, VideoSegmentPlaybackMap};

    fn test_format() -> AudioFormatMetadata {
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

    fn test_timeline(start: i64, end: i64) -> SavedReplayTimeline {
        let duration = end - start;
        SavedReplayTimeline {
            raw_capture_start_qpc_100ns: start,
            raw_capture_end_qpc_100ns: end,
            raw_capture_span_100ns: duration,
            clip_capture_start_qpc_100ns: start,
            clip_capture_end_qpc_100ns: end,
            clip_playback_start_100ns: 0,
            clip_playback_end_100ns: duration,
            clip_playback_duration_100ns: duration,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: 10_000_000,
            timestamp_strategy: "test".into(),
            segment_maps: vec![VideoSegmentPlaybackMap {
                sequence_number: 1,
                session_start_qpc_100ns: start,
                session_end_qpc_100ns: end,
                source_start_qpc_100ns: start,
                source_last_frame_qpc_100ns: end - 166_666,
                encoded_start_pts_100ns: 0,
                encoded_end_pts_100ns: duration,
                encoded_duration_100ns: duration,
                clip_start_100ns: 0,
                clip_end_100ns: duration,
                frame_timing_points: Vec::new(),
            }],
        }
    }

    fn test_shared(
        root: &PathBuf,
        roles: &[AudioTrackRole],
        replay_seconds: u32,
    ) -> Arc<AudioReplayShared> {
        let shared = Arc::new(AudioReplayShared::new());
        shared
            .begin(
                &AudioReplayConfiguration {
                    tracks: roles
                        .iter()
                        .enumerate()
                        .map(|(index, role)| AudioTrackConfiguration {
                            role: *role,
                            enabled: true,
                            source_kind: AudioSourceKind::Process,
                            process_id: Some(index as u32 + 1),
                            endpoint_id: None,
                            source_label: Some(format!("{role:?}")),
                        })
                        .collect(),
                },
                ReplaySessionClock::create().unwrap(),
                root.clone(),
                replay_seconds,
            )
            .unwrap();
        for track in shared.enabled_tracks() {
            track.set_prepared(format!("{:?}", track.configuration.role), test_format());
            track.set_running(0.0);
        }
        shared
    }

    fn complete_test_segment(track: &TrackShared, sequence: u64, start: i64, end: i64) {
        let path = track.directory.join(format!("segment-{sequence:06}.wav"));
        std::fs::write(&path, [0u8; 45]).unwrap();
        let frames =
            u64::try_from((i128::from(end - start) * 48_000i128 / 10_000_000i128).max(1)).unwrap();
        track
            .complete_segment(CompletedAudioSegment {
                track_role: track.configuration.role,
                source_identifier: track.configuration.source_identifier().unwrap(),
                process_id: track.configuration.process_id,
                endpoint_id: None,
                sequence_number: sequence,
                file_path: path.to_string_lossy().into_owned(),
                format: test_format(),
                start_qpc_100ns: start,
                end_qpc_100ns: end,
                start_session_100ns: start,
                end_session_100ns: end,
                first_device_position: Some(0),
                last_device_position: Some(frames),
                captured_sample_frames: frames,
                written_sample_frames: frames,
                actual_duration_ms: frames as f64 / 48_000.0 * 1_000.0,
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
    }

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
    fn save_barrier_waits_for_active_writer_tail_and_pins_evicted_early_audio() {
        let root =
            std::env::temp_dir().join(format!("replay-audio-save-barrier-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let shared = test_shared(&root, &[AudioTrackRole::Game], 2);
        let track = shared.enabled_tracks().remove(0);
        complete_test_segment(&track, 1, 0, 20_000_000);
        let first_path = track.directory.join("segment-000001.wav");
        let writer_track = Arc::clone(&track);
        let simulated_writer = thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(1);
            while !writer_track.should_cut_after_packet(50_000_000) {
                assert!(Instant::now() < deadline, "writer cut was not requested");
                thread::sleep(Duration::from_millis(1));
            }
            writer_track.record_written_through(50_000_000);
            complete_test_segment(&writer_track, 2, 20_000_000, 50_000_000);
        });

        let result = shared
            .wait_for_coverage_and_plan(&test_timeline(0, 25_000_000), Duration::from_secs(1))
            .unwrap();
        simulated_writer.join().unwrap();

        assert_eq!(result.plans[0].trailing_uncovered_ms, 0.0);
        assert_eq!(result.plans[0].raw_audio_start_qpc_100ns, Some(0));
        assert_eq!(result.plans[0].raw_audio_end_qpc_100ns, Some(50_000_000));
        assert!(result.barriers[0].explicit_writer_cut_requested);
        assert_eq!(result.barriers[0].satisfying_segment_sequence, Some(2));
        assert!(first_path.exists());
        drop(result.pins);
        assert!(!first_path.exists());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn three_tracks_independently_satisfy_the_authoritative_video_endpoint() {
        let root = std::env::temp_dir().join(format!(
            "replay-audio-three-barriers-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let shared = test_shared(
            &root,
            &[
                AudioTrackRole::Game,
                AudioTrackRole::VoiceChat,
                AudioTrackRole::Microphone,
            ],
            30,
        );
        let tracks = shared.enabled_tracks();
        for track in &tracks {
            complete_test_segment(track, 1, 0, 20_000_000);
        }
        let writer_tracks = tracks.clone();
        let simulated_writers = thread::spawn(move || {
            for (index, track) in writer_tracks.iter().enumerate() {
                let end = 26_000_000 + index as i64 * 1_000_000;
                let deadline = Instant::now() + Duration::from_secs(1);
                while !track.should_cut_after_packet(end) {
                    assert!(Instant::now() < deadline, "track cut was not requested");
                    thread::sleep(Duration::from_millis(1));
                }
                track.record_written_through(end);
                complete_test_segment(track, 2, 20_000_000, end);
            }
        });

        let result = shared
            .wait_for_coverage_and_plan(&test_timeline(0, 25_000_000), Duration::from_secs(1))
            .unwrap();
        simulated_writers.join().unwrap();
        assert_eq!(result.barriers.len(), 3);
        assert!(result
            .plans
            .iter()
            .all(|plan| plan.trailing_uncovered_ms == 0.0));
        assert!(result
            .barriers
            .iter()
            .all(|barrier| barrier.satisfying_segment_sequence == Some(2)));
        drop(result.pins);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn ended_source_remains_uncovered_without_waiting_or_timing_out() {
        let root =
            std::env::temp_dir().join(format!("replay-audio-ended-barrier-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let shared = test_shared(&root, &[AudioTrackRole::VoiceChat], 30);
        let track = shared.enabled_tracks().remove(0);
        complete_test_segment(&track, 1, 0, 20_000_000);
        track.set_terminal(AudioTrackState::Ended, Some("process exited".into()));

        let result = shared
            .wait_for_coverage_and_plan(&test_timeline(0, 25_000_000), Duration::from_millis(20))
            .unwrap();
        assert_eq!(result.plans[0].trailing_uncovered_ms, 500.0);
        assert!(!result.barriers[0].explicit_writer_cut_requested);
        assert!(!result.barriers[0].timed_out);
        assert_eq!(result.barriers[0].source_state, AudioTrackState::Ended);
        assert!(result.barriers[0].error_message.is_some());
        drop(result.pins);
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn barrier_timeout_preserves_running_state_and_allows_repeated_save() {
        let root = std::env::temp_dir().join(format!(
            "replay-audio-timeout-barrier-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let shared = test_shared(&root, &[AudioTrackRole::Microphone], 30);
        let track = shared.enabled_tracks().remove(0);
        complete_test_segment(&track, 1, 0, 20_000_000);

        let failure = shared
            .wait_for_coverage_and_plan(&test_timeline(0, 25_000_000), Duration::from_millis(20))
            .err()
            .expect("running track without a writer tail must time out");
        assert!(failure.message.contains("did not finalize"));
        assert_eq!(track.status().state, AudioTrackState::Running);
        assert!(shared.snapshot().save_barriers[0].timed_out);

        complete_test_segment(&track, 2, 20_000_000, 30_000_000);
        let repeated = shared
            .wait_for_coverage_and_plan(&test_timeline(0, 25_000_000), Duration::from_millis(20))
            .unwrap();
        assert_eq!(repeated.plans[0].trailing_uncovered_ms, 0.0);
        assert!(!repeated.barriers[0].explicit_writer_cut_requested);
        drop(repeated.pins);
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
