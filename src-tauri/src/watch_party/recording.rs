use std::error::Error;
use std::fmt;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
use windows::Win32::Storage::FileSystem::FILE_ATTRIBUTE_REPARSE_POINT;
use windows_capture::capture::{GraphicsCaptureApiError, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use crate::capture::capture_test::ensure_borderless_capture_access;
use crate::capture::encoder::{EncoderCodec, WindowsCaptureFileBackend};
use crate::capture::targets::{
    resolve_discord_window, resolve_target, CaptureTargetRequest, NativeCaptureTarget,
    ResolvedCaptureTarget,
};
use crate::clips::ClipSaveManager;
use crate::replay::segment::{CompletedSegment, VideoFrameTimingPoint};
use crate::replay::{
    AudioReplayConfiguration, AudioReplaySession, AudioReplayShared, AudioSnapshotPinGuard,
    ReplaySaveSnapshot, ReplaySessionClock, SavedReplayTimeline,
};

use super::checkpoint::{recoverable_sessions, WatchPartyCheckpoint};
use super::compositor::{CpuFrame, GpuCompositor};
use super::layout::{composition_plan, WatchPartyLayout};

const CANVAS_WIDTH: u32 = 1920;
const CANVAS_HEIGHT: u32 = 1080;
const FRAME_RATE: u32 = 30;
const SEGMENT_SECONDS: u64 = 30;
const SEGMENT_FRAMES: u64 = SEGMENT_SECONDS * FRAME_RATE as u64;
const MINIMUM_FREE_BYTES: u64 = 2 * 1024 * 1024 * 1024;
const LONG_FORM_AUDIO_RETENTION_SECONDS: u32 = 24 * 60 * 60;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum WatchPartyState {
    #[default]
    Stopped,
    Starting,
    Recording,
    Stopping,
    Finalizing,
    Completed,
    Error,
}

impl WatchPartyState {
    pub(crate) fn active(self) -> bool {
        matches!(
            self,
            Self::Starting | Self::Recording | Self::Stopping | Self::Finalizing
        )
    }
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPartySourceStatus {
    pub label: Option<String>,
    pub width: u32,
    pub height: u32,
    pub frames_received: u64,
    pub closed: bool,
    pub error_message: Option<String>,
}

#[derive(Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPartyStatus {
    pub state: WatchPartyState,
    pub session_id: Option<String>,
    pub layout: WatchPartyLayout,
    pub elapsed_seconds: f64,
    pub finalized_segment_count: usize,
    pub frames_composed: u64,
    pub main_source: WatchPartySourceStatus,
    pub reaction_source: WatchPartySourceStatus,
    pub output_path: Option<String>,
    pub error_message: Option<String>,
    pub recoverable_session_ids: Vec<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPartyStartRequest {
    pub main_target: CaptureTargetRequest,
    pub reaction_window_id: String,
    pub layout: WatchPartyLayout,
    #[serde(default)]
    pub audio: AudioReplayConfiguration,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPartyCommandResult {
    pub success: bool,
    pub status: WatchPartyStatus,
    pub error_message: Option<String>,
}

impl WatchPartyCommandResult {
    pub(crate) fn rejected(status: WatchPartyStatus, error: impl Into<String>) -> Self {
        failure(status, error)
    }
}

struct LatestSource {
    status: WatchPartySourceStatus,
    frame: Option<CpuFrame>,
    generation: u64,
}

impl LatestSource {
    fn new(label: String, width: u32, height: u32) -> Self {
        Self {
            status: WatchPartySourceStatus {
                label: Some(label),
                width,
                height,
                ..Default::default()
            },
            frame: None,
            generation: 0,
        }
    }
}

struct SharedRecording {
    status: Mutex<WatchPartyStatus>,
    main: Arc<Mutex<LatestSource>>,
    reaction: Arc<Mutex<LatestSource>>,
    stop: AtomicBool,
}

impl SharedRecording {
    fn snapshot(&self, root: &Path) -> WatchPartyStatus {
        let mut status = lock(&self.status).clone();
        status.main_source = lock(&self.main).status.clone();
        status.reaction_source = lock(&self.reaction).status.clone();
        status.recoverable_session_ids = if status.state.active() {
            Vec::new()
        } else {
            recoverable_sessions(root)
                .into_iter()
                .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
                .collect()
        };
        status
    }

    fn fail(&self, message: impl Into<String>) {
        let mut status = lock(&self.status);
        status.state = WatchPartyState::Error;
        status.error_message = Some(message.into());
        self.stop.store(true, Ordering::Release);
    }
}

#[derive(Clone)]
pub struct WatchPartyManager {
    root: Arc<PathBuf>,
    clips: ClipSaveManager,
    active: Arc<Mutex<Option<Arc<SharedRecording>>>>,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
}

impl WatchPartyManager {
    pub fn new(root: PathBuf, clips: ClipSaveManager) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|error| {
            format!(
                "Could not create Watch Party workspace '{}': {error}",
                root.display()
            )
        })?;
        Ok(Self {
            root: Arc::new(root),
            clips,
            active: Arc::new(Mutex::new(None)),
            worker: Arc::new(Mutex::new(None)),
        })
    }

    pub fn status(&self) -> WatchPartyStatus {
        lock(&self.active)
            .as_ref()
            .map(|shared| shared.snapshot(&self.root))
            .unwrap_or_else(|| WatchPartyStatus {
                recoverable_session_ids: recoverable_sessions(&self.root)
                    .into_iter()
                    .filter_map(|path| path.file_name()?.to_str().map(str::to_string))
                    .collect(),
                ..Default::default()
            })
    }

    pub fn start(&self, request: WatchPartyStartRequest) -> WatchPartyCommandResult {
        if let Err(error) = request.audio.validate() {
            return failure(self.status(), error);
        }
        self.reap_worker();
        if self.status().state.active() {
            return failure(self.status(), "A Watch Party recording is already active.");
        }
        let session_id = format!("watch-party-{}", now_ms());
        let placeholder = Arc::new(SharedRecording {
            status: Mutex::new(WatchPartyStatus {
                state: WatchPartyState::Starting,
                session_id: Some(session_id.clone()),
                layout: request.layout,
                ..Default::default()
            }),
            main: Arc::new(Mutex::new(LatestSource::new(
                "Main content".to_string(),
                0,
                0,
            ))),
            reaction: Arc::new(Mutex::new(LatestSource::new("Discord".to_string(), 0, 0))),
            stop: AtomicBool::new(false),
        });
        *lock(&self.active) = Some(Arc::clone(&placeholder));
        let root = Arc::clone(&self.root);
        let clips = self.clips.clone();
        let worker_shared = Arc::clone(&placeholder);
        let worker = thread::Builder::new()
            .name("slickclip-watch-party".to_string())
            .spawn(move || {
                if let Err(error) = run_recording(&root, &clips, &worker_shared, request) {
                    worker_shared.fail(error);
                }
            });
        match worker {
            Ok(worker) => *lock(&self.worker) = Some(worker),
            Err(error) => placeholder.fail(format!("Could not start Watch Party worker: {error}")),
        }
        let status = placeholder.snapshot(&self.root);
        if status.state == WatchPartyState::Error {
            failure(
                status.clone(),
                status.error_message.clone().unwrap_or_default(),
            )
        } else {
            success(status)
        }
    }

    pub fn stop(&self) -> WatchPartyCommandResult {
        let Some(shared) = lock(&self.active).clone() else {
            return failure(self.status(), "No Watch Party recording is active.");
        };
        {
            let mut status = lock(&shared.status);
            if !status.state.active() {
                return failure(
                    shared.snapshot(&self.root),
                    "No Watch Party recording is active.",
                );
            }
            status.state = WatchPartyState::Stopping;
        }
        shared.stop.store(true, Ordering::Release);
        success(shared.snapshot(&self.root))
    }

    pub fn recover(&self, session_id: &str) -> WatchPartyCommandResult {
        self.reap_worker();
        if self.status().state.active() {
            return failure(
                self.status(),
                "Stop the active Watch Party recording before recovery.",
            );
        }
        if session_id.is_empty()
            || session_id.contains(['/', '\\'])
            || !session_id.starts_with("watch-party-")
        {
            return failure(
                self.status(),
                "The Watch Party recovery identifier is invalid.",
            );
        }
        let session_directory = self.root.join(session_id);
        let checkpoint = match WatchPartyCheckpoint::read(&session_directory) {
            Ok(checkpoint) => checkpoint,
            Err(error) => return failure(self.status(), error),
        };
        let timeline = match SavedReplayTimeline::from_segments(&checkpoint.segments) {
            Ok(timeline) => timeline,
            Err(error) => return failure(self.status(), error),
        };
        let duration = (timeline.clip_playback_duration_100ns / 10_000_000)
            .clamp(1, i64::from(u32::MAX)) as u32;
        let snapshot = ReplaySaveSnapshot::from_completed_recording(
            now_ms(),
            duration,
            format!("{} + {}", checkpoint.main_label, checkpoint.reaction_label),
            checkpoint.segments,
            timeline,
            Vec::new(),
            Vec::new(),
            Vec::new(),
            AudioSnapshotPinGuard::empty(),
            0.0,
        );
        match self.clips.finalize_external_recording(&snapshot) {
            Ok(path) => success(WatchPartyStatus {
                state: WatchPartyState::Completed,
                session_id: Some(session_id.to_string()),
                output_path: Some(path.to_string_lossy().into_owned()),
                recoverable_session_ids: self.status().recoverable_session_ids,
                ..Default::default()
            }),
            Err(error) => failure(self.status(), error),
        }
    }

    pub fn shutdown_and_wait(&self) {
        if let Some(shared) = lock(&self.active).as_ref() {
            shared.stop.store(true, Ordering::Release);
        }
        if let Some(worker) = lock(&self.worker).take() {
            let _ = worker.join();
        }
    }

    fn reap_worker(&self) {
        let finished = lock(&self.worker)
            .as_ref()
            .is_some_and(JoinHandle::is_finished);
        if finished {
            if let Some(worker) = lock(&self.worker).take() {
                let _ = worker.join();
            }
        }
    }
}

fn run_recording(
    root: &Path,
    clips: &ClipSaveManager,
    shared: &Arc<SharedRecording>,
    request: WatchPartyStartRequest,
) -> Result<(), String> {
    ensure_borderless_capture_access()?;
    let ResolvedCaptureTarget {
        target: main_target,
        label: main_label,
        width: main_width,
        height: main_height,
        process_id: main_process_id,
    } = resolve_target(&request.main_target)?;
    let ResolvedCaptureTarget {
        target: reaction_target,
        label: reaction_label,
        width: reaction_width,
        height: reaction_height,
        process_id: reaction_process_id,
    } = resolve_discord_window(&request.reaction_window_id)?;
    validate_required_audio(
        &request.audio,
        main_process_id,
        reaction_process_id
            .ok_or_else(|| "Discord process identity is unavailable.".to_string())?,
    )?;
    *lock(&shared.main) = LatestSource::new(main_label.clone(), main_width, main_height);
    *lock(&shared.reaction) =
        LatestSource::new(reaction_label.clone(), reaction_width, reaction_height);

    let session_id = lock(&shared.status)
        .session_id
        .clone()
        .ok_or_else(|| "Watch Party session identifier is missing.".to_string())?;
    let session_directory = root.join(&session_id);
    fs::create_dir_all(&session_directory).map_err(|error| {
        format!(
            "Could not create Watch Party session '{}': {error}",
            session_directory.display()
        )
    })?;
    ensure_free_space(&session_directory)?;

    let clock = ReplaySessionClock::create()?;
    let audio = Arc::new(AudioReplayShared::new());
    audio.begin(
        &request.audio,
        clock.clone(),
        session_directory.clone(),
        LONG_FORM_AUDIO_RETENTION_SECONDS,
    )?;
    let mut audio_session = AudioReplaySession::prepare(audio.enabled_tracks(), clock.clone())?;

    let main_worker = start_source_worker(
        "main",
        main_target,
        Arc::clone(&shared.main),
        Arc::clone(shared),
    )?;
    let reaction_worker = start_source_worker(
        "Discord reaction",
        reaction_target,
        Arc::clone(&shared.reaction),
        Arc::clone(shared),
    )?;
    wait_for_initial_sources(shared)?;
    audio_session.start()?;
    lock(&shared.status).state = WatchPartyState::Recording;

    let started_at_ms = now_ms();
    let started = Instant::now();
    let video_start_qpc = {
        let main = lock(&shared.main)
            .frame
            .as_ref()
            .unwrap()
            .captured_qpc_100ns;
        let reaction = lock(&shared.reaction)
            .frame
            .as_ref()
            .unwrap()
            .captured_qpc_100ns;
        main.max(reaction)
    };
    let mut compositor = GpuCompositor::new(CANVAS_WIDTH, CANVAS_HEIGHT)?;
    let mut segments = Vec::new();
    let mut global_frame = 0u64;
    while !shared.stop.load(Ordering::Acquire) {
        ensure_free_space(&session_directory)?;
        let sequence = segments.len() as u64 + 1;
        let segment = record_segment(
            &session_directory,
            sequence,
            video_start_qpc,
            &mut global_frame,
            started,
            shared,
            request.layout,
            &mut compositor,
        )?;
        if segment.frame_count > 0 {
            segments.push(segment);
            let checkpoint = WatchPartyCheckpoint {
                schema_version: 1,
                session_id: session_id.clone(),
                state: "recording".to_string(),
                layout: request.layout,
                main_label: main_label.clone(),
                reaction_label: reaction_label.clone(),
                started_at_ms,
                segments: segments.clone(),
                last_error: current_source_error(shared),
            };
            checkpoint.write_atomic(&session_directory)?;
            lock(&shared.status).finalized_segment_count = segments.len();
        }
    }

    lock(&shared.status).state = WatchPartyState::Finalizing;
    let _ = main_worker.join();
    let _ = reaction_worker.join();
    if segments.is_empty() {
        audio_session.stop_and_wait();
        return Err("Watch Party stopped before any video could be finalized.".to_string());
    }
    let timeline = SavedReplayTimeline::from_segments(&segments)?;
    let barrier = audio
        .wait_for_coverage_and_plan(&timeline, Duration::from_secs(5))
        .map_err(|failure| failure.message)?;
    audio_session.stop_and_wait();
    let duration =
        (timeline.clip_playback_duration_100ns / 10_000_000).clamp(1, i64::from(u32::MAX)) as u32;
    let snapshot = ReplaySaveSnapshot::from_completed_recording(
        now_ms(),
        duration,
        format!("{main_label} + {reaction_label}"),
        segments.clone(),
        timeline,
        barrier.plans,
        barrier.tracks,
        barrier.barriers,
        barrier.pins,
        barrier.wait_duration.as_secs_f64() * 1_000.0,
    );
    let output = clips.finalize_external_recording(&snapshot)?;
    WatchPartyCheckpoint {
        schema_version: 1,
        session_id,
        state: "completed".to_string(),
        layout: request.layout,
        main_label,
        reaction_label,
        started_at_ms,
        segments,
        last_error: current_source_error(shared),
    }
    .write_atomic(&session_directory)?;
    drop(snapshot);
    let cleanup_warning = cleanup_completed_session(root, &session_directory).err();
    let mut status = lock(&shared.status);
    status.state = WatchPartyState::Completed;
    status.elapsed_seconds = started.elapsed().as_secs_f64();
    status.output_path = Some(output.to_string_lossy().into_owned());
    status.error_message = cleanup_warning;
    Ok(())
}

fn cleanup_completed_session(root: &Path, session_directory: &Path) -> Result<(), String> {
    if session_directory.parent() != Some(root)
        || !session_directory
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|name| name.starts_with("watch-party-"))
    {
        return Err("Refused to clean a Watch Party workspace outside its owned root.".to_string());
    }
    let metadata = fs::symlink_metadata(session_directory).map_err(|error| {
        format!("The saved Watch Party workspace could not be inspected for cleanup: {error}")
    })?;
    if metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0 {
        return Err(
            "The saved Watch Party workspace is a reparse point and was retained for safety."
                .to_string(),
        );
    }
    let canonical_root = root.canonicalize().map_err(|error| {
        format!("The Watch Party root could not be resolved for cleanup: {error}")
    })?;
    let canonical_session = session_directory.canonicalize().map_err(|error| {
        format!("The Watch Party workspace could not be resolved for cleanup: {error}")
    })?;
    if canonical_session.parent() != Some(canonical_root.as_path()) {
        return Err(
            "Refused to clean a Watch Party workspace outside its canonical root.".to_string(),
        );
    }
    fs::remove_dir_all(&canonical_session).map_err(|error| {
        format!("The Watch Party was saved, but temporary session cleanup failed: {error}")
    })
}

#[allow(clippy::too_many_arguments)]
fn record_segment(
    session_directory: &Path,
    sequence: u64,
    video_start_qpc: i64,
    global_frame: &mut u64,
    session_started: Instant,
    shared: &SharedRecording,
    layout: WatchPartyLayout,
    compositor: &mut GpuCompositor,
) -> Result<CompletedSegment, String> {
    let path = session_directory.join(format!("segment-{sequence:06}.mp4"));
    let encoder_started = Instant::now();
    let mut encoder = WindowsCaptureFileBackend::create(
        &path,
        EncoderCodec::H264,
        CANVAS_WIDTH,
        CANVAS_HEIGHT,
        FRAME_RATE,
    )
    .map_err(|error| format!("Could not initialize Watch Party H.264 segment: {error}"))?;
    let encoder_creation_ms = encoder_started.elapsed().as_secs_f64() * 1_000.0;
    let segment_global_start = *global_frame;
    let segment_start_ms = now_ms();
    let mut timing = Vec::new();
    let mut last_generations = None;
    for local_frame in 0..SEGMENT_FRAMES {
        if shared.stop.load(Ordering::Acquire) && local_frame > 0 {
            break;
        }
        let due = session_started
            + Duration::from_nanos(
                ((*global_frame as u128 * 1_000_000_000) / FRAME_RATE as u128) as u64,
            );
        if let Some(wait) = due.checked_duration_since(Instant::now()) {
            thread::sleep(wait.min(Duration::from_millis(34)));
        }
        let main = lock(&shared.main)
            .frame
            .clone()
            .ok_or_else(|| "The main Watch Party source has no visual frame.".to_string())?;
        let reaction = lock(&shared.reaction)
            .frame
            .clone()
            .ok_or_else(|| "The Discord reaction source has no visual frame.".to_string())?;
        let plan = composition_plan(
            layout,
            CANVAS_WIDTH,
            CANVAS_HEIGHT,
            (main.width, main.height),
            (reaction.width, reaction.height),
        )?;
        let output = compositor.compose(&main, &reaction, plan)?;
        let pts = (local_frame as i128 * 10_000_000 / FRAME_RATE as i128) as i64;
        let queued = encoder
            .encode_detached_frame(output, pts)
            .map_err(|error| format!("Watch Party encoder rejected a frame: {error}"))?;
        if !queued.telemetry.queued {
            return Err(
                "Watch Party encoder queue saturated; finalized material was preserved."
                    .to_string(),
            );
        }
        let generations = (main.generation, reaction.generation);
        let fresh = last_generations != Some(generations);
        last_generations = Some(generations);
        let output_qpc = video_start_qpc
            .saturating_add((*global_frame as i128 * 10_000_000 / FRAME_RATE as i128) as i64);
        timing.push(VideoFrameTimingPoint {
            frame_index: local_frame,
            output_qpc_100ns: output_qpc,
            source_qpc_100ns: main.captured_qpc_100ns.max(reaction.captured_qpc_100ns),
            encoded_pts_100ns: pts,
            fresh_source: fresh,
        });
        *global_frame += 1;
        let mut status = lock(&shared.status);
        status.frames_composed = *global_frame;
        status.elapsed_seconds = session_started.elapsed().as_secs_f64();
    }
    let frame_count = timing.len() as u64;
    let finish_started = Instant::now();
    encoder
        .finish()
        .map_err(|error| format!("Could not finalize Watch Party segment: {error}"))?;
    let finalization_ms = finish_started.elapsed().as_secs_f64() * 1_000.0;
    let file_size = fs::metadata(&path)
        .map_err(|error| format!("Could not inspect Watch Party segment: {error}"))?
        .len();
    let duration = (frame_count as i128 * 10_000_000 / FRAME_RATE as i128) as i64;
    let first = timing
        .first()
        .map(|point| point.source_qpc_100ns)
        .unwrap_or(video_start_qpc);
    let last = timing
        .last()
        .map(|point| point.source_qpc_100ns)
        .unwrap_or(first);
    let fresh = timing.iter().filter(|point| point.fresh_source).count() as u64;
    let segment_start_qpc = video_start_qpc
        .saturating_add((segment_global_start as i128 * 10_000_000 / FRAME_RATE as i128) as i64);
    Ok(CompletedSegment {
        sequence_number: sequence,
        file_path: path.to_string_lossy().into_owned(),
        start_timestamp_ms: segment_start_ms,
        end_timestamp_ms: now_ms(),
        actual_duration_ms: (duration / 10_000).max(0) as u64,
        segment_session_start_qpc_100ns: segment_start_qpc,
        segment_session_end_qpc_100ns: segment_start_qpc.saturating_add(duration),
        first_frame_timestamp_100ns: first,
        last_frame_timestamp_100ns: last,
        encoded_start_pts_100ns: 0,
        encoded_last_frame_pts_100ns: timing
            .last()
            .map(|point| point.encoded_pts_100ns)
            .unwrap_or(0),
        encoded_end_pts_100ns: duration,
        encoded_duration_100ns: duration,
        encoded_time_base_numerator: 1,
        encoded_time_base_denominator: 10_000_000,
        frame_timing_points: timing,
        next_segment_first_frame_timestamp_100ns: None,
        source_frame_gap_ms: None,
        source_update_count: fresh,
        fresh_output_frame_count: fresh,
        held_output_frame_count: frame_count.saturating_sub(fresh),
        frame_count,
        encoder_creation_time_ms: encoder_creation_ms,
        encoder_creation_started_ms: 0.0,
        encoder_creation_completed_ms: encoder_creation_ms,
        rotation_requested_ms: None,
        first_frame_submitted_ms: Some(0.0),
        last_frame_submitted_ms: (frame_count > 0).then_some(duration as f64 / 10_000.0),
        next_first_frame_submitted_ms: None,
        codec: "H.264".to_string(),
        width: CANVAS_WIDTH,
        height: CANVAS_HEIGHT,
        frame_rate: FRAME_RATE,
        file_size,
        average_bitrate_mbps: if duration > 0 {
            file_size as f64 * 8.0 * 10_000_000.0 / duration as f64 / 1_000_000.0
        } else {
            0.0
        },
        finalized: frame_count > 0 && file_size > 0,
        finalization_time_ms: finalization_ms,
        rotation_gap_ms: None,
    })
}

fn wait_for_initial_sources(shared: &SharedRecording) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if shared.stop.load(Ordering::Acquire) {
            return Err("Watch Party was stopped during source startup.".to_string());
        }
        if lock(&shared.main).frame.is_some() && lock(&shared.reaction).frame.is_some() {
            return Ok(());
        }
        thread::sleep(Duration::from_millis(20));
    }
    Err(
        "Watch Party did not receive initial frames from both sources within 10 seconds."
            .to_string(),
    )
}

fn validate_required_audio(
    audio: &AudioReplayConfiguration,
    main_process_id: Option<u32>,
    discord_process_id: u32,
) -> Result<(), String> {
    use crate::replay::{AudioSourceKind, AudioTrackRole};

    let enabled = |role| {
        audio
            .tracks
            .iter()
            .find(|track| track.enabled && track.role == role)
    };
    let main = enabled(AudioTrackRole::Game)
        .ok_or_else(|| "Watch Party requires an enabled Main Content audio track.".to_string())?;
    let main_matches_window = main_process_id.is_none_or(|window_pid| {
        main.process_id == Some(window_pid)
            || main.process_id.is_some_and(|audio_pid| {
                process_family(audio_pid)
                    .zip(process_family(window_pid))
                    .is_some_and(|(audio, window)| audio == window)
            })
    });
    if main.source_kind != AudioSourceKind::Process
        || main.process_id.is_none()
        || !main_matches_window
    {
        return Err(
            "Main Content audio must match the selected content window application family (or be an explicitly selected process for display capture)."
                .to_string(),
        );
    }
    let voice = enabled(AudioTrackRole::VoiceChat).ok_or_else(|| {
        "Watch Party requires an enabled Discord / Voice Chat audio track.".to_string()
    })?;
    let voice_is_discord = voice.process_id.is_some_and(|pid| {
        pid == discord_process_id
            || crate::audio::resolve_process_metadata(pid).is_some_and(|metadata| {
                metadata
                    .process_name
                    .trim_end_matches(".exe")
                    .to_ascii_lowercase()
                    .starts_with("discord")
            })
    });
    if voice.source_kind != AudioSourceKind::Process || !voice_is_discord {
        return Err(
            "Voice Chat audio must use a Discord desktop process from the active audio list."
                .to_string(),
        );
    }
    let microphone = enabled(AudioTrackRole::Microphone)
        .ok_or_else(|| "Watch Party requires an enabled Microphone audio track.".to_string())?;
    if microphone.source_kind != AudioSourceKind::Microphone {
        return Err("The Watch Party Microphone track must use a microphone endpoint.".to_string());
    }
    if audio.tracks.iter().filter(|track| track.enabled).count() != 3 {
        return Err(
            "Watch Party accepts exactly Main Content, Voice Chat, and Microphone audio tracks."
                .to_string(),
        );
    }
    Ok(())
}

fn process_family(process_id: u32) -> Option<String> {
    crate::audio::resolve_process_metadata(process_id).map(|metadata| {
        metadata
            .process_name
            .trim_end_matches(".exe")
            .to_ascii_lowercase()
    })
}

fn start_source_worker(
    role: &'static str,
    target: NativeCaptureTarget,
    source: Arc<Mutex<LatestSource>>,
    shared: Arc<SharedRecording>,
) -> Result<JoinHandle<()>, String> {
    thread::Builder::new()
        .name(format!("watch-party-{}", role.replace(' ', "-")))
        .spawn(move || {
            let result = match target {
                NativeCaptureTarget::Monitor(monitor) => {
                    capture_source(role, monitor, source.clone(), &shared)
                }
                NativeCaptureTarget::Window(window) => {
                    capture_source(role, window, source.clone(), &shared)
                }
            };
            if let Err(error) = result {
                let mut source = lock(&source);
                source.status.closed = true;
                source.status.error_message = Some(error);
            }
        })
        .map_err(|error| format!("Could not start {role} capture worker: {error}"))
}

fn capture_source<T>(
    role: &'static str,
    target: T,
    source: Arc<Mutex<LatestSource>>,
    shared: &SharedRecording,
) -> Result<(), String>
where
    T: TryInto<GraphicsCaptureItemType> + Send + 'static,
{
    let settings = Settings::new(
        target,
        CursorCaptureSettings::Default,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        SourceCaptureFlags { role, source },
    )
    .frame_pool_buffer_count(crate::capture::WGC_FRAME_POOL_BUFFER_COUNT);
    let control = SourceCaptureHandler::start_free_threaded(settings).map_err(map_capture_error)?;
    loop {
        if control.is_finished() {
            return control
                .wait()
                .map_err(|error| format!("{role} capture ended: {error}"));
        }
        if shared.stop.load(Ordering::Acquire) {
            return control
                .stop()
                .map_err(|error| format!("{role} capture could not stop cleanly: {error}"));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[derive(Clone)]
struct SourceCaptureFlags {
    role: &'static str,
    source: Arc<Mutex<LatestSource>>,
}

struct SourceCaptureHandler {
    flags: SourceCaptureFlags,
    padding: Vec<u8>,
}

#[derive(Debug)]
struct SourceCaptureError(String);

impl fmt::Display for SourceCaptureError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for SourceCaptureError {}

impl GraphicsCaptureApiHandler for SourceCaptureHandler {
    type Flags = SourceCaptureFlags;
    type Error = SourceCaptureError;

    fn new(ctx: windows_capture::capture::Context<Self::Flags>) -> Result<Self, Self::Error> {
        Ok(Self {
            flags: ctx.flags,
            padding: Vec::new(),
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        let captured_qpc = frame
            .timestamp()
            .map_err(|error| {
                SourceCaptureError(format!(
                    "Could not timestamp {} frame: {error}",
                    self.flags.role
                ))
            })?
            .Duration;
        let width = frame.width();
        let height = frame.height();
        let buffer = frame.buffer().map_err(|error| {
            SourceCaptureError(format!("Could not read {} frame: {error}", self.flags.role))
        })?;
        let pixels = buffer.as_nopadding_buffer(&mut self.padding).to_vec();
        let mut source = lock(&self.flags.source);
        source.generation += 1;
        source.status.width = width;
        source.status.height = height;
        source.status.frames_received += 1;
        source.status.closed = false;
        source.status.error_message = None;
        source.frame = Some(CpuFrame {
            pixels,
            width,
            height,
            captured_qpc_100ns: captured_qpc,
            generation: source.generation,
        });
        Ok(())
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        mark_source_closed(&self.flags.source, self.flags.role);
        Ok(())
    }
}

fn mark_source_closed(source: &Mutex<LatestSource>, role: &str) {
    let mut source = lock(source);
    source.status.closed = true;
    source.status.error_message = Some(format!(
        "{role} source closed. Recording is preserving the last valid frame; stop to finalize safely."
    ));
}

fn map_capture_error(error: GraphicsCaptureApiError<SourceCaptureError>) -> String {
    match error {
        GraphicsCaptureApiError::NewHandlerError(error)
        | GraphicsCaptureApiError::FrameHandlerError(error) => error.to_string(),
        error => format!("Watch Party source capture failed: {error}"),
    }
}

fn current_source_error(shared: &SharedRecording) -> Option<String> {
    lock(&shared.main)
        .status
        .error_message
        .clone()
        .or_else(|| lock(&shared.reaction).status.error_message.clone())
}

fn ensure_free_space(path: &Path) -> Result<(), String> {
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect::<Vec<_>>();
    let mut available = 0u64;
    unsafe { GetDiskFreeSpaceExW(PCWSTR(wide.as_ptr()), Some(&mut available), None, None) }
        .map_err(|error| format!("Could not check Watch Party disk capacity: {error}"))?;
    if available < MINIMUM_FREE_BYTES {
        return Err(format!(
            "Watch Party stopped safely because less than 2 GiB remains ({} bytes available).",
            available
        ));
    }
    Ok(())
}

fn success(status: WatchPartyStatus) -> WatchPartyCommandResult {
    WatchPartyCommandResult {
        success: true,
        status,
        error_message: None,
    }
}

fn failure(mut status: WatchPartyStatus, error: impl Into<String>) -> WatchPartyCommandResult {
    let error = error.into();
    status.error_message = Some(error.clone());
    WatchPartyCommandResult {
        success: false,
        status,
        error_message: Some(error),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_marks_only_work_states_active() {
        assert!(WatchPartyState::Starting.active());
        assert!(WatchPartyState::Recording.active());
        assert!(!WatchPartyState::Completed.active());
        assert!(!WatchPartyState::Error.active());
    }

    #[test]
    fn segment_constants_are_exact_and_cfr_compatible() {
        assert_eq!(SEGMENT_FRAMES, 900);
        assert_eq!(SEGMENT_FRAMES * 10_000_000 / FRAME_RATE as u64, 300_000_000);
    }

    #[test]
    fn required_audio_rejects_a_voice_process_that_is_not_discord() {
        use crate::replay::{AudioSourceKind, AudioTrackConfiguration, AudioTrackRole};
        let audio = AudioReplayConfiguration {
            tracks: vec![
                AudioTrackConfiguration {
                    role: AudioTrackRole::Game,
                    enabled: true,
                    source_kind: AudioSourceKind::Process,
                    process_id: Some(10),
                    endpoint_id: None,
                    source_label: None,
                },
                AudioTrackConfiguration {
                    role: AudioTrackRole::VoiceChat,
                    enabled: true,
                    source_kind: AudioSourceKind::Process,
                    process_id: Some(99),
                    endpoint_id: None,
                    source_label: None,
                },
                AudioTrackConfiguration {
                    role: AudioTrackRole::Microphone,
                    enabled: true,
                    source_kind: AudioSourceKind::Microphone,
                    process_id: None,
                    endpoint_id: Some("mic".to_string()),
                    source_label: None,
                },
            ],
        };
        assert!(validate_required_audio(&audio, Some(10), 20).is_err());
    }

    #[test]
    fn source_close_preserves_the_last_frame_and_surfaces_recovery_guidance() {
        let mut source = LatestSource::new("Discord".to_string(), 1280, 720);
        source.frame = Some(CpuFrame {
            pixels: vec![0; 1280 * 720 * 4],
            width: 1280,
            height: 720,
            captured_qpc_100ns: 10,
            generation: 1,
        });
        let source = Mutex::new(source);
        mark_source_closed(&source, "Discord reaction");
        let source = lock(&source);
        assert!(source.frame.is_some());
        assert!(source.status.closed);
        assert!(source
            .status
            .error_message
            .as_deref()
            .is_some_and(|message| message.contains("last valid frame")));
    }

    #[test]
    fn completed_session_cleanup_is_scoped_to_one_owned_direct_child() {
        let root = std::env::temp_dir().join(format!(
            "slickclip-watch-cleanup-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let owned = root.join("watch-party-test");
        let outside = root.with_file_name("watch-party-outside");
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&outside);
        fs::create_dir_all(&owned).unwrap();
        fs::create_dir_all(&outside).unwrap();
        fs::write(owned.join("segment.mp4"), b"temporary").unwrap();
        assert!(cleanup_completed_session(&root, &outside).is_err());
        assert!(outside.exists());
        cleanup_completed_session(&root, &owned).unwrap();
        assert!(!owned.exists());
        let _ = fs::remove_dir_all(root);
        let _ = fs::remove_dir_all(outside);
    }
}
