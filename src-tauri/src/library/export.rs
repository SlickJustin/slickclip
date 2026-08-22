use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, ExitStatus, Stdio};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use crate::clips::ffmpeg::{FfmpegExecutable, MediaProbeReport, MediaProbeStream};

use super::models::{ClipAudioTrack, ClipListItem};
use super::{ClipLibraryManager, SavedClipMetadata};

const EXPORT_STATUS_EVENT: &str = "editor-export-status";
const MAX_EXPORT_SEGMENTS: usize = 1_024;
const MAX_EXPORT_TRACKS: usize = 64;
const MIN_EXPORT_SEGMENT_DURATION_US: i64 = 100_000;
const ENCODER_INITIALIZATION_WINDOW_US: i64 = 500_000;
const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(200);
const AUDIO_LIMITER: &str = "alimiter=limit=0.95:attack=5:release=50:level=false:latency=true";

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorExportRequest {
    pub clip_id: String,
    pub segments: Vec<EditorExportSegment>,
    pub mixer: Vec<EditorExportTrackMix>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorExportSegment {
    pub id: String,
    pub source_start_us: i64,
    pub source_end_us: i64,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorExportTrackMix {
    pub stream_index: u32,
    pub gain_percent: i32,
    pub muted: bool,
    pub solo: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum EditorExportPhase {
    #[default]
    Idle,
    Preparing,
    Rendering,
    Verifying,
    Finalizing,
    Complete,
    Failed,
    Cancelled,
}

impl EditorExportPhase {
    fn is_active(self) -> bool {
        matches!(
            self,
            Self::Preparing | Self::Rendering | Self::Verifying | Self::Finalizing
        )
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorExportStatus {
    pub export_id: Option<String>,
    pub source_clip_id: Option<String>,
    pub phase: EditorExportPhase,
    pub progress_percent: f64,
    pub encoded_time_us: i64,
    pub total_time_us: i64,
    pub encoder: Option<String>,
    pub encoder_hardware: Option<bool>,
    pub encoder_settings: Option<String>,
    pub attempted_encoders: Vec<String>,
    pub filter_plan: Option<String>,
    pub planned_duration_us: Option<i64>,
    pub verified_duration_us: Option<i64>,
    pub output_clip: Option<ClipListItem>,
    pub output_display_name: Option<String>,
    pub indexing_warning: Option<String>,
    pub error_message: Option<String>,
    pub diagnostics: Vec<String>,
}

impl Default for EditorExportStatus {
    fn default() -> Self {
        Self {
            export_id: None,
            source_clip_id: None,
            phase: EditorExportPhase::Idle,
            progress_percent: 0.0,
            encoded_time_us: 0,
            total_time_us: 0,
            encoder: None,
            encoder_hardware: None,
            encoder_settings: None,
            attempted_encoders: Vec::new(),
            filter_plan: None,
            planned_duration_us: None,
            verified_duration_us: None,
            output_clip: None,
            output_display_name: None,
            indexing_warning: None,
            error_message: None,
            diagnostics: Vec::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EditorExportCommandResponse {
    pub success: bool,
    pub status: EditorExportStatus,
    pub error_message: Option<String>,
}

#[derive(Clone)]
pub struct EditorExportManager {
    inner: Arc<ExportManagerInner>,
}

struct ExportManagerInner {
    library: ClipLibraryManager,
    clips_root: PathBuf,
    app: AppHandle,
    runtime: Mutex<ExportRuntime>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

#[derive(Default)]
struct ExportRuntime {
    status: EditorExportStatus,
    cancel_requested: bool,
    child: Option<Child>,
    partial_path: Option<PathBuf>,
    permanent_promoted: bool,
}

impl EditorExportManager {
    pub fn new(library: ClipLibraryManager, clips_root: PathBuf, app: AppHandle) -> Self {
        Self {
            inner: Arc::new(ExportManagerInner {
                library,
                clips_root,
                app,
                runtime: Mutex::new(ExportRuntime::default()),
                worker: Mutex::new(None),
            }),
        }
    }

    pub fn start(&self, request: EditorExportRequest) -> EditorExportCommandResponse {
        self.reap_finished_worker();
        let export_id = Uuid::new_v4().to_string();
        {
            let mut runtime = self.lock_runtime();
            if runtime.status.phase.is_active() {
                let message = "An Editor export is already in progress.".to_string();
                return EditorExportCommandResponse {
                    success: false,
                    status: runtime.status.clone(),
                    error_message: Some(message),
                };
            }
            runtime.status = EditorExportStatus {
                export_id: Some(export_id.clone()),
                source_clip_id: Some(request.clip_id.clone()),
                phase: EditorExportPhase::Preparing,
                ..Default::default()
            };
            runtime.cancel_requested = false;
            runtime.child = None;
            runtime.partial_path = None;
            runtime.permanent_promoted = false;
        }
        self.emit_status();

        let inner = self.inner.clone();
        let worker_export_id = export_id.clone();
        let spawn_result = thread::Builder::new()
            .name("slickclip-editor-export".into())
            .spawn(move || run_export_worker(inner, worker_export_id, request));
        match spawn_result {
            Ok(worker) => {
                *self.lock_worker() = Some(worker);
                EditorExportCommandResponse {
                    success: true,
                    status: self.status(),
                    error_message: None,
                }
            }
            Err(error) => {
                let message = format!("Could not start the Editor export worker: {error}");
                finish_failed(&self.inner, &export_id, message.clone(), Vec::new());
                EditorExportCommandResponse {
                    success: false,
                    status: self.status(),
                    error_message: Some(message),
                }
            }
        }
    }

    pub fn cancel(&self, export_id: &str) -> EditorExportCommandResponse {
        let mut runtime = self.lock_runtime();
        if runtime.status.export_id.as_deref() != Some(export_id)
            || !runtime.status.phase.is_active()
        {
            let message = "That Editor export is no longer active.".to_string();
            return EditorExportCommandResponse {
                success: false,
                status: runtime.status.clone(),
                error_message: Some(message),
            };
        }
        if runtime.permanent_promoted {
            let message =
                "The export has already been finalized and can no longer be cancelled.".to_string();
            return EditorExportCommandResponse {
                success: false,
                status: runtime.status.clone(),
                error_message: Some(message),
            };
        }
        runtime.cancel_requested = true;
        if let Some(child) = runtime.child.as_mut() {
            let _ = child.kill();
        }
        EditorExportCommandResponse {
            success: true,
            status: runtime.status.clone(),
            error_message: None,
        }
    }

    pub fn status(&self) -> EditorExportStatus {
        self.lock_runtime().status.clone()
    }

    pub fn shutdown_and_wait(&self) {
        {
            let mut runtime = self.lock_runtime();
            if runtime.status.phase.is_active() && !runtime.permanent_promoted {
                runtime.cancel_requested = true;
                if let Some(child) = runtime.child.as_mut() {
                    let _ = child.kill();
                }
            }
        }
        if let Some(worker) = self.lock_worker().take() {
            let _ = worker.join();
        }
        cleanup_partial(&self.inner);
    }

    fn emit_status(&self) {
        emit_status(&self.inner);
    }

    fn reap_finished_worker(&self) {
        let finished = self
            .lock_worker()
            .as_ref()
            .is_some_and(JoinHandle::is_finished);
        if finished {
            if let Some(worker) = self.lock_worker().take() {
                let _ = worker.join();
            }
        }
    }

    fn lock_runtime(&self) -> MutexGuard<'_, ExportRuntime> {
        lock_runtime(&self.inner)
    }

    fn lock_worker(&self) -> MutexGuard<'_, Option<JoinHandle<()>>> {
        self.inner
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[tauri::command]
pub fn start_editor_export(
    manager: State<'_, EditorExportManager>,
    request: EditorExportRequest,
) -> EditorExportCommandResponse {
    manager.start(request)
}

#[tauri::command]
pub fn cancel_editor_export(
    manager: State<'_, EditorExportManager>,
    export_id: String,
) -> EditorExportCommandResponse {
    manager.cancel(&export_id)
}

#[tauri::command]
pub fn get_editor_export_status(manager: State<'_, EditorExportManager>) -> EditorExportStatus {
    manager.status()
}

#[derive(Clone, Debug)]
struct ValidatedSegment {
    source_start_us: i64,
    source_end_us: i64,
}

impl ValidatedSegment {
    fn duration_us(&self) -> i64 {
        self.source_end_us - self.source_start_us
    }
}

#[derive(Clone, Debug)]
struct ValidatedTrack {
    metadata: ClipAudioTrack,
    mix: EditorExportTrackMix,
}

#[derive(Clone, Debug)]
struct PreparedExport {
    source_clip: ClipListItem,
    source_path: PathBuf,
    video_stream_index: u32,
    output_width: u32,
    output_height: u32,
    fps_numerator: u32,
    fps_denominator: u32,
    planned_duration_us: i64,
    segments: Vec<ValidatedSegment>,
    audible_tracks: Vec<ValidatedTrack>,
}

#[derive(Clone, Debug)]
struct ExportPaths {
    partial: PathBuf,
    final_path: PathBuf,
    display_name: String,
}

#[derive(Clone, Debug)]
struct EncoderCandidate {
    name: &'static str,
    hardware: bool,
    pixel_format: &'static str,
    settings: &'static [&'static str],
    settings_summary: &'static str,
}

#[derive(Clone, Debug)]
struct ExportCommandPlan {
    arguments: Vec<OsString>,
    filter_graph: String,
}

#[derive(Debug)]
struct AttemptOutcome {
    status: ExitStatus,
    stderr: String,
    max_encoded_time_us: i64,
}

#[derive(Debug)]
struct VerifiedExport {
    report: MediaProbeReport,
    duration_us: i64,
}

#[derive(Debug)]
struct ExportCompletion {
    clip: Option<ClipListItem>,
    display_name: String,
    verified_duration_us: i64,
    indexing_warning: Option<String>,
}

#[derive(Debug)]
enum ExportJobError {
    Cancelled,
    Failed(String),
}

impl From<String> for ExportJobError {
    fn from(value: String) -> Self {
        Self::Failed(value)
    }
}

fn run_export_worker(
    inner: Arc<ExportManagerInner>,
    export_id: String,
    request: EditorExportRequest,
) {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        execute_export(&inner, &export_id, request)
    }));
    match result {
        Ok(Ok(completion)) => finish_complete(&inner, &export_id, completion),
        Ok(Err(ExportJobError::Cancelled)) => finish_cancelled(&inner, &export_id),
        Ok(Err(ExportJobError::Failed(message))) => {
            let diagnostics = lock_runtime(&inner).status.diagnostics.clone();
            finish_failed(&inner, &export_id, message, diagnostics);
        }
        Err(_) => finish_failed(
            &inner,
            &export_id,
            "The Editor export worker stopped unexpectedly.".into(),
            Vec::new(),
        ),
    }
}

fn execute_export(
    inner: &Arc<ExportManagerInner>,
    export_id: &str,
    request: EditorExportRequest,
) -> Result<ExportCompletion, ExportJobError> {
    check_cancelled(inner, export_id)?;
    let ffmpeg = FfmpegExecutable::resolve().map_err(ExportJobError::Failed)?;
    let (source_clip, source_path) = inner
        .library
        .resolved_clip(&request.clip_id)
        .map_err(ExportJobError::Failed)?;
    let source_report = ffmpeg
        .inspect_media(&source_path)
        .map_err(ExportJobError::Failed)?;
    let prepared = validate_export_request(request, source_clip, source_path, &source_report)
        .map_err(ExportJobError::Failed)?;
    let paths = plan_export_paths(&inner.clips_root, &prepared.source_clip.display_name)
        .map_err(ExportJobError::Failed)?;
    validate_distinct_paths(&prepared.source_path, &paths).map_err(ExportJobError::Failed)?;
    {
        let mut runtime = lock_runtime(inner);
        if runtime.status.export_id.as_deref() != Some(export_id) {
            return Err(ExportJobError::Cancelled);
        }
        runtime.partial_path = Some(paths.partial.clone());
        runtime.status.total_time_us = prepared.planned_duration_us;
        runtime.status.planned_duration_us = Some(prepared.planned_duration_us);
        runtime.status.output_display_name = Some(paths.display_name.clone());
    }
    emit_status(inner);

    let mut selected_encoder = None;
    let candidates = encoder_candidates();
    for candidate in &candidates {
        check_cancelled(inner, export_id)?;
        remove_file_if_present(&paths.partial);
        let plan = build_export_command_plan(&prepared, &paths.partial, candidate)
            .map_err(ExportJobError::Failed)?;
        {
            let mut runtime = lock_runtime(inner);
            runtime.status.phase = EditorExportPhase::Rendering;
            runtime.status.progress_percent = 0.0;
            runtime.status.encoded_time_us = 0;
            runtime.status.encoder = Some(candidate.name.to_string());
            runtime.status.encoder_hardware = Some(candidate.hardware);
            runtime.status.encoder_settings = Some(candidate.settings_summary.to_string());
            runtime
                .status
                .attempted_encoders
                .push(candidate.name.to_string());
            runtime.status.filter_plan = Some(plan.filter_graph.clone());
        }
        emit_status(inner);
        let outcome = run_ffmpeg_attempt(
            inner,
            export_id,
            &ffmpeg,
            &plan,
            prepared.planned_duration_us,
        )?;
        check_cancelled(inner, export_id)?;
        if outcome.status.success() {
            selected_encoder = Some(candidate.clone());
            break;
        }

        remove_file_if_present(&paths.partial);
        let compact_error = compact_diagnostic(&outcome.stderr);
        eprintln!(
            "Editor export encoder {} failed (encoded {} us): {}",
            candidate.name, outcome.max_encoded_time_us, outcome.stderr
        );
        {
            let mut runtime = lock_runtime(inner);
            runtime.status.diagnostics.push(format!(
                "{} failed after {} us: {}",
                candidate.name, outcome.max_encoded_time_us, compact_error
            ));
        }
        let initialization_failure =
            candidate.hardware && outcome.max_encoded_time_us <= ENCODER_INITIALIZATION_WINDOW_US;
        if !initialization_failure {
            return Err(ExportJobError::Failed(format!(
                "Could not export clip. The {} encoder stopped during rendering.",
                candidate.name
            )));
        }
    }
    let selected_encoder = selected_encoder.ok_or_else(|| {
        ExportJobError::Failed(
            "Could not export clip. No available H.264 encoder could render the edit.".into(),
        )
    })?;

    check_cancelled(inner, export_id)?;
    set_phase(inner, export_id, EditorExportPhase::Verifying, 99.0)?;
    let verified =
        verify_export(&ffmpeg, &paths.partial, &prepared).map_err(ExportJobError::Failed)?;
    check_cancelled(inner, export_id)?;
    set_phase(inner, export_id, EditorExportPhase::Finalizing, 99.5)?;
    {
        let mut runtime = lock_runtime(inner);
        if runtime.cancel_requested {
            return Err(ExportJobError::Cancelled);
        }
        if paths.final_path.exists() {
            return Err(ExportJobError::Failed(
                "Could not finalize the export because its destination filename is already in use."
                    .into(),
            ));
        }
        fs::rename(&paths.partial, &paths.final_path).map_err(|error| {
            ExportJobError::Failed(format!("Could not promote the verified export: {error}"))
        })?;
        runtime.permanent_promoted = true;
        runtime.partial_path = None;
    }

    let metadata = exported_clip_metadata(&prepared, &verified, &paths.final_path);
    let indexing_started = Instant::now();
    let (clip, indexing_warning) = match inner
        .library
        .index_exported_clip(metadata, paths.display_name.clone())
    {
        Ok(indexed) => {
            let (clip, reload_warning) = match inner.library.clip_by_id(&indexed.clip_id) {
                Ok(Some(clip)) => (Some(clip), None),
                Ok(None) => (
                    None,
                    Some(
                        "Export completed and was indexed, but its Library record could not be reloaded. Refresh Clips to recover it."
                            .to_string(),
                    ),
                ),
                Err(error) => (
                    None,
                    Some(format!(
                        "Export completed and was indexed, but its Library record could not be reloaded: {error}. Refresh Clips to recover it."
                    )),
                ),
            };
            let _ = inner.app.emit("clip-library-changed", indexed.clip_id);
            (clip, reload_warning)
        }
        Err(error) => {
            inner.library.record_saved_clip_index_failure(
                indexing_started.elapsed().as_secs_f64() * 1_000.0,
            );
            (
                None,
                Some(format!(
                    "Export completed, but Library indexing failed: {error}. Refresh Clips to recover it."
                )),
            )
        }
    };
    {
        let mut runtime = lock_runtime(inner);
        runtime.status.encoder = Some(selected_encoder.name.to_string());
        runtime.status.encoder_hardware = Some(selected_encoder.hardware);
        runtime.status.encoder_settings = Some(selected_encoder.settings_summary.to_string());
    }
    Ok(ExportCompletion {
        clip,
        display_name: paths.display_name,
        verified_duration_us: verified.duration_us,
        indexing_warning,
    })
}

fn validate_export_request(
    request: EditorExportRequest,
    source_clip: ClipListItem,
    source_path: PathBuf,
    report: &MediaProbeReport,
) -> Result<PreparedExport, String> {
    if request.segments.is_empty() {
        return Err("An Editor export requires at least one timeline segment.".into());
    }
    if request.segments.len() > MAX_EXPORT_SEGMENTS {
        return Err(format!(
            "The Editor export contains too many timeline segments (maximum {MAX_EXPORT_SEGMENTS})."
        ));
    }
    if request.mixer.len() > MAX_EXPORT_TRACKS {
        return Err(format!(
            "The Editor export contains too many mixer tracks (maximum {MAX_EXPORT_TRACKS})."
        ));
    }
    let video_streams = report
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .collect::<Vec<_>>();
    if video_streams.len() != 1 {
        return Err("The source clip must contain exactly one verified video stream.".into());
    }
    let video = video_streams[0];
    let source_width = video
        .width
        .filter(|value| *value > 0)
        .ok_or_else(|| "The source video has no valid width.".to_string())?;
    let source_height = video
        .height
        .filter(|value| *value > 0)
        .ok_or_else(|| "The source video has no valid height.".to_string())?;
    let (fps_numerator, fps_denominator) = verified_frame_rate(video)?;
    let source_duration_us = probe_duration_us(report, video)?;
    let source_end_tolerance_us =
        source_end_reconciliation_tolerance_us(fps_numerator, fps_denominator);
    let library_duration_us = duration_100ns_to_us(source_clip.duration_100ns);
    let container_duration_us = report
        .format
        .as_ref()
        .and_then(|format| format.duration.as_deref())
        .and_then(parse_seconds_us)
        .filter(|duration| *duration > 0);
    let nominal_source_ends = [library_duration_us, container_duration_us];

    let mut segment_ids = HashSet::new();
    let mut segments = Vec::with_capacity(request.segments.len());
    let mut planned_duration_us = 0_i64;
    let mut previous_end_us = 0_i64;
    for (index, segment) in request.segments.into_iter().enumerate() {
        if segment.id.trim().is_empty() || !segment_ids.insert(segment.id) {
            return Err("Timeline segment IDs must be nonempty and unique.".into());
        }
        if segment.source_start_us < 0 {
            return Err(format!(
                "Timeline segment {} starts before the source.",
                index + 1
            ));
        }
        let source_end_us = canonicalize_source_end_us(
            segment.source_end_us,
            source_duration_us,
            &nominal_source_ends,
            source_end_tolerance_us,
        )
        .ok_or_else(|| {
            format!(
                "Timeline segment {} ends after the verified source duration.",
                index + 1
            )
        })?;
        if source_end_us <= segment.source_start_us {
            return Err(format!(
                "Timeline segment {} does not have a positive duration.",
                index + 1
            ));
        }
        if source_end_us - segment.source_start_us < MIN_EXPORT_SEGMENT_DURATION_US {
            return Err(format!(
                "Timeline segment {} is shorter than the Editor's 100 ms minimum.",
                index + 1
            ));
        }
        if index > 0 && segment.source_start_us < previous_end_us {
            return Err(
                "Timeline segments must remain in ordered, non-overlapping source order.".into(),
            );
        }
        let validated = ValidatedSegment {
            source_start_us: segment.source_start_us,
            source_end_us,
        };
        planned_duration_us = planned_duration_us
            .checked_add(validated.duration_us())
            .ok_or_else(|| "The edited duration overflowed its supported range.".to_string())?;
        previous_end_us = source_end_us;
        segments.push(validated);
    }
    if planned_duration_us <= 0 {
        return Err("The Editor export has no positive output duration.".into());
    }

    let probed_audio_indexes = report
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .map(|stream| stream.index)
        .collect::<HashSet<_>>();
    let eligible_tracks = eligible_editor_tracks(&source_clip.audio_tracks);
    let expected_indexes = eligible_tracks
        .iter()
        .map(|track| track.stream_index)
        .collect::<HashSet<_>>();
    let mut decisions = HashMap::new();
    for mix in request.mixer {
        if !(0..=300).contains(&mix.gain_percent) {
            return Err(format!(
                "Mixer gain for stream {} must be between 0% and 300%.",
                mix.stream_index
            ));
        }
        if !expected_indexes.contains(&mix.stream_index) {
            return Err(format!(
                "Mixer stream {} is not an eligible audio track for this source.",
                mix.stream_index
            ));
        }
        if !probed_audio_indexes.contains(&mix.stream_index) {
            return Err(format!(
                "Mixer stream {} is not a verified audio stream in the source MP4.",
                mix.stream_index
            ));
        }
        let stream_index = mix.stream_index;
        if decisions.insert(stream_index, mix).is_some() {
            return Err(format!(
                "Mixer stream {stream_index} appears more than once in the export request."
            ));
        }
    }
    if decisions.len() != eligible_tracks.len() {
        return Err("The mixer snapshot does not contain exactly one decision for every eligible Editor audio track."
            .into());
    }
    let ordered_tracks = eligible_tracks
        .into_iter()
        .map(|metadata| ValidatedTrack {
            mix: decisions
                .remove(&metadata.stream_index)
                .expect("eligible mixer decision validated"),
            metadata,
        })
        .collect::<Vec<_>>();
    let any_solo = ordered_tracks.iter().any(|track| track.mix.solo);
    let audible_tracks = ordered_tracks
        .into_iter()
        .filter(|track| !track.mix.muted && (!any_solo || track.mix.solo))
        .collect();

    Ok(PreparedExport {
        source_clip,
        source_path,
        video_stream_index: video.index,
        output_width: source_width + source_width % 2,
        output_height: source_height + source_height % 2,
        fps_numerator,
        fps_denominator,
        planned_duration_us,
        segments,
        audible_tracks,
    })
}

fn eligible_editor_tracks(tracks: &[ClipAudioTrack]) -> Vec<ClipAudioTrack> {
    let independent = tracks
        .iter()
        .filter(|track| !is_combined_track(track))
        .cloned()
        .collect::<Vec<_>>();
    if independent.is_empty() {
        tracks
            .iter()
            .find(|track| is_combined_track(track))
            .cloned()
            .into_iter()
            .collect()
    } else {
        independent
    }
}

fn is_combined_track(track: &ClipAudioTrack) -> bool {
    track.role.trim().eq_ignore_ascii_case("combined")
        || track
            .title
            .as_deref()
            .is_some_and(|title| title.trim().eq_ignore_ascii_case("combined"))
}

fn build_export_command_plan(
    prepared: &PreparedExport,
    partial_path: &Path,
    encoder: &EncoderCandidate,
) -> Result<ExportCommandPlan, String> {
    if prepared.source_path == partial_path {
        return Err("The export source and partial output paths must be different.".into());
    }
    let mut filters = Vec::new();
    let frame_padding = microseconds_decimal(frame_duration_us(
        prepared.fps_numerator,
        prepared.fps_denominator,
    ))?;
    for (index, segment) in prepared.segments.iter().enumerate() {
        filters.push(format!(
            "[0:{}]trim=start={}:end={},setpts=PTS-STARTPTS,tpad=stop_mode=clone:stop_duration={frame_padding},trim=duration={},setpts=PTS-STARTPTS[v{index}]",
            prepared.video_stream_index,
            microseconds_decimal(segment.source_start_us)?,
            microseconds_decimal(segment.source_end_us)?,
            microseconds_decimal(segment.duration_us())?,
        ));
    }
    let video_inputs = (0..prepared.segments.len())
        .map(|index| format!("[v{index}]"))
        .collect::<String>();
    filters.push(format!(
        "{video_inputs}concat=n={}:v=1:a=0,tpad=stop_mode=clone:stop_duration={frame_padding},trim=duration={},setpts=PTS-STARTPTS,pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p[vout]",
        prepared.segments.len(),
        microseconds_decimal(prepared.planned_duration_us)?,
    ));

    for (track_index, track) in prepared.audible_tracks.iter().enumerate() {
        for (segment_index, segment) in prepared.segments.iter().enumerate() {
            filters.push(format!(
                "[0:{}]atrim=start={}:end={},asetpts=PTS-STARTPTS,aresample=48000,aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo,apad,atrim=duration={}[a{track_index}s{segment_index}]",
                track.metadata.stream_index,
                microseconds_decimal(segment.source_start_us)?,
                microseconds_decimal(segment.source_end_us)?,
                microseconds_decimal(segment.duration_us())?,
            ));
        }
        let pieces = (0..prepared.segments.len())
            .map(|segment_index| format!("[a{track_index}s{segment_index}]"))
            .collect::<String>();
        filters.push(format!(
            "{pieces}concat=n={}:v=0:a=1,volume={}[again{track_index}]",
            prepared.segments.len(),
            gain_decimal(track.mix.gain_percent)?,
        ));
    }
    if !prepared.audible_tracks.is_empty() {
        let inputs = (0..prepared.audible_tracks.len())
            .map(|index| format!("[again{index}]"))
            .collect::<String>();
        let mix = if prepared.audible_tracks.len() == 1 {
            format!("{inputs}anull[amixed]")
        } else {
            format!(
                "{inputs}amix=inputs={}:duration=longest:dropout_transition=0:normalize=0[amixed]",
                prepared.audible_tracks.len()
            )
        };
        filters.push(mix);
        filters.push(format!(
            "[amixed]{AUDIO_LIMITER},atrim=duration={},asetpts=PTS-STARTPTS[aout]",
            microseconds_decimal(prepared.planned_duration_us)?,
        ));
    }
    let filter_graph = filters.join(";");
    let mut arguments = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
        "-i".into(),
        prepared.source_path.as_os_str().to_os_string(),
        "-filter_complex".into(),
        OsString::from(&filter_graph),
        "-map".into(),
        "[vout]".into(),
    ];
    if prepared.audible_tracks.is_empty() {
        arguments.push("-an".into());
    } else {
        arguments.extend(["-map".into(), "[aout]".into()]);
    }
    arguments.extend([
        "-map_metadata".into(),
        "-1".into(),
        "-map_chapters".into(),
        "-1".into(),
        "-c:v".into(),
        encoder.name.into(),
        "-pix_fmt".into(),
        encoder.pixel_format.into(),
        "-profile:v".into(),
        "high".into(),
        "-r".into(),
        format!("{}/{}", prepared.fps_numerator, prepared.fps_denominator).into(),
        "-fps_mode".into(),
        "cfr".into(),
    ]);
    arguments.extend(encoder.settings.iter().map(OsString::from));
    if !prepared.audible_tracks.is_empty() {
        arguments.extend([
            "-c:a".into(),
            "aac".into(),
            "-profile:a".into(),
            "aac_low".into(),
            "-b:a".into(),
            "192k".into(),
            "-ar:a".into(),
            "48000".into(),
            "-ac:a".into(),
            "2".into(),
            "-metadata:s:a:0".into(),
            "title=Combined".into(),
            "-metadata:s:a:0".into(),
            "handler_name=Combined".into(),
            "-disposition:a:0".into(),
            "default".into(),
        ]);
    }
    arguments.extend([
        "-movflags".into(),
        "+faststart".into(),
        "-progress".into(),
        "pipe:1".into(),
        "-nostats".into(),
        "-y".into(),
        partial_path.as_os_str().to_os_string(),
    ]);
    Ok(ExportCommandPlan {
        arguments,
        filter_graph,
    })
}

fn encoder_candidates() -> Vec<EncoderCandidate> {
    vec![
        EncoderCandidate {
            name: "h264_nvenc",
            hardware: true,
            pixel_format: "yuv420p",
            settings: &["-preset", "p5", "-tune", "hq", "-cq", "20", "-b:v", "0"],
            settings_summary: "NVENC p5, HQ tune, constant quality 20",
        },
        EncoderCandidate {
            name: "h264_amf",
            hardware: true,
            pixel_format: "yuv420p",
            settings: &[
                "-quality", "quality", "-rc", "cqp", "-qp_i", "20", "-qp_p", "22",
            ],
            settings_summary: "AMF quality preset, CQP I20/P22",
        },
        EncoderCandidate {
            name: "h264_qsv",
            hardware: true,
            pixel_format: "nv12",
            settings: &["-preset", "medium", "-global_quality", "20"],
            settings_summary: "Quick Sync medium preset, global quality 20",
        },
        EncoderCandidate {
            name: "libx264",
            hardware: false,
            pixel_format: "yuv420p",
            settings: &["-preset", "medium", "-crf", "20"],
            settings_summary: "libx264 medium preset, CRF 20",
        },
    ]
}

fn run_ffmpeg_attempt(
    inner: &Arc<ExportManagerInner>,
    export_id: &str,
    ffmpeg: &FfmpegExecutable,
    plan: &ExportCommandPlan,
    total_time_us: i64,
) -> Result<AttemptOutcome, ExportJobError> {
    let mut command = ffmpeg.export_command();
    command
        .args(&plan.arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| {
        ExportJobError::Failed(format!(
            "Could not launch FFmpeg for Editor export: {error}"
        ))
    })?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| ExportJobError::Failed("FFmpeg progress output was unavailable.".into()))?;
    let stderr = child.stderr.take().ok_or_else(|| {
        ExportJobError::Failed("FFmpeg diagnostic output was unavailable.".into())
    })?;
    let (progress_tx, progress_rx) = mpsc::channel();
    let progress_reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if let Some(value) = parse_ffmpeg_progress_line(&line) {
                let _ = progress_tx.send(value);
            }
        }
    });
    let stderr_reader = thread::spawn(move || {
        let mut reader = BufReader::new(stderr);
        let mut value = String::new();
        let _ = reader.read_to_string(&mut value);
        value
    });
    {
        let mut runtime = lock_runtime(inner);
        if runtime.status.export_id.as_deref() != Some(export_id) {
            let _ = child.kill();
            return Err(ExportJobError::Cancelled);
        }
        runtime.child = Some(child);
    }

    let mut max_encoded_time_us = 0_i64;
    let mut last_emit = Instant::now() - PROGRESS_EMIT_INTERVAL;
    let exit_status = loop {
        while let Ok(encoded_time_us) = progress_rx.try_recv() {
            max_encoded_time_us = max_encoded_time_us.max(encoded_time_us);
            if last_emit.elapsed() >= PROGRESS_EMIT_INTERVAL {
                update_render_progress(inner, export_id, max_encoded_time_us, total_time_us)?;
                last_emit = Instant::now();
            }
        }
        let maybe_status = {
            let mut runtime = lock_runtime(inner);
            if runtime.cancel_requested {
                if let Some(child) = runtime.child.as_mut() {
                    let _ = child.kill();
                }
            }
            runtime
                .child
                .as_mut()
                .ok_or_else(|| {
                    ExportJobError::Failed("The owned FFmpeg process disappeared.".into())
                })?
                .try_wait()
                .map_err(|error| {
                    ExportJobError::Failed(format!("Could not monitor FFmpeg export: {error}"))
                })?
        };
        if let Some(status) = maybe_status {
            let mut runtime = lock_runtime(inner);
            if let Some(mut child) = runtime.child.take() {
                let _ = child.wait();
            }
            break status;
        }
        thread::sleep(Duration::from_millis(40));
    };
    while let Ok(encoded_time_us) = progress_rx.try_recv() {
        max_encoded_time_us = max_encoded_time_us.max(encoded_time_us);
    }
    let _ = progress_reader.join();
    let stderr = stderr_reader.join().unwrap_or_default();
    if lock_runtime(inner).cancel_requested {
        return Err(ExportJobError::Cancelled);
    }
    Ok(AttemptOutcome {
        status: exit_status,
        stderr,
        max_encoded_time_us,
    })
}

fn verify_export(
    ffmpeg: &FfmpegExecutable,
    partial_path: &Path,
    prepared: &PreparedExport,
) -> Result<VerifiedExport, String> {
    let metadata = fs::metadata(partial_path)
        .map_err(|error| format!("FFmpeg did not create the expected export: {error}"))?;
    if !metadata.is_file() || metadata.len() == 0 {
        return Err("FFmpeg created an invalid or empty export artifact.".into());
    }
    let report = ffmpeg.inspect_media(partial_path)?;
    let duration_us = verify_export_report(&report, prepared)?;
    let tolerance_us = duration_tolerance_us(prepared.fps_numerator, prepared.fps_denominator);
    let packet_duration = ffmpeg.validate_packet_timeline_if_available(partial_path)?;
    if let Some(packet_duration) = packet_duration {
        let packet_duration_us = (packet_duration * 1_000_000.0).round() as i64;
        if (packet_duration_us - prepared.planned_duration_us).abs() > tolerance_us {
            return Err(
                "The exported video packet timeline does not match the planned edit duration."
                    .into(),
            );
        }
    }
    Ok(VerifiedExport {
        report,
        duration_us,
    })
}

fn verify_export_report(
    report: &MediaProbeReport,
    prepared: &PreparedExport,
) -> Result<i64, String> {
    let format_name = report
        .format
        .as_ref()
        .and_then(|format| format.format_name.as_deref())
        .unwrap_or_default();
    if !format_name.split(',').any(|name| name == "mp4") {
        return Err("The rendered export is not an MP4 container.".into());
    }
    let videos = report
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .collect::<Vec<_>>();
    let audios = report
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .collect::<Vec<_>>();
    let expected_stream_count = if prepared.audible_tracks.is_empty() {
        1
    } else {
        2
    };
    if videos.len() != 1 || report.streams.len() != expected_stream_count {
        return Err("The export does not contain exactly the expected flattened streams.".into());
    }
    let video = videos[0];
    if video.codec_name.as_deref() != Some("h264") {
        return Err("The exported video stream is not H.264/AVC.".into());
    }
    if video.width != Some(prepared.output_width) || video.height != Some(prepared.output_height) {
        return Err(format!(
            "The export resolution is not the expected {}x{}.",
            prepared.output_width, prepared.output_height
        ));
    }
    if video.pix_fmt.as_deref() != Some("yuv420p") {
        return Err("The exported video pixel format is not yuv420p.".into());
    }
    let (output_fps_numerator, output_fps_denominator) = verified_frame_rate(video)?;
    let expected_fps = prepared.fps_numerator as f64 / prepared.fps_denominator as f64;
    let output_fps = output_fps_numerator as f64 / output_fps_denominator as f64;
    if (expected_fps - output_fps).abs() > 0.01 {
        return Err(format!(
            "The exported frame rate ({output_fps:.6}) differs from the verified source ({expected_fps:.6})."
        ));
    }
    if prepared.audible_tracks.is_empty() {
        if !audios.is_empty() {
            return Err("The requested video-only export unexpectedly contains audio.".into());
        }
    } else {
        if audios.len() != 1 {
            return Err("The flattened export must contain exactly one audio stream.".into());
        }
        let audio = audios[0];
        let title = audio
            .tags
            .title
            .as_deref()
            .or(audio.tags.handler_name.as_deref());
        if audio.codec_name.as_deref() != Some("aac")
            || !audio
                .profile
                .as_deref()
                .is_some_and(|profile| profile.eq_ignore_ascii_case("LC"))
            || audio.sample_rate.as_deref() != Some("48000")
            || audio.channels != Some(2)
            || audio.disposition.is_default != 1
            || title != Some("Combined")
        {
            return Err("The flattened export does not contain the expected default AAC Combined audio stream."
                .into());
        }
    }
    let duration_us = probe_duration_us(report, video)?;
    let tolerance_us = duration_tolerance_us(prepared.fps_numerator, prepared.fps_denominator);
    if (duration_us - prepared.planned_duration_us).abs() > tolerance_us {
        return Err(format!(
            "The verified export duration differs from the planned edit by more than {} ms.",
            tolerance_us as f64 / 1_000.0
        ));
    }
    if let Some(audio) = audios.first() {
        if let Some(audio_duration_us) = audio.duration.as_deref().and_then(parse_seconds_us) {
            if (audio_duration_us - prepared.planned_duration_us).abs() > tolerance_us {
                return Err(
                    "The flattened audio duration does not match the edited video duration.".into(),
                );
            }
        }
    }
    Ok(duration_us)
}

fn exported_clip_metadata(
    prepared: &PreparedExport,
    verified: &VerifiedExport,
    final_path: &Path,
) -> SavedClipMetadata {
    let video = verified
        .report
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .expect("verified export has video");
    let audio_tracks = verified
        .report
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .map(|stream| ClipAudioTrack {
            stream_index: stream.index,
            role: "Combined".into(),
            title: Some("Combined".into()),
            handler_name: Some("Combined".into()),
            codec: "aac".into(),
            profile: stream.profile.clone().or_else(|| Some("LC".into())),
            sample_rate: Some(48_000),
            channels: Some(2),
            bitrate_bps: stream
                .bit_rate
                .as_deref()
                .and_then(|value| value.parse::<u64>().ok()),
            is_default: true,
        })
        .collect();
    SavedClipMetadata {
        file_path: final_path.to_path_buf(),
        created_at_ms: now_ms(),
        duration_100ns: verified.duration_us.saturating_mul(10),
        requested_duration_seconds: u32::try_from(
            (prepared.planned_duration_us + 999_999) / 1_000_000,
        )
        .unwrap_or(u32::MAX),
        width: prepared.output_width,
        height: prepared.output_height,
        fps_numerator: prepared.fps_numerator,
        fps_denominator: prepared.fps_denominator,
        video_codec: "h264".into(),
        video_profile: video.profile.clone(),
        video_bitrate_bps: video
            .bit_rate
            .as_deref()
            .and_then(|value| value.parse::<u64>().ok()),
        total_bitrate_bps: verified
            .report
            .format
            .as_ref()
            .and_then(|format| format.bit_rate.as_deref())
            .and_then(|value| value.parse::<u64>().ok()),
        capture_target_label: prepared.source_clip.capture_target_label.clone(),
        capture_target_type: prepared.source_clip.capture_target_type.clone(),
        audio_tracks,
    }
}

fn plan_export_paths(root: &Path, source_display_name: &str) -> Result<ExportPaths, String> {
    fs::create_dir_all(root)
        .map_err(|error| format!("Could not create the permanent Clips directory: {error}"))?;
    let root = root
        .canonicalize()
        .map_err(|error| format!("Could not resolve the permanent Clips directory: {error}"))?;
    let base = sanitize_export_name(source_display_name);
    for collision in 1..=10_000_u32 {
        let display_name = if collision == 1 {
            format!("{base} - Edited")
        } else {
            format!("{base} - Edited ({collision})")
        };
        let final_path = root.join(format!("{display_name}.mp4"));
        let partial = root.join(format!("{display_name}.partial.mp4"));
        if !final_path.exists() && !partial.exists() {
            if final_path.parent() != Some(root.as_path())
                || partial.parent() != Some(root.as_path())
            {
                return Err("Export paths escaped the permanent Clips directory.".into());
            }
            return Ok(ExportPaths {
                partial,
                final_path,
                display_name,
            });
        }
    }
    Err("Could not find a collision-free filename for the edited export.".into())
}

fn sanitize_export_name(value: &str) -> String {
    let mut result = value
        .chars()
        .map(|character| {
            if character.is_control() || "<>:\"/\\|?*".contains(character) {
                '-'
            } else {
                character
            }
        })
        .take(80)
        .collect::<String>();
    result = result.trim().trim_matches('.').trim().to_string();
    if result.is_empty() {
        result = "SlickClip".into();
    }
    let reserved = [
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    if reserved
        .iter()
        .any(|reserved| result.eq_ignore_ascii_case(reserved))
    {
        result.insert(0, '_');
    }
    result
}

fn validate_distinct_paths(source: &Path, paths: &ExportPaths) -> Result<(), String> {
    if source == paths.partial || source == paths.final_path || paths.partial == paths.final_path {
        return Err(
            "Source, partial output, and permanent output paths must all be distinct.".into(),
        );
    }
    Ok(())
}

fn microseconds_decimal(value: i64) -> Result<String, String> {
    if value < 0 {
        return Err("A media timestamp cannot be negative.".into());
    }
    Ok(format!("{}.{:06}", value / 1_000_000, value % 1_000_000))
}

fn gain_decimal(value: i32) -> Result<String, String> {
    if !(0..=300).contains(&value) {
        return Err("Track gain must be between 0% and 300%.".into());
    }
    let whole = value / 100;
    let remainder = value % 100;
    Ok(if remainder == 0 {
        whole.to_string()
    } else if remainder % 10 == 0 {
        format!("{whole}.{}", remainder / 10)
    } else {
        format!("{whole}.{remainder:02}")
    })
}

fn parse_seconds_us(value: &str) -> Option<i64> {
    let value = value.trim();
    if value.is_empty() || value.starts_with('-') {
        return None;
    }
    let (whole, fraction) = value.split_once('.').unwrap_or((value, ""));
    let whole = whole.parse::<i64>().ok()?;
    if !fraction.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let mut microseconds = fraction.chars().take(6).collect::<String>();
    while microseconds.len() < 6 {
        microseconds.push('0');
    }
    let mut result = whole.checked_mul(1_000_000)?;
    result = result.checked_add(microseconds.parse::<i64>().ok()?)?;
    if fraction
        .as_bytes()
        .get(6)
        .is_some_and(|digit| *digit >= b'5')
    {
        result = result.checked_add(1)?;
    }
    Some(result)
}

fn probe_duration_us(report: &MediaProbeReport, video: &MediaProbeStream) -> Result<i64, String> {
    video
        .duration
        .as_deref()
        .and_then(parse_seconds_us)
        .or_else(|| {
            report
                .format
                .as_ref()
                .and_then(|format| format.duration.as_deref())
                .and_then(parse_seconds_us)
        })
        .filter(|duration| *duration > 0)
        .ok_or_else(|| "The media does not have a verified positive duration.".into())
}

fn duration_100ns_to_us(duration_100ns: i64) -> Option<i64> {
    (duration_100ns > 0)
        .then(|| duration_100ns.checked_add(5)?.checked_div(10))
        .flatten()
}

fn canonicalize_source_end_us(
    requested_end_us: i64,
    verified_video_end_us: i64,
    nominal_source_ends_us: &[Option<i64>],
    maximum_rounding_us: i64,
) -> Option<i64> {
    if requested_end_us <= verified_video_end_us {
        return Some(requested_end_us);
    }
    let overrun_us = requested_end_us.checked_sub(verified_video_end_us)?;
    (overrun_us <= maximum_rounding_us
        && nominal_source_ends_us
            .iter()
            .flatten()
            .any(|nominal_end_us| *nominal_end_us == requested_end_us))
    .then_some(verified_video_end_us)
}

fn verified_frame_rate(stream: &MediaProbeStream) -> Result<(u32, u32), String> {
    stream
        .avg_frame_rate
        .as_deref()
        .and_then(parse_rate)
        .or_else(|| stream.r_frame_rate.as_deref().and_then(parse_rate))
        .ok_or_else(|| "The video does not have a verified positive frame rate.".into())
}

fn parse_rate(value: &str) -> Option<(u32, u32)> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator = numerator.parse::<u32>().ok()?;
    let denominator = denominator.parse::<u32>().ok()?;
    (numerator > 0 && denominator > 0).then_some((numerator, denominator))
}

fn duration_tolerance_us(fps_numerator: u32, fps_denominator: u32) -> i64 {
    frame_duration_us(fps_numerator, fps_denominator).saturating_add(35_000)
}

fn source_end_reconciliation_tolerance_us(fps_numerator: u32, fps_denominator: u32) -> i64 {
    frame_duration_us(fps_numerator, fps_denominator).saturating_add(1) / 2
}

fn frame_duration_us(fps_numerator: u32, fps_denominator: u32) -> i64 {
    let frame_us = (1_000_000_u64 * u64::from(fps_denominator)).div_ceil(u64::from(fps_numerator));
    i64::try_from(frame_us).unwrap_or(i64::MAX)
}

fn parse_ffmpeg_progress_line(line: &str) -> Option<i64> {
    if let Some(value) = line
        .strip_prefix("out_time_us=")
        .or_else(|| line.strip_prefix("out_time_ms="))
    {
        return value.parse::<i64>().ok().filter(|value| *value >= 0);
    }
    let value = line.strip_prefix("out_time=")?;
    let mut pieces = value.split(':');
    let hours = pieces.next()?.parse::<i64>().ok()?;
    let minutes = pieces.next()?.parse::<i64>().ok()?;
    let seconds = parse_seconds_us(pieces.next()?)?;
    if pieces.next().is_some() || hours < 0 || !(0..60).contains(&minutes) {
        return None;
    }
    hours
        .checked_mul(3_600_000_000)?
        .checked_add(minutes.checked_mul(60_000_000)?)?
        .checked_add(seconds)
}

fn update_render_progress(
    inner: &Arc<ExportManagerInner>,
    export_id: &str,
    encoded_time_us: i64,
    total_time_us: i64,
) -> Result<(), ExportJobError> {
    {
        let mut runtime = lock_runtime(inner);
        if runtime.status.export_id.as_deref() != Some(export_id) {
            return Err(ExportJobError::Cancelled);
        }
        runtime.status.encoded_time_us = encoded_time_us.clamp(0, total_time_us);
        runtime.status.progress_percent = if total_time_us > 0 {
            (encoded_time_us as f64 / total_time_us as f64 * 100.0).clamp(0.0, 98.5)
        } else {
            0.0
        };
    }
    emit_status(inner);
    Ok(())
}

fn set_phase(
    inner: &Arc<ExportManagerInner>,
    export_id: &str,
    phase: EditorExportPhase,
    progress_percent: f64,
) -> Result<(), ExportJobError> {
    {
        let mut runtime = lock_runtime(inner);
        if runtime.status.export_id.as_deref() != Some(export_id) {
            return Err(ExportJobError::Cancelled);
        }
        runtime.status.phase = phase;
        runtime.status.progress_percent = progress_percent;
        runtime.status.encoded_time_us = runtime.status.total_time_us;
    }
    emit_status(inner);
    Ok(())
}

fn check_cancelled(inner: &Arc<ExportManagerInner>, export_id: &str) -> Result<(), ExportJobError> {
    let runtime = lock_runtime(inner);
    if runtime.status.export_id.as_deref() != Some(export_id) || runtime.cancel_requested {
        Err(ExportJobError::Cancelled)
    } else {
        Ok(())
    }
}

fn finish_complete(inner: &Arc<ExportManagerInner>, export_id: &str, completion: ExportCompletion) {
    {
        let mut runtime = lock_runtime(inner);
        if runtime.status.export_id.as_deref() != Some(export_id) {
            return;
        }
        runtime.child = None;
        runtime.partial_path = None;
        runtime.status.phase = EditorExportPhase::Complete;
        runtime.status.progress_percent = 100.0;
        runtime.status.encoded_time_us = runtime.status.total_time_us;
        runtime.status.verified_duration_us = Some(completion.verified_duration_us);
        runtime.status.output_clip = completion.clip;
        runtime.status.output_display_name = Some(completion.display_name);
        runtime.status.indexing_warning = completion.indexing_warning;
        runtime.status.error_message = None;
    }
    emit_status(inner);
}

fn finish_cancelled(inner: &Arc<ExportManagerInner>, export_id: &str) {
    cleanup_partial(inner);
    {
        let mut runtime = lock_runtime(inner);
        if runtime.status.export_id.as_deref() != Some(export_id) {
            return;
        }
        runtime.child = None;
        runtime.partial_path = None;
        runtime.status.phase = EditorExportPhase::Cancelled;
        runtime.status.error_message = None;
    }
    emit_status(inner);
}

fn finish_failed(
    inner: &Arc<ExportManagerInner>,
    export_id: &str,
    message: String,
    diagnostics: Vec<String>,
) {
    cleanup_partial(inner);
    {
        let mut runtime = lock_runtime(inner);
        if runtime.status.export_id.as_deref() != Some(export_id) {
            return;
        }
        runtime.child = None;
        runtime.partial_path = None;
        runtime.status.phase = EditorExportPhase::Failed;
        runtime.status.error_message = Some(message);
        runtime.status.diagnostics = diagnostics;
    }
    emit_status(inner);
}

fn cleanup_partial(inner: &Arc<ExportManagerInner>) {
    let partial_path = lock_runtime(inner).partial_path.clone();
    if let Some(path) = partial_path {
        remove_file_if_present(&path);
    }
}

fn remove_file_if_present(path: &Path) {
    if path.is_file() {
        let _ = fs::remove_file(path);
    }
}

fn compact_diagnostic(value: &str) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    compact.chars().take(1_500).collect()
}

fn emit_status(inner: &Arc<ExportManagerInner>) {
    let status = lock_runtime(inner).status.clone();
    let _ = inner.app.emit(EXPORT_STATUS_EVENT, status);
}

fn lock_runtime(inner: &Arc<ExportManagerInner>) -> MutexGuard<'_, ExportRuntime> {
    inner
        .runtime
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audio(index: u32, role: &str) -> ClipAudioTrack {
        ClipAudioTrack {
            stream_index: index,
            role: role.into(),
            title: Some(role.into()),
            handler_name: Some(role.into()),
            codec: "aac".into(),
            profile: Some("LC".into()),
            sample_rate: Some(48_000),
            channels: Some(2),
            bitrate_bps: Some(192_000),
            is_default: role == "Combined",
        }
    }

    fn source_clip(audio_tracks: Vec<ClipAudioTrack>) -> ClipListItem {
        ClipListItem {
            id: Uuid::new_v4().to_string(),
            file_path: "C:\\Clips\\source.mp4".into(),
            filename: "source.mp4".into(),
            display_name: "Source".into(),
            created_at_ms: 0,
            library_added_at_ms: 0,
            file_modified_at_ms: 0,
            file_size_bytes: 1,
            duration_100ns: 300_000_000,
            requested_duration_seconds: Some(30),
            width: 2_560,
            height: 1_440,
            fps_numerator: 60,
            fps_denominator: 1,
            video_codec: "hevc".into(),
            video_profile: None,
            video_bitrate_bps: None,
            total_bitrate_bps: None,
            capture_target_label: None,
            capture_target_type: None,
            favorite: false,
            imported_existing_file: false,
            audio_stream_count: u32::try_from(audio_tracks.len()).unwrap(),
            default_audio_stream_title: Some("Combined".into()),
            metadata_version: 1,
            audio_tracks,
            play_count: 0,
            last_watched_at_ms: None,
            collection_ids: Vec::new(),
        }
    }

    fn probe(audio_indexes: &[u32]) -> MediaProbeReport {
        let mut streams = vec![MediaProbeStream {
            index: 0,
            codec_name: Some("hevc".into()),
            profile: None,
            codec_type: Some("video".into()),
            width: Some(2_560),
            height: Some(1_440),
            pix_fmt: Some("yuv420p".into()),
            r_frame_rate: Some("60/1".into()),
            avg_frame_rate: Some("60/1".into()),
            sample_rate: None,
            channels: None,
            duration: Some("30.000000".into()),
            bit_rate: None,
            tags: Default::default(),
            disposition: Default::default(),
        }];
        streams.extend(audio_indexes.iter().map(|index| MediaProbeStream {
            index: *index,
            codec_name: Some("aac".into()),
            profile: Some("LC".into()),
            codec_type: Some("audio".into()),
            width: None,
            height: None,
            pix_fmt: None,
            r_frame_rate: None,
            avg_frame_rate: None,
            sample_rate: Some("48000".into()),
            channels: Some(2),
            duration: Some("30.000000".into()),
            bit_rate: Some("192000".into()),
            tags: Default::default(),
            disposition: Default::default(),
        }));
        MediaProbeReport {
            streams,
            format: Some(crate::clips::ffmpeg::MediaProbeFormat {
                format_name: Some("mov,mp4,m4a,3gp,3g2,mj2".into()),
                duration: Some("30.000000".into()),
                bit_rate: None,
            }),
        }
    }

    fn output_probe(with_audio: bool, duration: &str) -> MediaProbeReport {
        let mut report = probe(if with_audio { &[1] } else { &[] });
        let video = &mut report.streams[0];
        video.codec_name = Some("h264".into());
        video.duration = Some(duration.into());
        report.format.as_mut().unwrap().duration = Some(duration.into());
        if with_audio {
            let audio = &mut report.streams[1];
            audio.duration = Some(duration.into());
            audio.tags.title = Some("Combined".into());
            audio.tags.handler_name = Some("Combined".into());
            audio.disposition.is_default = 1;
        }
        report
    }

    fn request(
        segments: Vec<(i64, i64)>,
        tracks: &[(u32, i32, bool, bool)],
    ) -> EditorExportRequest {
        EditorExportRequest {
            clip_id: "trusted-id".into(),
            segments: segments
                .into_iter()
                .enumerate()
                .map(|(index, (start, end))| EditorExportSegment {
                    id: format!("segment-{index}"),
                    source_start_us: start,
                    source_end_us: end,
                })
                .collect(),
            mixer: tracks
                .iter()
                .map(
                    |(stream_index, gain_percent, muted, solo)| EditorExportTrackMix {
                        stream_index: *stream_index,
                        gain_percent: *gain_percent,
                        muted: *muted,
                        solo: *solo,
                    },
                )
                .collect(),
        }
    }

    fn prepare(
        segments: Vec<(i64, i64)>,
        source_tracks: Vec<ClipAudioTrack>,
        decisions: &[(u32, i32, bool, bool)],
    ) -> PreparedExport {
        let indexes = source_tracks
            .iter()
            .map(|track| track.stream_index)
            .collect::<Vec<_>>();
        validate_export_request(
            request(segments, decisions),
            source_clip(source_tracks),
            PathBuf::from("C:\\Clips\\source.mp4"),
            &probe(&indexes),
        )
        .unwrap()
    }

    fn filter(prepared: &PreparedExport) -> String {
        build_export_command_plan(
            prepared,
            Path::new("C:\\Clips\\output.partial.mp4"),
            encoder_candidates().last().unwrap(),
        )
        .unwrap()
        .filter_graph
    }

    #[test]
    fn exact_microsecond_and_gain_formatting_is_deterministic() {
        assert_eq!(microseconds_decimal(12_345_678).unwrap(), "12.345678");
        assert_eq!(microseconds_decimal(1).unwrap(), "0.000001");
        assert!(microseconds_decimal(-1).is_err());
        assert_eq!(gain_decimal(50).unwrap(), "0.5");
        assert_eq!(gain_decimal(100).unwrap(), "1");
        assert_eq!(gain_decimal(150).unwrap(), "1.5");
        assert_eq!(gain_decimal(300).unwrap(), "3");
    }

    #[test]
    fn full_trimmed_two_and_three_segment_video_plans_are_contiguous() {
        let tracks = vec![audio(1, "Combined"), audio(2, "Game")];
        let full = prepare(
            vec![(0, 30_000_000)],
            tracks.clone(),
            &[(2, 100, false, false)],
        );
        assert_eq!(full.planned_duration_us, 30_000_000);
        assert!(filter(&full).contains("trim=start=0.000000:end=30.000000"));

        let trimmed = prepare(
            vec![(2_500_001, 27_250_009)],
            tracks.clone(),
            &[(2, 100, false, false)],
        );
        assert_eq!(trimmed.planned_duration_us, 24_750_008);
        assert!(filter(&trimmed).contains("start=2.500001:end=27.250009"));
        let trimmed_beginning = prepare(
            vec![(2_500_001, 30_000_000)],
            tracks.clone(),
            &[(2, 100, false, false)],
        );
        assert!(filter(&trimmed_beginning).contains("start=2.500001:end=30.000000"));
        let trimmed_end = prepare(
            vec![(0, 27_250_009)],
            tracks.clone(),
            &[(2, 100, false, false)],
        );
        assert!(filter(&trimmed_end).contains("start=0.000000:end=27.250009"));

        let cut = prepare(
            vec![(0, 5_000_000), (10_000_000, 20_000_000)],
            tracks.clone(),
            &[(2, 100, false, false)],
        );
        let cut_filter = filter(&cut);
        assert_eq!(cut.planned_duration_us, 15_000_000);
        assert!(cut_filter.contains("[v0][v1]concat=n=2:v=1:a=0"));
        assert!(!cut_filter.contains("start=5.000000:end=10.000000"));

        let three = prepare(
            vec![
                (0, 2_000_000),
                (5_000_000, 7_000_000),
                (11_000_000, 15_000_000),
            ],
            tracks,
            &[(2, 100, false, false)],
        );
        assert!(filter(&three).contains("[v0][v1][v2]concat=n=3:v=1:a=0"));
    }

    #[test]
    fn invalid_zero_negative_overlapping_and_out_of_duration_segments_are_rejected() {
        let tracks = vec![audio(2, "Game")];
        let source = source_clip(tracks.clone());
        let report = probe(&[2]);
        for segments in [
            vec![(0, 0)],
            vec![(0, MIN_EXPORT_SEGMENT_DURATION_US - 1)],
            vec![(-1, 1)],
            vec![(0, 5_000_000), (4_000_000, 6_000_000)],
            vec![(0, 30_000_001)],
        ] {
            assert!(validate_export_request(
                request(segments, &[(2, 100, false, false)]),
                source.clone(),
                PathBuf::from("source.mp4"),
                &report,
            )
            .is_err());
        }
    }

    #[test]
    fn exact_verified_source_endpoint_is_accepted_without_reconciliation() {
        let prepared = prepare(
            vec![(0, 30_000_000)],
            vec![audio(2, "Game")],
            &[(2, 100, false, false)],
        );
        assert_eq!(prepared.segments[0].source_end_us, 30_000_000);
        assert_eq!(prepared.planned_duration_us, 30_000_000);
    }

    #[test]
    fn trusted_container_endpoint_within_half_frame_is_canonicalized_for_every_stream() {
        let tracks = vec![
            audio(1, "Combined"),
            audio(2, "Game"),
            audio(3, "VoiceChat"),
            audio(4, "Microphone"),
        ];
        let source = source_clip(tracks);
        let mut report = probe(&[1, 2, 3, 4]);
        report.streams[0].duration = Some("27.999783".into());
        report.streams[0].avg_frame_rate = Some("100800000/1679987".into());
        report.format.as_mut().unwrap().duration = Some("28.000000".into());
        let prepared = validate_export_request(
            request(
                vec![(0, 5_620_000), (19_100_000, 28_000_000)],
                &[
                    (2, 60, false, false),
                    (3, 140, false, false),
                    (4, 200, false, false),
                ],
            ),
            source,
            PathBuf::from("source.mp4"),
            &report,
        )
        .unwrap();

        assert_eq!(
            source_end_reconciliation_tolerance_us(100_800_000, 1_679_987),
            8_334
        );
        assert_eq!(prepared.segments[0].source_end_us, 5_620_000);
        assert_eq!(prepared.segments[1].source_start_us, 19_100_000);
        assert_eq!(prepared.segments[1].source_end_us, 27_999_783);
        assert_eq!(prepared.planned_duration_us, 14_519_783);
        assert!(prepared
            .segments
            .windows(2)
            .all(|segments| segments[0].source_end_us <= segments[1].source_start_us));

        let graph = filter(&prepared);
        assert!(graph.contains("[0:0]trim=start=19.100000:end=27.999783"));
        for stream in [2, 3, 4] {
            assert!(graph.contains(&format!("[0:{stream}]atrim=start=19.100000:end=27.999783")));
        }
    }

    #[test]
    fn reconciliation_rejects_non_nominal_overruns_and_post_clamp_short_segments() {
        let source = source_clip(vec![audio(2, "Game")]);
        let mut report = probe(&[2]);
        report.streams[0].duration = Some("27.999783".into());
        report.format.as_mut().unwrap().duration = Some("28.000000".into());

        for segments in [
            vec![(0, 28_000_001)],
            vec![(0, 29_000_000)],
            vec![(27_899_900, 28_000_000)],
        ] {
            assert!(validate_export_request(
                request(segments, &[(2, 100, false, false)]),
                source.clone(),
                PathBuf::from("source.mp4"),
                &report,
            )
            .is_err());
        }

        let mut one_fps_report = probe(&[2]);
        one_fps_report.streams[0].duration = Some("28.000000".into());
        one_fps_report.streams[0].avg_frame_rate = Some("1/1".into());
        one_fps_report.format.as_mut().unwrap().duration = Some("29.000000".into());
        assert!(validate_export_request(
            request(vec![(0, 29_000_000)], &[(2, 100, false, false)]),
            source,
            PathBuf::from("source.mp4"),
            &one_fps_report,
        )
        .is_err());
    }

    #[test]
    fn audio_edl_gain_mix_and_limiter_follow_every_included_track() {
        let tracks = vec![
            audio(1, "Combined"),
            audio(2, "Game"),
            audio(3, "VoiceChat"),
            audio(4, "Microphone"),
        ];
        let prepared = prepare(
            vec![(0, 5_000_000), (10_000_000, 20_000_000)],
            tracks,
            &[
                (2, 50, false, false),
                (3, 150, false, false),
                (4, 300, false, false),
            ],
        );
        let graph = filter(&prepared);
        assert_eq!(prepared.audible_tracks.len(), 3);
        assert!(!graph.contains("[0:1]atrim"));
        for stream in [2, 3, 4] {
            assert!(graph.contains(&format!("[0:{stream}]atrim=start=0.000000:end=5.000000")));
            assert!(graph.contains(&format!("[0:{stream}]atrim=start=10.000000:end=20.000000")));
        }
        assert!(graph.contains("volume=0.5"));
        assert!(graph.contains("volume=1.5"));
        assert!(graph.contains("volume=3"));
        assert!(graph.contains("amix=inputs=3:duration=longest:dropout_transition=0:normalize=0"));
        assert!(graph.contains(AUDIO_LIMITER));
    }

    #[test]
    fn mute_solo_multiple_solo_and_mute_override_match_preview_rules() {
        let tracks = vec![
            audio(2, "Game"),
            audio(3, "VoiceChat"),
            audio(4, "Microphone"),
        ];
        let no_solo = prepare(
            vec![(0, 10_000_000)],
            tracks.clone(),
            &[
                (2, 100, true, false),
                (3, 100, false, false),
                (4, 100, false, false),
            ],
        );
        assert_eq!(
            no_solo
                .audible_tracks
                .iter()
                .map(|track| track.metadata.stream_index)
                .collect::<Vec<_>>(),
            vec![3, 4]
        );
        let solos = prepare(
            vec![(0, 10_000_000)],
            tracks,
            &[
                (2, 100, false, true),
                (3, 100, false, true),
                (4, 100, true, true),
            ],
        );
        assert_eq!(
            solos
                .audible_tracks
                .iter()
                .map(|track| track.metadata.stream_index)
                .collect::<Vec<_>>(),
            vec![2, 3]
        );
    }

    #[test]
    fn combined_is_fallback_only_and_all_muted_is_video_only() {
        let combined = prepare(
            vec![(0, 10_000_000)],
            vec![audio(1, "Combined")],
            &[(1, 100, false, false)],
        );
        assert_eq!(combined.audible_tracks[0].metadata.stream_index, 1);
        let video_only = prepare(
            vec![(0, 10_000_000)],
            vec![audio(2, "Game")],
            &[(2, 100, true, false)],
        );
        assert!(video_only.audible_tracks.is_empty());
        let plan = build_export_command_plan(
            &video_only,
            Path::new("output.partial.mp4"),
            encoder_candidates().last().unwrap(),
        )
        .unwrap();
        let args = plan
            .arguments
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();
        assert!(args.iter().any(|value| value == "-an"));
        assert!(!plan.filter_graph.contains(AUDIO_LIMITER));

        let no_source_audio = prepare(vec![(0, 10_000_000)], Vec::new(), &[]);
        assert!(no_source_audio.audible_tracks.is_empty());
    }

    #[test]
    fn audible_export_maps_only_aac_lc_combined_with_compatibility_settings() {
        let prepared = prepare(
            vec![(0, 10_000_000)],
            vec![audio(2, "Game")],
            &[(2, 100, false, false)],
        );
        let plan = build_export_command_plan(
            &prepared,
            Path::new("output.partial.mp4"),
            encoder_candidates().last().unwrap(),
        )
        .unwrap();
        let args = plan
            .arguments
            .iter()
            .map(|value| value.to_string_lossy())
            .collect::<Vec<_>>();
        for expected in [
            "aac",
            "aac_low",
            "192k",
            "48000",
            "2",
            "title=Combined",
            "handler_name=Combined",
            "default",
            "+faststart",
        ] {
            assert!(args.iter().any(|value| value == expected));
        }
    }

    #[test]
    fn stale_duplicate_missing_and_non_audio_mixer_indexes_are_rejected() {
        let tracks = vec![audio(2, "Game")];
        let source = source_clip(tracks);
        let report = probe(&[2]);
        for decisions in [
            vec![],
            vec![(2, 100, false, false), (2, 100, false, false)],
            vec![(0, 100, false, false)],
            vec![(99, 100, false, false)],
        ] {
            assert!(validate_export_request(
                request(vec![(0, 10_000_000)], &decisions),
                source.clone(),
                PathBuf::from("source.mp4"),
                &report,
            )
            .is_err());
        }
    }

    #[test]
    fn filename_collision_and_path_separation_are_owned_and_safe() {
        let root = std::env::temp_dir().join(format!("stage17-paths-{}", Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        let first = plan_export_paths(&root, "Raid: Night? 1").unwrap();
        assert_eq!(first.display_name, "Raid- Night- 1 - Edited");
        fs::write(&first.final_path, b"existing").unwrap();
        let second = plan_export_paths(&root, "Raid: Night? 1").unwrap();
        assert_eq!(second.display_name, "Raid- Night- 1 - Edited (2)");
        assert_eq!(
            second.final_path.parent(),
            Some(root.canonicalize().unwrap().as_path())
        );
        assert_eq!(second.partial.parent(), second.final_path.parent());
        assert!(validate_distinct_paths(Path::new("source.mp4"), &second).is_ok());
        assert!(validate_distinct_paths(&second.final_path, &second).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn progress_parsing_and_duration_tolerance_are_bounded() {
        assert_eq!(
            parse_ffmpeg_progress_line("out_time_us=1234567"),
            Some(1_234_567)
        );
        assert_eq!(
            parse_ffmpeg_progress_line("out_time=00:00:12.345678"),
            Some(12_345_678)
        );
        assert!(duration_tolerance_us(60, 1) < 60_000);
        assert!(duration_tolerance_us(30, 1) < 70_000);
    }

    #[test]
    fn verification_accepts_only_flattened_h264_mp4_with_expected_audio_and_duration() {
        let with_audio = prepare(
            vec![(0, 10_000_000)],
            vec![audio(2, "Game")],
            &[(2, 100, false, false)],
        );
        assert_eq!(
            verify_export_report(&output_probe(true, "10.000000"), &with_audio).unwrap(),
            10_000_000
        );
        let mut wrong_codec = output_probe(true, "10.000000");
        wrong_codec.streams[0].codec_name = Some("hevc".into());
        assert!(verify_export_report(&wrong_codec, &with_audio).is_err());
        let mut unexpected_stem = output_probe(true, "10.000000");
        unexpected_stem.streams[1].tags.title = Some("Game".into());
        assert!(verify_export_report(&unexpected_stem, &with_audio).is_err());
        let mut wrong_profile = output_probe(true, "10.000000");
        wrong_profile.streams[1].profile = Some("HE-AAC".into());
        assert!(verify_export_report(&wrong_profile, &with_audio).is_err());
        assert!(verify_export_report(&output_probe(true, "10.250000"), &with_audio).is_err());

        let video_only = prepare(
            vec![(0, 10_000_000)],
            vec![audio(2, "Game")],
            &[(2, 100, true, false)],
        );
        assert!(verify_export_report(&output_probe(false, "10.000000"), &video_only).is_ok());
        assert!(verify_export_report(&output_probe(true, "10.000000"), &video_only).is_err());
    }

    #[test]
    fn export_phases_define_single_job_concurrency_and_terminal_recovery() {
        for phase in [
            EditorExportPhase::Preparing,
            EditorExportPhase::Rendering,
            EditorExportPhase::Verifying,
            EditorExportPhase::Finalizing,
        ] {
            assert!(phase.is_active());
        }
        for phase in [
            EditorExportPhase::Idle,
            EditorExportPhase::Complete,
            EditorExportPhase::Failed,
            EditorExportPhase::Cancelled,
        ] {
            assert!(!phase.is_active());
        }
    }
}
