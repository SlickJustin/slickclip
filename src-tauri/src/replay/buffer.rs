use std::error::Error;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Deserialize;
use windows_capture::capture::{Context, GraphicsCaptureApiError, GraphicsCaptureApiHandler};
use windows_capture::frame::Frame;
use windows_capture::graphics_capture_api::InternalCaptureControl;
use windows_capture::settings::{
    ColorFormat, CursorCaptureSettings, DirtyRegionSettings, DrawBorderSettings,
    GraphicsCaptureItemType, MinimumUpdateIntervalSettings, SecondaryWindowSettings, Settings,
};

use crate::capture::capture_test::ensure_borderless_capture_access;
use crate::capture::encoder::{
    resolve_encoder, EncoderChoice, EncoderCodec, VideoEncoderBackend, WindowsCaptureFileBackend,
};
use crate::capture::targets::{
    resolve_target, CaptureTargetRequest, NativeCaptureTarget, ResolvedCaptureTarget,
};

use super::segment::{CompletedSegment, SegmentRing};
use super::state::{ReplayBufferStatus, ReplayCommandResult, ReplayLifecycleState};

pub const SEGMENT_DURATION: Duration = Duration::from_secs(2);
const RECENT_SEGMENT_LIMIT: usize = 5;
const ALLOWED_REPLAY_DURATIONS: [u32; 5] = [30, 60, 120, 180, 300];
static SESSION_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplayBufferStartRequest {
    pub target: CaptureTargetRequest,
    pub encoder: EncoderChoice,
    pub replay_duration_seconds: u32,
    pub frame_rate: u32,
}

struct ReplayInner {
    state: ReplayLifecycleState,
    error_message: Option<String>,
    target_id: Option<String>,
    target_label: Option<String>,
    requested_encoder: Option<String>,
    actual_encoder: Option<String>,
    replay_duration_seconds: u32,
    frame_rate: u32,
    width: u32,
    height: u32,
    session_id: Option<String>,
    session_directory: Option<PathBuf>,
    ring: SegmentRing,
    pending_finalizations: usize,
    dropped_segments: u64,
    last_segment_duration_ms: Option<u64>,
    last_rotation_gap_ms: Option<f64>,
    last_finalize_time_ms: Option<f64>,
}

impl ReplayInner {
    fn stopped() -> Self {
        Self {
            state: ReplayLifecycleState::Stopped,
            error_message: None,
            target_id: None,
            target_label: None,
            requested_encoder: None,
            actual_encoder: None,
            replay_duration_seconds: 0,
            frame_rate: 0,
            width: 0,
            height: 0,
            session_id: None,
            session_directory: None,
            ring: SegmentRing::new(0),
            pending_finalizations: 0,
            dropped_segments: 0,
            last_segment_duration_ms: None,
            last_rotation_gap_ms: None,
            last_finalize_time_ms: None,
        }
    }

    fn snapshot(&self) -> ReplayBufferStatus {
        ReplayBufferStatus {
            state: self.state,
            error_message: self.error_message.clone(),
            target_id: self.target_id.clone(),
            target_label: self.target_label.clone(),
            requested_encoder: self.requested_encoder.clone(),
            actual_encoder: self.actual_encoder.clone(),
            replay_duration_seconds: self.replay_duration_seconds,
            expected_segment_duration_seconds: SEGMENT_DURATION.as_secs_f64(),
            frame_rate: self.frame_rate,
            width: self.width,
            height: self.height,
            session_id: self.session_id.clone(),
            session_directory: self
                .session_directory
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            completed_segment_count: self.ring.len(),
            retained_duration_seconds: self.ring.total_duration_ms() as f64 / 1_000.0,
            retained_bytes: self.ring.total_bytes(),
            pending_finalizations: self.pending_finalizations,
            dropped_segments: self.dropped_segments,
            last_segment_duration_seconds: self
                .last_segment_duration_ms
                .map(|duration| duration as f64 / 1_000.0),
            last_rotation_gap_ms: self.last_rotation_gap_ms,
            last_finalize_time_ms: self.last_finalize_time_ms,
            recent_segments: self.ring.recent(RECENT_SEGMENT_LIMIT),
        }
    }
}

struct SharedReplay {
    inner: Mutex<ReplayInner>,
    stop_requested: AtomicBool,
}

impl SharedReplay {
    fn new() -> Self {
        Self {
            inner: Mutex::new(ReplayInner::stopped()),
            stop_requested: AtomicBool::new(false),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ReplayInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn snapshot(&self) -> ReplayBufferStatus {
        self.lock().snapshot()
    }

    fn begin(&self, request: &ReplayBufferStartRequest) {
        self.stop_requested.store(false, Ordering::Release);
        let mut inner = self.lock();
        *inner = ReplayInner {
            state: ReplayLifecycleState::Starting,
            error_message: None,
            target_id: Some(request.target.id.clone()),
            target_label: None,
            requested_encoder: Some(request.encoder.result_name().to_string()),
            actual_encoder: None,
            replay_duration_seconds: request.replay_duration_seconds,
            frame_rate: request.frame_rate,
            width: 0,
            height: 0,
            session_id: None,
            session_directory: None,
            ring: SegmentRing::new(request.replay_duration_seconds),
            pending_finalizations: 0,
            dropped_segments: 0,
            last_segment_duration_ms: None,
            last_rotation_gap_ms: None,
            last_finalize_time_ms: None,
        };
    }

    fn configure(
        &self,
        target_label: String,
        actual_encoder: EncoderCodec,
        width: u32,
        height: u32,
        session_id: String,
        session_directory: PathBuf,
    ) {
        let mut inner = self.lock();
        inner.target_label = Some(target_label);
        inner.actual_encoder = Some(actual_encoder.display_name().to_string());
        inner.width = width;
        inner.height = height;
        inner.session_id = Some(session_id);
        inner.session_directory = Some(session_directory);
    }

    fn mark_running(&self) {
        let mut inner = self.lock();
        if inner.state != ReplayLifecycleState::Error {
            inner.state = ReplayLifecycleState::Running;
        }
    }

    fn request_stop(&self) {
        self.stop_requested.store(true, Ordering::Release);
        let mut inner = self.lock();
        if matches!(
            inner.state,
            ReplayLifecycleState::Starting | ReplayLifecycleState::Running
        ) {
            inner.state = ReplayLifecycleState::Stopping;
        }
    }

    fn should_stop(&self) -> bool {
        self.stop_requested.load(Ordering::Acquire)
    }

    fn mark_stopped(&self) {
        let mut inner = self.lock();
        if inner.state != ReplayLifecycleState::Error {
            inner.state = ReplayLifecycleState::Stopped;
            inner.error_message = None;
        }
    }

    fn mark_error(&self, error: impl Into<String>) {
        self.stop_requested.store(true, Ordering::Release);
        let mut inner = self.lock();
        inner.state = ReplayLifecycleState::Error;
        inner.error_message = Some(error.into());
    }

    fn record_rotation_gap(&self, gap_ms: f64) {
        self.lock().last_rotation_gap_ms = Some(gap_ms);
    }

    fn segment_submitted(&self) {
        self.lock().pending_finalizations += 1;
    }

    fn complete_segment(&self, segment: CompletedSegment) {
        let (evicted, session_directory) = {
            let mut inner = self.lock();
            inner.pending_finalizations = inner.pending_finalizations.saturating_sub(1);
            inner.last_segment_duration_ms = Some(segment.actual_duration_ms);
            inner.last_finalize_time_ms = Some(segment.finalization_time_ms);
            let evicted = inner.ring.push(segment);
            (evicted, inner.session_directory.clone())
        };

        for path in evicted {
            if !path_is_inside_session(&path, session_directory.as_deref()) {
                self.mark_error(format!(
                    "Replay retention refused to delete a segment outside the active session: '{}'",
                    path.display()
                ));
                return;
            }
            if let Err(error) = fs::remove_file(&path) {
                self.mark_error(format!(
                    "Could not evict expired replay segment '{}': {error}",
                    path.display()
                ));
                return;
            }
        }
    }

    fn fail_segment(&self, path: &Path, error: impl Into<String>) {
        {
            let mut inner = self.lock();
            inner.pending_finalizations = inner.pending_finalizations.saturating_sub(1);
            inner.dropped_segments += 1;
        }
        let _ = fs::remove_file(path);
        self.mark_error(error);
    }

    fn discard_empty_segment(&self, path: &Path) {
        {
            let mut inner = self.lock();
            inner.pending_finalizations = inner.pending_finalizations.saturating_sub(1);
            inner.dropped_segments += 1;
        }
        let _ = fs::remove_file(path);
    }
}

#[derive(Clone)]
pub struct ReplayBufferManager {
    shared: Arc<SharedReplay>,
    worker: Arc<Mutex<Option<JoinHandle<()>>>>,
    root: Arc<PathBuf>,
}

impl ReplayBufferManager {
    pub fn new(root: PathBuf) -> Result<Self, String> {
        fs::create_dir_all(&root).map_err(|error| {
            format!(
                "Could not create the replay-buffer root '{}': {error}",
                root.display()
            )
        })?;
        cleanup_session_directories(&root)?;

        Ok(Self {
            shared: Arc::new(SharedReplay::new()),
            worker: Arc::new(Mutex::new(None)),
            root: Arc::new(root),
        })
    }

    pub fn status(&self) -> ReplayBufferStatus {
        self.shared.snapshot()
    }

    pub fn start(&self, request: ReplayBufferStartRequest) -> ReplayCommandResult {
        if let Err(error) = validate_start_request(&request) {
            return ReplayCommandResult::failure(self.status(), error);
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
        if !current.state.can_start() || worker.is_some() {
            return ReplayCommandResult::failure(
                current,
                "A replay-buffer session is already starting, running, or stopping.",
            );
        }

        if let Err(error) = cleanup_session_directories(&self.root) {
            return ReplayCommandResult::failure(self.status(), error);
        }

        self.shared.begin(&request);
        let shared = Arc::clone(&self.shared);
        let root = Arc::clone(&self.root);
        let thread = match thread::Builder::new()
            .name("justin-replay-buffer".to_string())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_replay_session(Arc::clone(&shared), root.as_ref(), request)
                }));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        if shared.snapshot().state != ReplayLifecycleState::Error {
                            shared.mark_error(error);
                        }
                    }
                    Err(_) => shared.mark_error("The replay-buffer capture worker panicked."),
                }
            }) {
            Ok(thread) => thread,
            Err(error) => {
                self.shared.mark_error(format!(
                    "Could not start the replay-buffer capture thread: {error}"
                ));
                return ReplayCommandResult::failure(self.status(), error.to_string());
            }
        };
        *worker = Some(thread);

        ReplayCommandResult::success(self.status())
    }

    pub fn stop_and_wait(&self) -> ReplayCommandResult {
        let status = self.status();
        if !status.state.is_active() {
            return if status.state == ReplayLifecycleState::Stopped {
                ReplayCommandResult::success(status)
            } else {
                ReplayCommandResult::failure(status, "The replay buffer is not currently running.")
            };
        }

        self.shared.request_stop();
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            if worker.join().is_err() {
                self.shared
                    .mark_error("The replay-buffer capture thread panicked while stopping.");
            }
        }

        let status = self.status();
        if status.state == ReplayLifecycleState::Stopped {
            ReplayCommandResult::success(status)
        } else {
            ReplayCommandResult::failure(
                status.clone(),
                status
                    .error_message
                    .unwrap_or_else(|| "The replay buffer did not stop cleanly.".to_string()),
            )
        }
    }

    pub fn shutdown_and_cleanup(&self) {
        self.shared.request_stop();
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            if worker.join().is_err() {
                self.shared
                    .mark_error("The replay-buffer capture thread panicked during app shutdown.");
            }
        }
        if let Err(error) = cleanup_session_directories(&self.root) {
            self.shared.mark_error(error);
        }
    }
}

fn validate_start_request(request: &ReplayBufferStartRequest) -> Result<(), String> {
    if !ALLOWED_REPLAY_DURATIONS.contains(&request.replay_duration_seconds) {
        return Err(format!(
            "Replay duration must be one of 30, 60, 120, 180, or 300 seconds; received {}.",
            request.replay_duration_seconds
        ));
    }
    if !matches!(request.frame_rate, 30 | 60) {
        return Err(format!(
            "Replay frame rate must be 30 or 60 FPS; received {}.",
            request.frame_rate
        ));
    }
    if matches!(request.encoder, EncoderChoice::Av1) {
        return Err(
            "AV1 production encoding is not available through windows-capture 2.0.1. Choose Automatic, HEVC, or H.264."
                .to_string(),
        );
    }

    Ok(())
}

fn run_replay_session(
    shared: Arc<SharedReplay>,
    root: &Path,
    request: ReplayBufferStartRequest,
) -> Result<(), String> {
    let resolved_encoder = resolve_encoder(request.encoder)?;
    let ResolvedCaptureTarget {
        target,
        label,
        width,
        height,
    } = resolve_target(&request.target)?;
    let width = even_dimension(width)?;
    let height = even_dimension(height)?;
    let session_id = create_session_id();
    let session_directory = root.join(&session_id);
    fs::create_dir(&session_directory).map_err(|error| {
        format!(
            "Could not create replay session directory '{}': {error}",
            session_directory.display()
        )
    })?;

    shared.configure(
        label,
        resolved_encoder.actual,
        width,
        height,
        session_id,
        session_directory.clone(),
    );

    let flags = ReplayCaptureFlags {
        shared: Arc::clone(&shared),
        session_directory,
        codec: resolved_encoder.actual,
        width,
        height,
        frame_rate: request.frame_rate,
    };
    let capture_result = match target {
        NativeCaptureTarget::Monitor(monitor) => start_target_capture(monitor, flags),
        NativeCaptureTarget::Window(window) => start_target_capture(window, flags),
    };

    match capture_result {
        Ok(()) => {
            shared.mark_stopped();
            Ok(())
        }
        Err(error) => Err(error),
    }
}

struct ReplayCaptureFlags {
    shared: Arc<SharedReplay>,
    session_directory: PathBuf,
    codec: EncoderCodec,
    width: u32,
    height: u32,
    frame_rate: u32,
}

struct ActiveSegment {
    sequence_number: u64,
    path: PathBuf,
    backend: Box<dyn VideoEncoderBackend>,
    first_frame_instant: Option<Instant>,
    first_frame_timestamp: Option<i64>,
    last_frame_timestamp: Option<i64>,
    start_timestamp_ms: Option<u64>,
    frame_count: u64,
}

impl ActiveSegment {
    fn create(flags: &ReplayCaptureFlags, sequence_number: u64) -> Result<Self, String> {
        let path = flags
            .session_directory
            .join(format!("segment-{sequence_number:06}.mp4"));
        let backend = WindowsCaptureFileBackend::create(
            &path,
            flags.codec,
            flags.width,
            flags.height,
            flags.frame_rate,
        )
        .map_err(|error| format!("Could not initialize replay segment encoder: {error}"))?;

        Ok(Self {
            sequence_number,
            path,
            backend,
            first_frame_instant: None,
            first_frame_timestamp: None,
            last_frame_timestamp: None,
            start_timestamp_ms: None,
            frame_count: 0,
        })
    }

    fn should_rotate(&self, now: Instant) -> bool {
        self.first_frame_instant
            .is_some_and(|started| now.duration_since(started) >= SEGMENT_DURATION)
    }

    fn encode_frame(&mut self, frame: &mut Frame<'_>) -> Result<(), String> {
        let timestamp = frame
            .timestamp()
            .map_err(|error| format!("Could not read replay frame timestamp: {error}"))?
            .Duration;
        self.backend
            .encode_frame(frame)
            .map_err(|error| format!("Replay encoder rejected a captured frame: {error}"))?;

        if self.first_frame_instant.is_none() {
            self.first_frame_instant = Some(Instant::now());
            self.first_frame_timestamp = Some(timestamp);
            self.start_timestamp_ms = Some(unix_timestamp_ms());
        }
        self.last_frame_timestamp = Some(timestamp);
        self.frame_count += 1;
        Ok(())
    }

    fn into_finalize_job(
        self,
        flags: &ReplayCaptureFlags,
        rotation_gap_ms: Option<f64>,
    ) -> FinalizeJob {
        let frame_duration_100ns = 10_000_000 / i64::from(flags.frame_rate);
        let actual_duration_100ns = match (self.first_frame_timestamp, self.last_frame_timestamp) {
            (Some(first), Some(last)) => last.saturating_sub(first) + frame_duration_100ns,
            _ => 0,
        };
        let actual_duration_ms = u64::try_from(actual_duration_100ns.max(0) / 10_000)
            .unwrap_or(0)
            .max(1);
        let start_timestamp_ms = self.start_timestamp_ms.unwrap_or_else(unix_timestamp_ms);

        FinalizeJob {
            backend: self.backend,
            path: self.path,
            sequence_number: self.sequence_number,
            start_timestamp_ms,
            end_timestamp_ms: unix_timestamp_ms(),
            actual_duration_ms,
            codec: flags.codec,
            width: flags.width,
            height: flags.height,
            rotation_gap_ms,
            frame_count: self.frame_count,
        }
    }
}

struct FinalizeJob {
    backend: Box<dyn VideoEncoderBackend>,
    path: PathBuf,
    sequence_number: u64,
    start_timestamp_ms: u64,
    end_timestamp_ms: u64,
    actual_duration_ms: u64,
    codec: EncoderCodec,
    width: u32,
    height: u32,
    rotation_gap_ms: Option<f64>,
    frame_count: u64,
}

struct FinalizerWorker {
    sender: Option<mpsc::Sender<FinalizeJob>>,
    thread: Option<JoinHandle<()>>,
    shared: Arc<SharedReplay>,
}

impl FinalizerWorker {
    fn new(shared: Arc<SharedReplay>) -> Result<Self, String> {
        let (sender, receiver) = mpsc::channel::<FinalizeJob>();
        let worker_shared = Arc::clone(&shared);
        let thread = thread::Builder::new()
            .name("justin-replay-finalizer".to_string())
            .spawn(move || finalize_segments(receiver, worker_shared))
            .map_err(|error| format!("Could not start the replay segment finalizer: {error}"))?;

        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
            shared,
        })
    }

    fn submit(&mut self, job: FinalizeJob) -> Result<(), String> {
        let sender = self
            .sender
            .as_ref()
            .ok_or_else(|| "The replay segment finalizer is already closed.".to_string())?;
        self.shared.segment_submitted();
        if let Err(error) = sender.send(job) {
            let path = error.0.path.clone();
            let message = format!("Could not queue replay segment finalization: {error}");
            self.shared.fail_segment(&path, message.clone());
            return Err(message);
        }

        Ok(())
    }

    fn shutdown(mut self) -> Result<(), String> {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            thread
                .join()
                .map_err(|_| "The replay segment finalizer panicked.".to_string())?;
        }
        Ok(())
    }
}

impl Drop for FinalizerWorker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn finalize_segments(receiver: mpsc::Receiver<FinalizeJob>, shared: Arc<SharedReplay>) {
    for job in receiver {
        let finalization_started = Instant::now();
        let path = job.path.clone();
        let finish_result =
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| job.backend.finish()));
        let finish_result = match finish_result {
            Ok(result) => result.map_err(|error| error.to_string()),
            Err(_) => Err("The windows-capture encoder panicked while finalizing.".to_string()),
        };

        if job.frame_count == 0 {
            shared.discard_empty_segment(&path);
            continue;
        }
        if let Err(error) = finish_result {
            shared.fail_segment(
                &path,
                format!(
                    "Replay segment {} could not be finalized: {error}",
                    job.sequence_number
                ),
            );
            continue;
        }
        let file_size = match fs::metadata(&path) {
            Ok(metadata) if metadata.len() > 0 => metadata.len(),
            Ok(_) => {
                shared.fail_segment(
                    &path,
                    format!(
                        "Replay segment {} finalized as an empty file.",
                        job.sequence_number
                    ),
                );
                continue;
            }
            Err(error) => {
                shared.fail_segment(
                    &path,
                    format!(
                        "Replay segment {} could not be verified: {error}",
                        job.sequence_number
                    ),
                );
                continue;
            }
        };

        shared.complete_segment(CompletedSegment {
            sequence_number: job.sequence_number,
            file_path: path.to_string_lossy().into_owned(),
            start_timestamp_ms: job.start_timestamp_ms,
            end_timestamp_ms: job.end_timestamp_ms,
            actual_duration_ms: job.actual_duration_ms,
            codec: job.codec.display_name().to_string(),
            width: job.width,
            height: job.height,
            file_size,
            finalized: true,
            finalization_time_ms: finalization_started.elapsed().as_secs_f64() * 1_000.0,
            rotation_gap_ms: job.rotation_gap_ms,
        });
    }
}

struct ReplayCaptureHandler {
    flags: ReplayCaptureFlags,
    active: Option<ActiveSegment>,
    finalizer: Option<FinalizerWorker>,
    next_sequence: u64,
    finished: bool,
}

impl ReplayCaptureHandler {
    fn rotate(&mut self) -> Result<(), ReplayHandlerError> {
        let rotation_started = Instant::now();
        let next = ActiveSegment::create(&self.flags, self.next_sequence)
            .map_err(ReplayHandlerError::new)?;
        let rotation_gap_ms = rotation_started.elapsed().as_secs_f64() * 1_000.0;
        self.flags.shared.record_rotation_gap(rotation_gap_ms);
        self.next_sequence += 1;

        let previous = self
            .active
            .replace(next)
            .ok_or_else(|| ReplayHandlerError::new("The active replay segment is missing."))?;
        let job = previous.into_finalize_job(&self.flags, Some(rotation_gap_ms));
        self.finalizer
            .as_mut()
            .ok_or_else(|| ReplayHandlerError::new("The replay finalizer is unavailable."))?
            .submit(job)
            .map_err(ReplayHandlerError::new)
    }

    fn finish_session(&mut self) -> Result<(), ReplayHandlerError> {
        if self.finished {
            return Ok(());
        }
        self.finished = true;

        if let Some(active) = self.active.take() {
            let job = active.into_finalize_job(&self.flags, None);
            self.finalizer
                .as_mut()
                .ok_or_else(|| ReplayHandlerError::new("The replay finalizer is unavailable."))?
                .submit(job)
                .map_err(ReplayHandlerError::new)?;
        }
        if let Some(finalizer) = self.finalizer.take() {
            finalizer.shutdown().map_err(ReplayHandlerError::new)?;
        }

        Ok(())
    }
}

impl Drop for ReplayCaptureHandler {
    fn drop(&mut self) {
        if let Err(error) = self.finish_session() {
            self.flags.shared.mark_error(format!(
                "The active replay segment could not be finalized during capture shutdown: {error}"
            ));
        }
    }
}

#[derive(Debug)]
struct ReplayHandlerError(String);

impl ReplayHandlerError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for ReplayHandlerError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl Error for ReplayHandlerError {}

impl GraphicsCaptureApiHandler for ReplayCaptureHandler {
    type Flags = ReplayCaptureFlags;
    type Error = ReplayHandlerError;

    fn new(ctx: Context<Self::Flags>) -> Result<Self, Self::Error> {
        ensure_borderless_capture_access().map_err(ReplayHandlerError::new)?;
        let finalizer =
            FinalizerWorker::new(Arc::clone(&ctx.flags.shared)).map_err(ReplayHandlerError::new)?;
        let active = ActiveSegment::create(&ctx.flags, 1).map_err(ReplayHandlerError::new)?;
        ctx.flags.shared.mark_running();

        Ok(Self {
            flags: ctx.flags,
            active: Some(active),
            finalizer: Some(finalizer),
            next_sequence: 2,
            finished: false,
        })
    }

    fn on_frame_arrived(
        &mut self,
        frame: &mut Frame,
        _capture_control: InternalCaptureControl,
    ) -> Result<(), Self::Error> {
        if self
            .active
            .as_ref()
            .is_some_and(|active| active.should_rotate(Instant::now()))
        {
            self.rotate()?;
        }

        self.active
            .as_mut()
            .ok_or_else(|| ReplayHandlerError::new("The active replay segment is missing."))?
            .encode_frame(frame)
            .map_err(ReplayHandlerError::new)
    }

    fn on_closed(&mut self) -> Result<(), Self::Error> {
        let finalize_result = self.finish_session();
        let message = match finalize_result {
            Ok(()) => {
                "The selected capture target closed. The final replay segment was finalized safely."
                    .to_string()
            }
            Err(error) => format!(
                "The selected capture target closed, and the final replay segment could not be finalized: {error}"
            ),
        };
        self.flags.shared.mark_error(message.clone());
        Err(ReplayHandlerError::new(message))
    }
}

fn start_target_capture<T>(target: T, flags: ReplayCaptureFlags) -> Result<(), String>
where
    T: TryInto<GraphicsCaptureItemType> + Send + 'static,
{
    let shared = Arc::clone(&flags.shared);
    let settings = Settings::new(
        target,
        CursorCaptureSettings::Default,
        DrawBorderSettings::WithoutBorder,
        SecondaryWindowSettings::Default,
        MinimumUpdateIntervalSettings::Default,
        DirtyRegionSettings::Default,
        ColorFormat::Bgra8,
        flags,
    );

    let control =
        ReplayCaptureHandler::start_free_threaded(settings).map_err(|error| match error {
            GraphicsCaptureApiError::NewHandlerError(error) => error.to_string(),
            error => format!("Replay capture could not start: {error}"),
        })?;

    loop {
        if control.is_finished() {
            return control
                .wait()
                .map_err(|error| format!("Replay capture ended unexpectedly: {error}"));
        }
        if shared.should_stop() {
            return control
                .stop()
                .map_err(|error| format!("Replay capture could not stop cleanly: {error}"));
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn cleanup_session_directories(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|error| {
        format!(
            "Could not access replay-buffer root '{}': {error}",
            root.display()
        )
    })?;

    let entries = fs::read_dir(root).map_err(|error| {
        format!(
            "Could not inspect replay-buffer root '{}': {error}",
            root.display()
        )
    })?;
    for entry in entries {
        let entry =
            entry.map_err(|error| format!("Could not inspect a stale replay session: {error}"))?;
        let path = entry.path();
        if path.parent() != Some(root)
            || !entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false)
        {
            continue;
        }
        fs::remove_dir_all(&path).map_err(|error| {
            format!(
                "Could not remove stale replay session '{}': {error}",
                path.display()
            )
        })?;
    }

    Ok(())
}

fn path_is_inside_session(path: &Path, session_directory: Option<&Path>) -> bool {
    session_directory.is_some_and(|directory| path.parent() == Some(directory))
}

fn even_dimension(value: u32) -> Result<u32, String> {
    if value == 0 {
        return Err("The selected capture target has a zero-sized dimension.".to_string());
    }
    Ok(if value % 2 == 0 { value } else { value + 1 })
}

fn create_session_id() -> String {
    let counter = SESSION_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("session-{}-{counter:04}", unix_timestamp_ms())
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::{validate_start_request, ReplayBufferStartRequest, SharedReplay};
    use crate::capture::encoder::EncoderChoice;
    use crate::capture::targets::{CaptureTargetRequest, CaptureTargetType};
    use crate::replay::state::ReplayLifecycleState;

    fn request(duration: u32, frame_rate: u32) -> ReplayBufferStartRequest {
        ReplayBufferStartRequest {
            target: CaptureTargetRequest {
                target_type: CaptureTargetType::Monitor,
                id: "monitor:test".to_string(),
            },
            encoder: EncoderChoice::Automatic,
            replay_duration_seconds: duration,
            frame_rate,
        }
    }

    #[test]
    fn accepts_only_supported_replay_durations() {
        for duration in [30, 60, 120, 180, 300] {
            assert!(validate_start_request(&request(duration, 60)).is_ok());
        }
        assert!(validate_start_request(&request(45, 60)).is_err());
    }

    #[test]
    fn rejects_unsupported_frame_rate() {
        assert!(validate_start_request(&request(30, 144)).is_err());
    }

    #[test]
    fn normal_session_state_transitions_are_explicit() {
        let shared = SharedReplay::new();
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Stopped);

        shared.begin(&request(60, 60));
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Starting);

        shared.mark_running();
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Running);

        shared.request_stop();
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Stopping);

        shared.mark_stopped();
        assert_eq!(shared.snapshot().state, ReplayLifecycleState::Stopped);
    }
}
