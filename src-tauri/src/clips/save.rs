use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::replay::{
    AudioSnapshotPlan, ReplayBufferManager, ReplayLifecycleState, ReplaySaveSnapshot,
    SavedReplayTimeline,
};

use super::assembler::{ClipAssembler, FfmpegClipAssembler};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SaveJobState {
    Idle,
    Preparing,
    FinalizingCurrentSegment,
    Assembling,
    Completed,
    Error,
}

impl SaveJobState {
    pub const fn is_active(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::FinalizingCurrentSegment | Self::Assembling
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
    pub selected_segment_count: usize,
    pub selected_segment_sequence_numbers: Vec<u64>,
    pub actual_earliest_timestamp_ms: Option<u64>,
    pub actual_latest_timestamp_ms: Option<u64>,
    pub output_path: Option<String>,
    pub file_size: Option<u64>,
    pub codec: Option<String>,
    pub error_message: Option<String>,
    pub audio_snapshot_plans: Vec<AudioSnapshotPlan>,
    pub video_timeline: Option<SavedReplayTimeline>,
    pub internal_encoded_duration_seconds: Option<f64>,
    pub ffprobe_duration_seconds: Option<f64>,
    pub internal_ffprobe_difference_ms: Option<f64>,
}

impl SaveReplayStatus {
    fn idle() -> Self {
        Self {
            state: SaveJobState::Idle,
            requested_duration_seconds: 0,
            actual_saved_duration_seconds: None,
            save_request_timestamp_ms: None,
            selected_segment_count: 0,
            selected_segment_sequence_numbers: Vec::new(),
            actual_earliest_timestamp_ms: None,
            actual_latest_timestamp_ms: None,
            output_path: None,
            file_size: None,
            codec: None,
            error_message: None,
            audio_snapshot_plans: Vec::new(),
            video_timeline: None,
            internal_encoded_duration_seconds: None,
            ffprobe_duration_seconds: None,
            internal_ffprobe_difference_ms: None,
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
            selected_segment_count: 0,
            selected_segment_sequence_numbers: Vec::new(),
            actual_earliest_timestamp_ms: None,
            actual_latest_timestamp_ms: None,
            output_path: None,
            file_size: None,
            codec: None,
            error_message: None,
            audio_snapshot_plans: Vec::new(),
            video_timeline: None,
            internal_encoded_duration_seconds: None,
            ffprobe_duration_seconds: None,
            internal_ffprobe_difference_ms: None,
        };
    }

    fn set_state(&self, state: SaveJobState) {
        self.lock().state = state;
    }

    fn set_snapshot(&self, snapshot: &ReplaySaveSnapshot) {
        let mut status = self.lock();
        status.save_request_timestamp_ms = Some(snapshot.save_request_timestamp_ms);
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
        status.video_timeline = Some(snapshot.video_timeline.clone());
        status.internal_encoded_duration_seconds =
            Some(snapshot.video_timeline.clip_playback_duration_100ns as f64 / 10_000_000.0);
    }

    fn complete(&self, result: super::assembler::ClipAssemblyResult) {
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
        status.error_message = None;
    }

    fn fail(&self, error: impl Into<String>) {
        let mut status = self.lock();
        status.state = SaveJobState::Error;
        status.error_message = Some(error.into());
        status.output_path = None;
        status.file_size = None;
    }
}

#[derive(Clone)]
pub struct ClipSaveManager {
    replay: ReplayBufferManager,
    output_directory: Arc<PathBuf>,
    shared: Arc<SharedSaveJob>,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl ClipSaveManager {
    pub fn new(replay: ReplayBufferManager, output_directory: PathBuf) -> Self {
        Self {
            replay,
            output_directory: Arc::new(output_directory),
            shared: Arc::new(SharedSaveJob::new()),
            worker: Arc::new(Mutex::new(None)),
        }
    }

    pub fn status(&self) -> SaveReplayStatus {
        self.shared.snapshot()
    }

    pub fn start(&self) -> SaveReplayCommandResult {
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
        let thread = match thread::Builder::new()
            .name("justin-replay-save".to_string())
            .spawn(move || run_save_job(replay, output_directory.as_ref(), shared))
        {
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
}

fn run_save_job(
    replay: ReplayBufferManager,
    output_directory: &PathBuf,
    shared: Arc<SharedSaveJob>,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        shared.set_state(SaveJobState::FinalizingCurrentSegment);
        let snapshot = replay.snapshot_for_save()?;
        shared.set_snapshot(&snapshot);
        shared.set_state(SaveJobState::Assembling);

        let timestamp = utc_file_timestamp()?;
        FfmpegClipAssembler.assemble(&snapshot.segments, output_directory, &timestamp)
    }));

    match result {
        Ok(Ok(result)) => shared.complete(result),
        Ok(Err(error)) => shared.fail(error),
        Err(_) => shared.fail("The Save Replay worker panicked."),
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
    use super::{duplicate_job_error, SaveJobState};

    #[test]
    fn duplicate_active_save_jobs_are_rejected() {
        for state in [
            SaveJobState::Preparing,
            SaveJobState::FinalizingCurrentSegment,
            SaveJobState::Assembling,
        ] {
            assert!(duplicate_job_error(state, true).is_some());
        }
        assert!(duplicate_job_error(SaveJobState::Completed, false).is_none());
        assert!(duplicate_job_error(SaveJobState::Error, false).is_none());
    }
}
