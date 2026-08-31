use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::library::{ClipAudioTrack, ClipLibraryManager, SavedClipIndexResult, SavedClipMetadata};
use crate::replay::{
    AudioSaveBarrierTelemetry, AudioSnapshotPlan, ReplayBufferManager, ReplayLifecycleState,
    ReplaySaveSnapshot, SavedReplayTimeline,
};

use super::assembler::{
    ClipAssemblyFailure, ClipAssemblyPhase, FfmpegClipAssembler, FinalMuxDiagnostics,
};
use super::audio_render::AudioRenderDiagnostics;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SaveJobState {
    Idle,
    Preparing,
    FinalizingCurrentSegment,
    AssemblingVideo,
    RenderingAudio,
    Muxing,
    Verifying,
    Promoting,
    Completed,
    Error,
}

impl SaveJobState {
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Preparing
                | Self::FinalizingCurrentSegment
                | Self::AssemblingVideo
                | Self::RenderingAudio
                | Self::Muxing
                | Self::Verifying
                | Self::Promoting
        )
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReplayStatus {
    pub state: SaveJobState,
    pub requested_duration_seconds: u32,
    pub actual_saved_duration_seconds: Option<f64>,
    pub save_request_timestamp_ms: Option<u64>,
    pub save_request_qpc_100ns: Option<i64>,
    pub selected_segment_count: usize,
    pub selected_segment_sequence_numbers: Vec<u64>,
    pub actual_earliest_timestamp_ms: Option<u64>,
    pub actual_latest_timestamp_ms: Option<u64>,
    pub output_path: Option<String>,
    pub file_size: Option<u64>,
    pub codec: Option<String>,
    pub error_message: Option<String>,
    pub audio_snapshot_plans: Vec<AudioSnapshotPlan>,
    pub audio_save_barriers: Vec<AudioSaveBarrierTelemetry>,
    pub video_boundary_wait_ms: Option<f64>,
    pub audio_barrier_wait_ms: Option<f64>,
    pub snapshot_ready_latency_ms: Option<f64>,
    pub total_save_latency_ms: Option<f64>,
    pub video_timeline: Option<SavedReplayTimeline>,
    pub internal_encoded_duration_seconds: Option<f64>,
    pub ffprobe_duration_seconds: Option<f64>,
    pub internal_ffprobe_difference_ms: Option<f64>,
    pub audio_render_diagnostics: Vec<AudioRenderDiagnostics>,
    pub final_mux: Option<FinalMuxDiagnostics>,
    pub temporary_workspace_path: Option<String>,
    pub temporary_video_path: Option<String>,
    pub temporary_artifacts_retained: bool,
    pub library_clip_id: Option<String>,
    pub library_indexed: Option<bool>,
    pub library_indexing_warning: Option<String>,
    pub library_insertion_latency_ms: Option<f64>,
}

impl SaveReplayStatus {
    fn idle() -> Self {
        Self {
            state: SaveJobState::Idle,
            requested_duration_seconds: 0,
            actual_saved_duration_seconds: None,
            save_request_timestamp_ms: None,
            save_request_qpc_100ns: None,
            selected_segment_count: 0,
            selected_segment_sequence_numbers: Vec::new(),
            actual_earliest_timestamp_ms: None,
            actual_latest_timestamp_ms: None,
            output_path: None,
            file_size: None,
            codec: None,
            error_message: None,
            audio_snapshot_plans: Vec::new(),
            audio_save_barriers: Vec::new(),
            video_boundary_wait_ms: None,
            audio_barrier_wait_ms: None,
            snapshot_ready_latency_ms: None,
            total_save_latency_ms: None,
            video_timeline: None,
            internal_encoded_duration_seconds: None,
            ffprobe_duration_seconds: None,
            internal_ffprobe_difference_ms: None,
            audio_render_diagnostics: Vec::new(),
            final_mux: None,
            temporary_workspace_path: None,
            temporary_video_path: None,
            temporary_artifacts_retained: false,
            library_clip_id: None,
            library_indexed: None,
            library_indexing_warning: None,
            library_insertion_latency_ms: None,
        }
    }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveReplayCommandResult {
    pub success: bool,
    pub status: SaveReplayStatus,
    pub error_message: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveReplayCompletionFeedback {
    success: bool,
    message: String,
    save_state: SaveJobState,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct SaveAndNameRequest {
    clip_id: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SaveIntent {
    SaveOnly,
    SaveAndName,
}

impl SaveReplayCommandResult {
    fn success(status: SaveReplayStatus) -> Self {
        Self {
            success: true,
            status,
            error_message: None,
        }
    }

    fn failure(status: SaveReplayStatus, error: impl Into<String>) -> Self {
        Self {
            success: false,
            status,
            error_message: Some(error.into()),
        }
    }
}

struct SharedSaveJob {
    status: Mutex<SaveReplayStatus>,
}

impl SharedSaveJob {
    fn new() -> Self {
        Self {
            status: Mutex::new(SaveReplayStatus::idle()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, SaveReplayStatus> {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn snapshot(&self) -> SaveReplayStatus {
        self.lock().clone()
    }

    fn begin(&self, requested_duration_seconds: u32) {
        *self.lock() = SaveReplayStatus {
            state: SaveJobState::Preparing,
            requested_duration_seconds,
            actual_saved_duration_seconds: None,
            save_request_timestamp_ms: None,
            save_request_qpc_100ns: None,
            selected_segment_count: 0,
            selected_segment_sequence_numbers: Vec::new(),
            actual_earliest_timestamp_ms: None,
            actual_latest_timestamp_ms: None,
            output_path: None,
            file_size: None,
            codec: None,
            error_message: None,
            audio_snapshot_plans: Vec::new(),
            audio_save_barriers: Vec::new(),
            video_boundary_wait_ms: None,
            audio_barrier_wait_ms: None,
            snapshot_ready_latency_ms: None,
            total_save_latency_ms: None,
            video_timeline: None,
            internal_encoded_duration_seconds: None,
            ffprobe_duration_seconds: None,
            internal_ffprobe_difference_ms: None,
            audio_render_diagnostics: Vec::new(),
            final_mux: None,
            temporary_workspace_path: None,
            temporary_video_path: None,
            temporary_artifacts_retained: false,
            library_clip_id: None,
            library_indexed: None,
            library_indexing_warning: None,
            library_insertion_latency_ms: None,
        };
    }

    fn set_state(&self, state: SaveJobState) {
        self.lock().state = state;
    }

    fn set_snapshot(&self, snapshot: &ReplaySaveSnapshot) {
        let mut status = self.lock();
        status.save_request_timestamp_ms = Some(snapshot.save_request_timestamp_ms);
        status.save_request_qpc_100ns = Some(snapshot.save_request_qpc_100ns);
        status.requested_duration_seconds = snapshot.requested_duration_seconds;
        status.selected_segment_count = snapshot.segments.len();
        status.selected_segment_sequence_numbers = snapshot
            .segments
            .iter()
            .map(|segment| segment.sequence_number)
            .collect();
        status.actual_saved_duration_seconds =
            Some(snapshot.video_timeline.clip_playback_duration_100ns as f64 / 10_000_000.0);
        status.actual_earliest_timestamp_ms = snapshot
            .segments
            .first()
            .map(|segment| segment.start_timestamp_ms);
        status.actual_latest_timestamp_ms = snapshot
            .segments
            .last()
            .map(|segment| segment.end_timestamp_ms);
        status.codec = snapshot
            .segments
            .first()
            .map(|segment| segment.codec.clone());
        status.audio_snapshot_plans = snapshot.audio_snapshot_plans.clone();
        status.audio_save_barriers = snapshot.audio_save_barriers.clone();
        status.video_boundary_wait_ms = Some(snapshot.video_boundary_wait_ms);
        status.audio_barrier_wait_ms = Some(snapshot.audio_barrier_wait_ms);
        status.snapshot_ready_latency_ms = Some(snapshot.snapshot_ready_latency_ms);
        status.video_timeline = Some(snapshot.video_timeline.clone());
        status.internal_encoded_duration_seconds =
            Some(snapshot.video_timeline.clip_playback_duration_100ns as f64 / 10_000_000.0);
    }

    fn complete(&self, outcome: SaveJobOutcome, total_save_latency_ms: f64) {
        let result = outcome.assembly;
        let mut status = self.lock();
        status.state = SaveJobState::Completed;
        status.actual_saved_duration_seconds = Some(result.actual_duration_seconds);
        status.actual_earliest_timestamp_ms = Some(result.earliest_timestamp_ms);
        status.actual_latest_timestamp_ms = Some(result.latest_timestamp_ms);
        status.output_path = Some(result.output_path.to_string_lossy().into_owned());
        status.file_size = Some(result.file_size);
        status.codec = Some(result.codec);
        status.internal_encoded_duration_seconds = Some(result.internal_encoded_duration_seconds);
        status.ffprobe_duration_seconds = result.ffprobe_duration_seconds;
        status.internal_ffprobe_difference_ms = result.internal_ffprobe_difference_ms;
        status.audio_render_diagnostics = result.audio_render_diagnostics;
        status.final_mux = Some(result.final_mux);
        status.total_save_latency_ms = Some(total_save_latency_ms);
        status.temporary_workspace_path = None;
        status.temporary_video_path = None;
        status.temporary_artifacts_retained = false;
        status.library_clip_id = outcome
            .index_result
            .as_ref()
            .map(|value| value.clip_id.clone());
        status.library_indexed = Some(outcome.index_result.is_some());
        status.library_indexing_warning = outcome.index_warning;
        status.library_insertion_latency_ms = outcome
            .index_result
            .as_ref()
            .map(|value| value.insertion_ms)
            .or(outcome.failed_index_latency_ms);
        status.error_message = None;
    }

    fn fail(&self, error: impl Into<String>) {
        let mut status = self.lock();
        status.state = SaveJobState::Error;
        status.error_message = Some(error.into());
        status.output_path = None;
        status.file_size = None;
    }

    fn fail_with_audio_barriers(
        &self,
        failure: ClipAssemblyFailure,
        barriers: Vec<AudioSaveBarrierTelemetry>,
        total_save_latency_ms: f64,
    ) {
        let mut status = self.lock();
        status.state = SaveJobState::Error;
        status.error_message = Some(failure.message);
        status.audio_save_barriers = barriers;
        status.total_save_latency_ms = Some(total_save_latency_ms);
        status.output_path = None;
        status.file_size = None;
        status.temporary_workspace_path = failure
            .temporary_workspace_path
            .map(|path| path.to_string_lossy().into_owned());
        status.temporary_video_path = failure
            .temporary_video_path
            .map(|path| path.to_string_lossy().into_owned());
        status.temporary_artifacts_retained = failure.temporary_artifacts_retained;
    }
}

#[derive(Clone)]
pub struct ClipSaveManager {
    replay: ReplayBufferManager,
    output_directory: Arc<PathBuf>,
    shared: Arc<SharedSaveJob>,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
    library: ClipLibraryManager,
    app_handle: AppHandle,
}

impl ClipSaveManager {
    pub fn new(
        replay: ReplayBufferManager,
        output_directory: PathBuf,
        library: ClipLibraryManager,
        app_handle: AppHandle,
    ) -> Self {
        Self {
            replay,
            output_directory: Arc::new(output_directory),
            shared: Arc::new(SharedSaveJob::new()),
            worker: Arc::new(Mutex::new(None)),
            library,
            app_handle,
        }
    }

    pub fn status(&self) -> SaveReplayStatus {
        self.shared.snapshot()
    }

    pub fn start(&self) -> SaveReplayCommandResult {
        self.start_with_intent(SaveIntent::SaveOnly)
    }

    pub fn start_and_name(&self) -> SaveReplayCommandResult {
        self.start_with_intent(SaveIntent::SaveAndName)
    }

    fn start_with_intent(&self, intent: SaveIntent) -> SaveReplayCommandResult {
        let replay_status = self.replay.status();
        if replay_status.state != ReplayLifecycleState::Running {
            return SaveReplayCommandResult::failure(
                self.status(),
                "Save Replay requires a running replay buffer.",
            );
        }
        if replay_status.completed_segment_count == 0 {
            return SaveReplayCommandResult::failure(
                self.status(),
                "No finalized replay video is available yet.",
            );
        }

        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(finished) = worker.take() {
                let _ = finished.join();
            }
        }
        let current = self.status();
        if let Some(error) = duplicate_job_error(current.state, worker.is_some()) {
            return SaveReplayCommandResult::failure(current, error);
        }

        self.shared.begin(replay_status.replay_duration_seconds);
        let shared = Arc::clone(&self.shared);
        let replay = self.replay.clone();
        let output_directory = Arc::clone(&self.output_directory);
        let library = self.library.clone();
        let app_handle = self.app_handle.clone();
        let thread = match thread::Builder::new()
            .name("slickclip-save".to_string())
            .spawn(move || {
                run_save_job(
                    replay,
                    output_directory.as_ref(),
                    shared,
                    library,
                    app_handle,
                    intent,
                )
            }) {
            Ok(thread) => thread,
            Err(error) => {
                let message = format!("Could not start the Save Replay worker: {error}");
                self.shared.fail(&message);
                return SaveReplayCommandResult::failure(self.status(), message);
            }
        };
        *worker = Some(thread);

        SaveReplayCommandResult::success(self.status())
    }

    pub fn shutdown_and_wait(&self) {
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            if worker.join().is_err() {
                self.shared
                    .fail("The Save Replay worker panicked during app shutdown.");
            }
        }
    }

    pub(crate) fn finalize_external_recording(
        &self,
        snapshot: &ReplaySaveSnapshot,
    ) -> Result<PathBuf, String> {
        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(finished) = worker.take() {
                let _ = finished.join();
            }
        }
        if duplicate_job_error(self.status().state, worker.is_some()).is_some() {
            return Err("A Replay save is already in progress.".to_string());
        }
        drop(worker);

        let timestamp = format!(
            "WatchParty-{}",
            utc_file_timestamp().map_err(|error| error.to_string())?
        );
        let assembly = FfmpegClipAssembler
            .assemble(
                snapshot,
                self.output_directory.as_ref(),
                &timestamp,
                &|_| {},
            )
            .map_err(|failure| failure.message)?;
        let metadata = saved_clip_metadata(snapshot, &assembly);
        let indexed = self.library.index_saved_clip(metadata).map_err(|error| {
            format!(
                "Watch Party was saved to '{}', but Library indexing failed: {error}",
                assembly.output_path.display()
            )
        })?;
        let _ = self
            .app_handle
            .emit("clip-library-changed", indexed.clip_id.clone());
        Ok(assembly.output_path)
    }
}

fn run_save_job(
    replay: ReplayBufferManager,
    output_directory: &PathBuf,
    shared: Arc<SharedSaveJob>,
    library: ClipLibraryManager,
    app_handle: AppHandle,
    intent: SaveIntent,
) {
    let save_started = Instant::now();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(
        || -> Result<SaveJobOutcome, ClipAssemblyFailure> {
            shared.set_state(SaveJobState::FinalizingCurrentSegment);
            let snapshot = replay
                .snapshot_for_save()
                .map_err(ClipAssemblyFailure::without_artifacts)?;
            shared.set_snapshot(&snapshot);
            let timestamp = utc_file_timestamp().map_err(ClipAssemblyFailure::without_artifacts)?;
            let assembly = FfmpegClipAssembler.assemble(
                &snapshot,
                output_directory,
                &timestamp,
                &|phase| shared.set_state(save_state_for_phase(phase)),
            )?;
            let index_started = Instant::now();
            let metadata = saved_clip_metadata(&snapshot, &assembly);
            let (index_result, index_warning, failed_index_latency_ms) = match library
                .index_saved_clip(metadata)
            {
                Ok(indexed) => {
                    let _ = app_handle.emit("clip-library-changed", indexed.clip_id.clone());
                    (Some(indexed), None, None)
                }
                Err(error) => {
                    let elapsed = index_started.elapsed().as_secs_f64() * 1_000.0;
                    library.record_saved_clip_index_failure(elapsed);
                    (
                        None,
                        Some(format!(
                            "Replay saved successfully, but library indexing failed: {error}. Refresh Clips to retry discovery."
                        )),
                        Some(elapsed),
                    )
                }
            };
            Ok(SaveJobOutcome {
                assembly,
                index_result,
                index_warning,
                failed_index_latency_ms,
                overlay_monitor_origin: snapshot.overlay_monitor_origin,
            })
        },
    ));

    let total_save_latency_ms = save_started.elapsed().as_secs_f64() * 1_000.0;
    match result {
        Ok(Ok(result)) => {
            let duration_seconds = result.assembly.actual_duration_seconds;
            let library_indexed = result.index_result.is_some();
            let indexing_warning = result.index_warning.clone();
            let overlay_monitor_origin = result.overlay_monitor_origin;
            let naming_clip_id = should_request_name(intent, library_indexed)
                .then(|| {
                    result
                        .index_result
                        .as_ref()
                        .map(|value| value.clip_id.clone())
                })
                .flatten();
            shared.complete(result, total_save_latency_ms);
            if should_show_success_overlay(true, library_indexed) {
                emit_save_completion_feedback(
                    &app_handle,
                    true,
                    "Replay Saved".to_string(),
                    SaveJobState::Completed,
                );
                crate::desktop::show_save_overlay(
                    &app_handle,
                    duration_seconds,
                    overlay_monitor_origin,
                );
                if let Some(clip_id) = naming_clip_id {
                    let _ = app_handle
                        .emit("save-replay-name-requested", SaveAndNameRequest { clip_id });
                    let _ = crate::desktop::show_main_window(&app_handle);
                }
            } else {
                let message = indexing_warning.unwrap_or_else(|| {
                    "The replay file was created, but SlickClip could not index it in the Library."
                        .to_string()
                });
                emit_save_completion_feedback(
                    &app_handle,
                    false,
                    message.clone(),
                    SaveJobState::Completed,
                );
                crate::desktop::show_save_failure_overlay(&app_handle, &message);
            }
        }
        Ok(Err(failure)) => {
            let message = failure.message.clone();
            shared.fail_with_audio_barriers(
                failure,
                replay.status().audio.save_barriers,
                total_save_latency_ms,
            );
            emit_save_completion_feedback(&app_handle, false, message.clone(), SaveJobState::Error);
            crate::desktop::show_save_failure_overlay(&app_handle, &message);
        }
        Err(_) => {
            let message = "The Save Replay worker panicked.".to_string();
            shared.fail_with_audio_barriers(
                ClipAssemblyFailure::without_artifacts(&message),
                replay.status().audio.save_barriers,
                total_save_latency_ms,
            );
            emit_save_completion_feedback(&app_handle, false, message.clone(), SaveJobState::Error);
            crate::desktop::show_save_failure_overlay(&app_handle, &message);
        }
    }
    crate::desktop::refresh_tray_status(&app_handle);
}

fn emit_save_completion_feedback(
    app: &AppHandle,
    success: bool,
    message: String,
    save_state: SaveJobState,
) {
    let _ = app.emit(
        "save-replay-completed",
        SaveReplayCompletionFeedback {
            success,
            message,
            save_state,
        },
    );
}

fn should_show_success_overlay(assembly_succeeded: bool, library_indexed: bool) -> bool {
    assembly_succeeded && library_indexed
}

fn should_request_name(intent: SaveIntent, library_indexed: bool) -> bool {
    intent == SaveIntent::SaveAndName && library_indexed
}

struct SaveJobOutcome {
    assembly: super::assembler::ClipAssemblyResult,
    index_result: Option<SavedClipIndexResult>,
    index_warning: Option<String>,
    failed_index_latency_ms: Option<f64>,
    overlay_monitor_origin: Option<(i32, i32)>,
}

fn saved_clip_metadata(
    snapshot: &ReplaySaveSnapshot,
    assembly: &super::assembler::ClipAssemblyResult,
) -> SavedClipMetadata {
    let first = &snapshot.segments[0];
    SavedClipMetadata {
        file_path: assembly.output_path.clone(),
        created_at_ms: i64::try_from(snapshot.save_request_timestamp_ms).unwrap_or(i64::MAX),
        duration_100ns: (assembly.actual_duration_seconds * 10_000_000.0).round() as i64,
        requested_duration_seconds: snapshot.requested_duration_seconds,
        width: first.width,
        height: first.height,
        fps_numerator: first.frame_rate,
        fps_denominator: 1,
        video_codec: assembly.codec.clone(),
        video_profile: assembly.final_mux.video_profile.clone(),
        video_bitrate_bps: assembly
            .final_mux
            .video_bitrate_mbps
            .map(|value| (value * 1_000_000.0).round() as u64),
        total_bitrate_bps: assembly
            .final_mux
            .total_bitrate_mbps
            .map(|value| (value * 1_000_000.0).round() as u64),
        capture_target_label: snapshot.capture_target_label.clone(),
        capture_target_type: snapshot.capture_target_type.clone(),
        audio_tracks: assembly
            .final_mux
            .audio_streams
            .iter()
            .map(|stream| ClipAudioTrack {
                stream_index: stream.stream_index,
                role: library_audio_role(&stream.title).to_string(),
                title: Some(stream.title.clone()),
                handler_name: Some(stream.title.clone()),
                codec: stream.codec.clone(),
                profile: Some("LC".to_string()),
                sample_rate: Some(stream.sample_rate),
                channels: Some(stream.channels),
                bitrate_bps: stream
                    .bitrate_kbps
                    .map(|value| (value * 1_000.0).round() as u64),
                is_default: stream.is_default,
            })
            .collect(),
    }
}

fn library_audio_role(title: &str) -> &'static str {
    match title {
        "Combined" => "Combined",
        "Game" => "Game",
        "Voice Chat" => "VoiceChat",
        "Microphone" => "Microphone",
        "Other" => "Other",
        _ => "Unknown",
    }
}

fn save_state_for_phase(phase: ClipAssemblyPhase) -> SaveJobState {
    match phase {
        ClipAssemblyPhase::AssemblingVideo => SaveJobState::AssemblingVideo,
        ClipAssemblyPhase::RenderingAudio => SaveJobState::RenderingAudio,
        ClipAssemblyPhase::Muxing => SaveJobState::Muxing,
        ClipAssemblyPhase::Verifying => SaveJobState::Verifying,
        ClipAssemblyPhase::Promoting => SaveJobState::Promoting,
    }
}

fn duplicate_job_error(state: SaveJobState, worker_exists: bool) -> Option<&'static str> {
    if state.is_active() || worker_exists {
        Some("A Save Replay job is already in progress.")
    } else {
        None
    }
}

fn utc_file_timestamp() -> Result<String, String> {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| format!("The system clock is invalid: {error}"))?
        .as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let seconds_in_day = seconds.rem_euclid(86_400);
    let shifted_days = days + 719_468;
    let era = shifted_days.div_euclid(146_097);
    let day_of_era = shifted_days - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    let hour = seconds_in_day / 3_600;
    let minute = seconds_in_day % 3_600 / 60;
    let second = seconds_in_day % 60;

    Ok(format!(
        "{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}"
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        duplicate_job_error, should_request_name, should_show_success_overlay, SaveIntent,
        SaveJobState,
    };

    #[test]
    fn duplicate_active_save_jobs_are_rejected() {
        for state in [
            SaveJobState::Preparing,
            SaveJobState::FinalizingCurrentSegment,
            SaveJobState::AssemblingVideo,
            SaveJobState::RenderingAudio,
            SaveJobState::Muxing,
            SaveJobState::Verifying,
            SaveJobState::Promoting,
        ] {
            assert!(duplicate_job_error(state, true).is_some());
        }
        assert!(duplicate_job_error(SaveJobState::Completed, false).is_none());
        assert!(duplicate_job_error(SaveJobState::Error, false).is_none());
    }

    #[test]
    fn replay_saved_overlay_requires_successful_assembly_and_indexing() {
        assert!(should_show_success_overlay(true, true));
        assert!(!should_show_success_overlay(false, true));
        assert!(!should_show_success_overlay(true, false));
        assert!(!should_show_success_overlay(false, false));
    }

    #[test]
    fn naming_prompt_requires_the_explicit_intent_and_successful_indexing() {
        assert!(should_request_name(SaveIntent::SaveAndName, true));
        assert!(!should_request_name(SaveIntent::SaveOnly, true));
        assert!(!should_request_name(SaveIntent::SaveAndName, false));
    }
}
