use std::collections::HashSet;
use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use uuid::Uuid;

use crate::clips::ffmpeg::FfmpegExecutable;

use super::models::{
    CacheArtifactState, CacheArtifactStatus, ClipAudioTrack, ClipListItem, PrepareClipMediaResponse,
};

const CACHE_QUEUE_CAPACITY: usize = 128;
const INTERACTIVE_QUEUE_CAPACITY: usize = 16;

#[derive(Clone)]
pub struct MediaCacheManager {
    root: Arc<PathBuf>,
    runtime: Arc<Mutex<CacheRuntime>>,
}

struct CacheRuntime {
    senders: Option<CacheSenders>,
    in_flight: HashSet<JobKey>,
    deleted_clips: HashSet<String>,
}

#[derive(Clone)]
struct CacheSenders {
    interactive: SyncSender<CacheJob>,
    thumbnails: SyncSender<CacheJob>,
}

#[derive(Clone, Debug, Eq, Hash, PartialEq)]
enum JobKey {
    Thumbnail(String),
    Preview(String),
    Audio(String, u32),
    EditorAudio(String, u32),
}

#[derive(Clone)]
enum CacheJob {
    Thumbnail(CacheClip),
    Preview(CacheClip),
    Audio {
        clip: CacheClip,
        track: ClipAudioTrack,
        video_source: PathBuf,
    },
    EditorAudio {
        clip: CacheClip,
        track: ClipAudioTrack,
    },
}

impl CacheJob {
    fn key(&self) -> JobKey {
        match self {
            Self::Thumbnail(clip) => JobKey::Thumbnail(clip.id.clone()),
            Self::Preview(clip) => JobKey::Preview(clip.id.clone()),
            Self::Audio { clip, track, .. } => JobKey::Audio(clip.id.clone(), track.stream_index),
            Self::EditorAudio { clip, track } => {
                JobKey::EditorAudio(clip.id.clone(), track.stream_index)
            }
        }
    }

    fn event_payload(&self, success: bool) -> CacheChangedEvent {
        let (clip_id, kind, stream_index) = match self {
            Self::Thumbnail(clip) => (clip.id.clone(), "thumbnail", None),
            Self::Preview(clip) => (clip.id.clone(), "preview", None),
            Self::Audio { clip, track, .. } => {
                (clip.id.clone(), "audioPreview", Some(track.stream_index))
            }
            Self::EditorAudio { clip, track } => (
                clip.id.clone(),
                "editorAudioPreview",
                Some(track.stream_index),
            ),
        };
        CacheChangedEvent {
            clip_id,
            kind: kind.to_string(),
            stream_index,
            success,
        }
    }

    fn clip_id(&self) -> &str {
        match self {
            Self::Thumbnail(clip) | Self::Preview(clip) => &clip.id,
            Self::Audio { clip, .. } | Self::EditorAudio { clip, .. } => &clip.id,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct CacheChangedEvent {
    clip_id: String,
    kind: String,
    stream_index: Option<u32>,
    success: bool,
}

#[derive(Clone, Debug)]
pub struct CacheClip {
    id: String,
    master_path: PathBuf,
    fingerprint: SourceFingerprint,
    duration_100ns: i64,
    width: u32,
    height: u32,
    fps_numerator: u32,
    fps_denominator: u32,
    video_codec: String,
    audio_tracks: Vec<ClipAudioTrack>,
}

impl CacheClip {
    pub fn from_library(clip: &ClipListItem, master_path: PathBuf) -> Result<Self, String> {
        let metadata = fs::metadata(&master_path)
            .map_err(|error| format!("Could not inspect the clip source: {error}"))?;
        Ok(Self {
            id: normalized_clip_id(&clip.id)?,
            master_path: master_path.clone(),
            fingerprint: SourceFingerprint {
                source_path: master_path.to_string_lossy().into_owned(),
                file_size_bytes: metadata.len(),
                file_modified_at_ms: system_time_ms(
                    metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                ),
            },
            duration_100ns: clip.duration_100ns,
            width: clip.width,
            height: clip.height,
            fps_numerator: clip.fps_numerator,
            fps_denominator: clip.fps_denominator,
            video_codec: clip.video_codec.clone(),
            audio_tracks: clip.audio_tracks.clone(),
        })
    }

    fn combined_track(&self) -> Option<&ClipAudioTrack> {
        self.audio_tracks
            .iter()
            .find(|track| track.role.eq_ignore_ascii_case("Combined"))
            .or_else(|| self.audio_tracks.iter().find(|track| track.is_default))
    }

    pub fn track(&self, stream_index: u32) -> Result<ClipAudioTrack, String> {
        self.audio_tracks
            .iter()
            .find(|track| track.stream_index == stream_index)
            .cloned()
            .ok_or_else(|| {
                format!(
                    "Clip '{}' has no audio stream with index {stream_index}.",
                    self.id
                )
            })
    }

    fn duration_seconds(&self) -> f64 {
        (self.duration_100ns.max(0) as f64) / 10_000_000.0
    }

    fn is_h264(&self) -> bool {
        matches!(
            self.video_codec.trim().to_ascii_lowercase().as_str(),
            "h264" | "avc" | "avc1"
        )
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceFingerprint {
    source_path: String,
    file_size_bytes: u64,
    file_modified_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactMetadata {
    fingerprint: SourceFingerprint,
    generation_duration_ms: f64,
    file_size_bytes: u64,
    bitrate_bps: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ArtifactFailure {
    fingerprint: SourceFingerprint,
    message: String,
}

#[derive(Clone, Debug)]
pub struct FfmpegCachePlan {
    pub arguments: Vec<OsString>,
    pub output_path: PathBuf,
}

impl MediaCacheManager {
    pub fn new(root: PathBuf) -> Self {
        Self {
            root: Arc::new(root),
            runtime: Arc::new(Mutex::new(CacheRuntime {
                senders: None,
                in_flight: HashSet::new(),
                deleted_clips: HashSet::new(),
            })),
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn thumbnail_status(&self, clip: &CacheClip) -> CacheArtifactStatus {
        match self.thumbnail_paths(&clip.id) {
            Ok(paths) => self.artifact_status(
                &paths,
                &clip.fingerprint,
                &JobKey::Thumbnail(clip.id.clone()),
            ),
            Err(error) => cache_error(error),
        }
    }

    pub fn preview_status(&self, clip: &CacheClip) -> CacheArtifactStatus {
        match self.preview_paths(&clip.id) {
            Ok(paths) => {
                self.artifact_status(&paths, &clip.fingerprint, &JobKey::Preview(clip.id.clone()))
            }
            Err(error) => cache_error(error),
        }
    }

    pub fn audio_status(&self, clip: &CacheClip, stream_index: u32) -> CacheArtifactStatus {
        match self.audio_paths(&clip.id, stream_index) {
            Ok(paths) => self.artifact_status(
                &paths,
                &clip.fingerprint,
                &JobKey::Audio(clip.id.clone(), stream_index),
            ),
            Err(error) => cache_error(error),
        }
    }

    pub fn request_thumbnail(
        &self,
        clip: CacheClip,
        retry: bool,
        app: AppHandle,
    ) -> CacheArtifactStatus {
        let paths = match self.thumbnail_paths(&clip.id) {
            Ok(value) => value,
            Err(error) => return cache_error(error),
        };
        self.request_artifact(CacheJob::Thumbnail(clip), paths, retry, app)
    }

    pub fn request_preview(
        &self,
        clip: CacheClip,
        retry: bool,
        app: AppHandle,
    ) -> CacheArtifactStatus {
        let paths = match self.preview_paths(&clip.id) {
            Ok(value) => value,
            Err(error) => return cache_error(error),
        };
        self.request_artifact(CacheJob::Preview(clip), paths, retry, app)
    }

    pub fn request_audio(
        &self,
        clip: CacheClip,
        track: ClipAudioTrack,
        retry: bool,
        app: AppHandle,
    ) -> CacheArtifactStatus {
        let stream_index = track.stream_index;
        let paths = match self.audio_paths(&clip.id, track.stream_index) {
            Ok(value) => value,
            Err(error) => return cache_error(error),
        };
        let current = self.artifact_status(
            &paths,
            &clip.fingerprint,
            &JobKey::Audio(clip.id.clone(), track.stream_index),
        );
        if current.state == CacheArtifactState::Preparing
            || (!retry
                && matches!(
                    current.state,
                    CacheArtifactState::Ready | CacheArtifactState::Error
                ))
        {
            return current;
        }
        if retry {
            remove_if_file(&paths.artifact);
            remove_if_file(&paths.metadata);
            remove_if_file(&paths.failure);
            remove_if_file(&paths.partial);
        }

        let video_source = if clip.is_h264() {
            clip.master_path.clone()
        } else {
            let existing_preview = self.preview_status(&clip);
            let preview = if existing_preview.state == CacheArtifactState::Ready {
                existing_preview
            } else {
                self.request_preview(clip.clone(), retry, app.clone())
            };
            if preview.state != CacheArtifactState::Ready {
                return preview;
            }
            let Some(path) = preview.file_path else {
                return cache_error("The ready H.264 preview did not provide a cache path.");
            };
            PathBuf::from(path)
        };
        self.enqueue(
            CacheJob::Audio {
                clip: clip.clone(),
                track,
                video_source,
            },
            app,
        )
        .map(|_| self.audio_status(&clip, stream_index))
        .unwrap_or_else(cache_error)
    }

    pub fn request_editor_audio(
        &self,
        clip: CacheClip,
        track: ClipAudioTrack,
        retry: bool,
        app: AppHandle,
    ) -> CacheArtifactStatus {
        let paths = match self.editor_audio_paths(&clip.id, track.stream_index) {
            Ok(value) => value,
            Err(error) => return cache_error(error),
        };
        self.request_artifact(CacheJob::EditorAudio { clip, track }, paths, retry, app)
    }

    pub fn cleanup_clip(&self, clip_id: &str) -> Result<(), String> {
        let id = normalized_clip_id(clip_id)?;
        self.lock_runtime().deleted_clips.insert(id.clone());
        let thumbnails = self.thumbnails_root()?;
        for extension in ["jpg", "partial.jpg", "meta.json", "error.json"] {
            remove_if_file(&thumbnails.join(format!("{id}.{extension}")));
        }
        let previews = self.previews_root()?;
        let clip_directory = previews.join(&id);
        if clip_directory.exists() {
            let canonical_previews = previews.canonicalize().map_err(cache_io_error)?;
            let canonical_clip = clip_directory.canonicalize().map_err(cache_io_error)?;
            if canonical_clip.parent() != Some(canonical_previews.as_path()) {
                return Err(
                    "Refused to clean a preview directory outside the owned cache root.".into(),
                );
            }
            fs::remove_dir_all(&canonical_clip).map_err(cache_io_error)?;
        }
        Ok(())
    }

    fn request_artifact(
        &self,
        job: CacheJob,
        paths: ArtifactPaths,
        retry: bool,
        app: AppHandle,
    ) -> CacheArtifactStatus {
        let key = job.key();
        let fingerprint = match &job {
            CacheJob::Thumbnail(clip) | CacheJob::Preview(clip) => &clip.fingerprint,
            CacheJob::Audio { clip, .. } | CacheJob::EditorAudio { clip, .. } => &clip.fingerprint,
        };
        let current = self.artifact_status(&paths, fingerprint, &key);
        if current.state == CacheArtifactState::Preparing
            || (!retry
                && matches!(
                    current.state,
                    CacheArtifactState::Ready | CacheArtifactState::Error
                ))
        {
            return current;
        }
        if retry {
            remove_if_file(&paths.artifact);
            remove_if_file(&paths.metadata);
            remove_if_file(&paths.failure);
            remove_if_file(&paths.partial);
        }
        self.enqueue(job, app)
            .map(|_| CacheArtifactStatus {
                state: CacheArtifactState::Preparing,
                ..Default::default()
            })
            .unwrap_or_else(cache_error)
    }

    fn enqueue(&self, job: CacheJob, app: AppHandle) -> Result<(), String> {
        let key = job.key();
        let mut runtime = self.lock_runtime();
        if runtime.in_flight.contains(&key) {
            return Ok(());
        }
        if runtime.senders.is_none() {
            let (interactive, interactive_receiver) =
                mpsc::sync_channel(INTERACTIVE_QUEUE_CAPACITY);
            let (thumbnails, thumbnail_receiver) = mpsc::sync_channel(CACHE_QUEUE_CAPACITY);
            runtime.senders = Some(CacheSenders {
                interactive,
                thumbnails,
            });
            let manager = self.clone();
            thread::Builder::new()
                .name("justin-replay-media-cache-worker".into())
                .spawn(move || loop {
                    let job = match interactive_receiver.try_recv() {
                        Ok(job) => job,
                        Err(_) => match thumbnail_receiver
                            .recv_timeout(std::time::Duration::from_millis(100))
                        {
                            Ok(job) => job,
                            Err(RecvTimeoutError::Timeout) => continue,
                            Err(RecvTimeoutError::Disconnected) => {
                                match interactive_receiver.recv() {
                                    Ok(job) => job,
                                    Err(_) => break,
                                }
                            }
                        },
                    };
                    let key = job.key();
                    let deleted = manager.lock_runtime().deleted_clips.contains(job.clip_id());
                    let success = !deleted && manager.execute(&job).is_ok();
                    if manager.lock_runtime().deleted_clips.contains(job.clip_id()) {
                        let _ = manager.cleanup_clip(job.clip_id());
                    }
                    let _ = app.emit("clip-media-cache-changed", job.event_payload(success));
                    manager.lock_runtime().in_flight.remove(&key);
                })
                .map_err(|error| format!("Could not start the media cache worker: {error}"))?;
        }
        runtime.in_flight.insert(key.clone());
        let senders = runtime.senders.as_ref().expect("cache senders initialized");
        let sender = match key {
            JobKey::Thumbnail(_) => &senders.thumbnails,
            JobKey::Preview(_) | JobKey::Audio(_, _) | JobKey::EditorAudio(_, _) => {
                &senders.interactive
            }
        };
        match sender.try_send(job) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => {
                runtime.in_flight.remove(&key);
                Err("The preview queue is full. Try again after current previews finish.".into())
            }
            Err(TrySendError::Disconnected(_)) => {
                runtime.in_flight.remove(&key);
                runtime.senders = None;
                Err("The preview worker is unavailable. Retry the request.".into())
            }
        }
    }

    fn execute(&self, job: &CacheJob) -> Result<(), String> {
        let result = match job {
            CacheJob::Thumbnail(clip) => {
                let paths = self.thumbnail_paths(&clip.id)?;
                self.generate(
                    clip,
                    &paths,
                    build_thumbnail_plan(clip, paths.partial.clone()),
                    "generate a clip thumbnail",
                )
            }
            CacheJob::Preview(clip) => {
                let paths = self.preview_paths(&clip.id)?;
                self.generate(
                    clip,
                    &paths,
                    build_preview_plan(clip, paths.partial.clone()),
                    "generate an H.264 preview",
                )
            }
            CacheJob::Audio {
                clip,
                track,
                video_source,
            } => {
                let paths = self.audio_paths(&clip.id, track.stream_index)?;
                self.generate(
                    clip,
                    &paths,
                    build_audio_preview_plan(clip, track, video_source, paths.partial.clone()),
                    "prepare the selected audio track",
                )
            }
            CacheJob::EditorAudio { clip, track } => {
                let paths = self.editor_audio_paths(&clip.id, track.stream_index)?;
                self.generate(
                    clip,
                    &paths,
                    build_editor_audio_plan(clip, track, paths.partial.clone()),
                    "prepare an Editor audio stem",
                )
            }
        };
        if let Err(error) = &result {
            let (clip, paths) = match job {
                CacheJob::Thumbnail(clip) => (clip, self.thumbnail_paths(&clip.id)?),
                CacheJob::Preview(clip) => (clip, self.preview_paths(&clip.id)?),
                CacheJob::Audio { clip, track, .. } => {
                    (clip, self.audio_paths(&clip.id, track.stream_index)?)
                }
                CacheJob::EditorAudio { clip, track } => {
                    (clip, self.editor_audio_paths(&clip.id, track.stream_index)?)
                }
            };
            let failure = ArtifactFailure {
                fingerprint: clip.fingerprint.clone(),
                message: error.clone(),
            };
            let _ = write_json_atomic(&paths.failure, &failure);
            remove_if_file(&paths.partial);
        }
        result
    }

    fn generate(
        &self,
        clip: &CacheClip,
        paths: &ArtifactPaths,
        plan: Result<FfmpegCachePlan, String>,
        description: &str,
    ) -> Result<(), String> {
        let plan = plan?;
        let started = Instant::now();
        remove_if_file(&paths.partial);
        fs::create_dir_all(
            paths
                .artifact
                .parent()
                .ok_or_else(|| "Cache artifact has no parent directory.".to_string())?,
        )
        .map_err(cache_io_error)?;
        FfmpegExecutable::resolve()?.run_cache_arguments(&plan.arguments, description)?;
        let size = fs::metadata(&plan.output_path)
            .map_err(|error| format!("FFmpeg did not create the expected cache artifact: {error}"))?
            .len();
        if size == 0 {
            return Err("FFmpeg created an empty cache artifact.".into());
        }
        remove_if_file(&paths.artifact);
        fs::rename(&plan.output_path, &paths.artifact)
            .map_err(|error| format!("Could not promote the cache artifact atomically: {error}"))?;
        let duration_seconds = clip.duration_seconds();
        let metadata = ArtifactMetadata {
            fingerprint: clip.fingerprint.clone(),
            generation_duration_ms: started.elapsed().as_secs_f64() * 1_000.0,
            file_size_bytes: size,
            bitrate_bps: (duration_seconds > 0.0)
                .then(|| (size as f64 * 8.0 / duration_seconds).round() as u64),
        };
        write_json_atomic(&paths.metadata, &metadata)?;
        remove_if_file(&paths.failure);
        Ok(())
    }

    fn artifact_status(
        &self,
        paths: &ArtifactPaths,
        fingerprint: &SourceFingerprint,
        key: &JobKey,
    ) -> CacheArtifactStatus {
        if let Ok(metadata) = read_json::<ArtifactMetadata>(&paths.metadata) {
            if metadata.fingerprint == *fingerprint && paths.artifact.is_file() {
                return CacheArtifactStatus {
                    state: CacheArtifactState::Ready,
                    file_path: Some(paths.artifact.to_string_lossy().into_owned()),
                    generation_duration_ms: Some(metadata.generation_duration_ms),
                    file_size_bytes: Some(metadata.file_size_bytes),
                    bitrate_bps: metadata.bitrate_bps,
                    error_message: None,
                };
            }
            remove_if_file(&paths.artifact);
            remove_if_file(&paths.metadata);
        }
        if let Ok(failure) = read_json::<ArtifactFailure>(&paths.failure) {
            if failure.fingerprint == *fingerprint {
                return cache_error(failure.message);
            }
            remove_if_file(&paths.failure);
        }
        if self.lock_runtime().in_flight.contains(key) {
            return CacheArtifactStatus {
                state: CacheArtifactState::Preparing,
                ..Default::default()
            };
        }
        CacheArtifactStatus::default()
    }

    fn thumbnails_root(&self) -> Result<PathBuf, String> {
        owned_cache_root(&self.root.join("Thumbnails"))
    }

    fn previews_root(&self) -> Result<PathBuf, String> {
        owned_cache_root(&self.root.join("Previews"))
    }

    fn thumbnail_paths(&self, clip_id: &str) -> Result<ArtifactPaths, String> {
        let id = normalized_clip_id(clip_id)?;
        let root = self.thumbnails_root()?;
        Ok(ArtifactPaths {
            artifact: root.join(format!("{id}.jpg")),
            partial: root.join(format!("{id}.partial.jpg")),
            metadata: root.join(format!("{id}.meta.json")),
            failure: root.join(format!("{id}.error.json")),
        })
    }

    fn preview_paths(&self, clip_id: &str) -> Result<ArtifactPaths, String> {
        let id = normalized_clip_id(clip_id)?;
        let root = self.previews_root()?.join(id);
        fs::create_dir_all(&root).map_err(cache_io_error)?;
        Ok(ArtifactPaths {
            artifact: root.join("combined.preview.mp4"),
            partial: root.join("combined.preview.partial.mp4"),
            metadata: root.join("combined.preview.meta.json"),
            failure: root.join("combined.preview.error.json"),
        })
    }

    fn audio_paths(&self, clip_id: &str, stream_index: u32) -> Result<ArtifactPaths, String> {
        let id = normalized_clip_id(clip_id)?;
        let root = self.previews_root()?.join(id);
        fs::create_dir_all(&root).map_err(cache_io_error)?;
        Ok(ArtifactPaths {
            artifact: root.join(format!("audio-{stream_index}.mp4")),
            partial: root.join(format!("audio-{stream_index}.partial.mp4")),
            metadata: root.join(format!("audio-{stream_index}.meta.json")),
            failure: root.join(format!("audio-{stream_index}.error.json")),
        })
    }

    fn editor_audio_paths(
        &self,
        clip_id: &str,
        stream_index: u32,
    ) -> Result<ArtifactPaths, String> {
        let id = normalized_clip_id(clip_id)?;
        let root = self.previews_root()?.join(id);
        fs::create_dir_all(&root).map_err(cache_io_error)?;
        Ok(ArtifactPaths {
            artifact: root.join(format!("editor-audio-{stream_index}.m4a")),
            partial: root.join(format!("editor-audio-{stream_index}.partial.m4a")),
            metadata: root.join(format!("editor-audio-{stream_index}.meta.json")),
            failure: root.join(format!("editor-audio-{stream_index}.error.json")),
        })
    }

    fn lock_runtime(&self) -> MutexGuard<'_, CacheRuntime> {
        self.runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    fn reserve_for_test(&self, key: JobKey) -> bool {
        self.lock_runtime().in_flight.insert(key)
    }

    #[cfg(test)]
    pub fn preview_artifact_for_test(&self, clip_id: &str) -> PathBuf {
        self.preview_paths(clip_id).unwrap().artifact
    }
}

#[derive(Clone)]
struct ArtifactPaths {
    artifact: PathBuf,
    partial: PathBuf,
    metadata: PathBuf,
    failure: PathBuf,
}

pub fn playback_restore_plan(
    current_time_seconds: f64,
    duration_100ns: i64,
    was_playing: bool,
) -> (f64, bool) {
    let duration = (duration_100ns.max(0) as f64) / 10_000_000.0;
    let current = if current_time_seconds.is_finite() {
        current_time_seconds.max(0.0).min(duration.max(0.0))
    } else {
        0.0
    };
    (current, was_playing)
}

pub fn media_response(
    artifact: CacheArtifactStatus,
    playback_source: &str,
    selected_audio_role: Option<String>,
    current_time_seconds: f64,
    duration_100ns: i64,
    was_playing: bool,
) -> PrepareClipMediaResponse {
    let (restore_at_seconds, resume_playing) =
        playback_restore_plan(current_time_seconds, duration_100ns, was_playing);
    let success = artifact.state != CacheArtifactState::Error;
    let error_message = artifact.error_message.clone();
    PrepareClipMediaResponse {
        success,
        artifact,
        playback_source: Some(playback_source.to_string()),
        selected_audio_role,
        restore_at_seconds,
        resume_playing,
        error_message,
    }
}

fn build_thumbnail_plan(clip: &CacheClip, output_path: PathBuf) -> Result<FfmpegCachePlan, String> {
    let timestamp = thumbnail_timestamp(clip.duration_seconds());
    Ok(FfmpegCachePlan {
        arguments: os_arguments([
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-nostdin".into(),
            "-ss".into(),
            format!("{timestamp:.3}").into(),
            "-i".into(),
            clip.master_path.as_os_str().to_os_string(),
            "-map".into(),
            "0:v:0".into(),
            "-frames:v".into(),
            "1".into(),
            "-vf".into(),
            "scale=640:-2:force_original_aspect_ratio=decrease".into(),
            "-q:v".into(),
            "3".into(),
            "-y".into(),
            output_path.as_os_str().to_os_string(),
        ]),
        output_path,
    })
}

fn build_preview_plan(clip: &CacheClip, output_path: PathBuf) -> Result<FfmpegCachePlan, String> {
    if clip.width == 0 || clip.height == 0 || clip.fps_denominator == 0 {
        return Err("The clip metadata does not contain valid preview dimensions/FPS.".into());
    }
    let fps = (clip.fps_numerator as f64 / clip.fps_denominator as f64).min(60.0);
    if !fps.is_finite() || fps <= 0.0 {
        return Err("The clip metadata contains an invalid frame rate.".into());
    }
    let mut arguments = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
        "-i".into(),
        clip.master_path.as_os_str().to_os_string(),
        "-map".into(),
        "0:v:0".into(),
    ];
    if let Some(track) = clip.combined_track() {
        arguments.extend(["-map".into(), format!("0:{}?", track.stream_index).into()]);
    } else {
        arguments.push("-an".into());
    }
    arguments.extend(os_arguments([
        "-vf".into(),
        "scale=1920:1080:force_original_aspect_ratio=decrease:force_divisible_by=2".into(),
        "-r".into(),
        format!("{fps:.6}").into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-threads".into(),
        "2".into(),
        "-crf".into(),
        "23".into(),
        "-pix_fmt".into(),
        "yuv420p".into(),
        "-profile:v".into(),
        "high".into(),
        "-c:a".into(),
        "aac".into(),
        "-profile:a".into(),
        "aac_low".into(),
        "-b:a".into(),
        "160k".into(),
        "-ar".into(),
        "48000".into(),
        "-ac".into(),
        "2".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-y".into(),
        output_path.as_os_str().to_os_string(),
    ]));
    Ok(FfmpegCachePlan {
        arguments,
        output_path,
    })
}

fn build_audio_preview_plan(
    clip: &CacheClip,
    track: &ClipAudioTrack,
    video_source: &Path,
    output_path: PathBuf,
) -> Result<FfmpegCachePlan, String> {
    let video_is_master = video_source == clip.master_path;
    let audio_input = if video_is_master { 0 } else { 1 };
    let mut arguments = vec![
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
        "-i".into(),
        video_source.as_os_str().to_os_string(),
    ];
    if !video_is_master {
        arguments.extend(["-i".into(), clip.master_path.as_os_str().to_os_string()]);
    }
    arguments.extend(os_arguments([
        "-map".into(),
        "0:v:0".into(),
        "-map".into(),
        format!("{audio_input}:{}", track.stream_index).into(),
        "-c:v".into(),
        "copy".into(),
        "-c:a".into(),
        "aac".into(),
        "-profile:a".into(),
        "aac_low".into(),
        "-b:a".into(),
        "160k".into(),
        "-ar".into(),
        "48000".into(),
        "-ac".into(),
        "2".into(),
        "-metadata:s:a:0".into(),
        format!("title={}", display_audio_role(track)).into(),
        "-disposition:a:0".into(),
        "default".into(),
        "-movflags".into(),
        "+faststart".into(),
        "-y".into(),
        output_path.as_os_str().to_os_string(),
    ]));
    Ok(FfmpegCachePlan {
        arguments,
        output_path,
    })
}

fn build_editor_audio_plan(
    clip: &CacheClip,
    track: &ClipAudioTrack,
    output_path: PathBuf,
) -> Result<FfmpegCachePlan, String> {
    clip.track(track.stream_index)?;
    let mut arguments = os_arguments([
        "-hide_banner".into(),
        "-loglevel".into(),
        "error".into(),
        "-nostdin".into(),
        "-i".into(),
        clip.master_path.as_os_str().to_os_string(),
        "-map".into(),
        format!("0:{}", track.stream_index).into(),
        "-vn".into(),
    ]);
    if track.codec.trim().eq_ignore_ascii_case("aac") {
        arguments.extend(os_arguments(["-c:a".into(), "copy".into()]));
    } else {
        arguments.extend(os_arguments([
            "-c:a".into(),
            "aac".into(),
            "-profile:a".into(),
            "aac_low".into(),
            "-b:a".into(),
            "160k".into(),
            "-ar".into(),
            "48000".into(),
        ]));
    }
    arguments.extend(os_arguments([
        "-metadata:s:a:0".into(),
        format!("title={}", display_audio_role(track)).into(),
        "-movflags".into(),
        "+faststart".into(),
        "-y".into(),
        output_path.as_os_str().to_os_string(),
    ]));
    Ok(FfmpegCachePlan {
        arguments,
        output_path,
    })
}

fn display_audio_role(track: &ClipAudioTrack) -> String {
    match track.role.as_str() {
        "VoiceChat" => "Voice Chat".into(),
        "Microphone" => "Microphone".into(),
        "Unknown" => track
            .title
            .clone()
            .or_else(|| track.handler_name.clone())
            .unwrap_or_else(|| format!("Audio {}", track.stream_index)),
        known => known.to_string(),
    }
}

fn thumbnail_timestamp(duration_seconds: f64) -> f64 {
    if !duration_seconds.is_finite() || duration_seconds <= 0.2 {
        return 0.0;
    }
    (duration_seconds * 0.25)
        .clamp(0.1, 10.0)
        .min(duration_seconds - 0.1)
}

fn normalized_clip_id(value: &str) -> Result<String, String> {
    Uuid::parse_str(value)
        .map(|value| value.to_string())
        .map_err(|_| "The requested Clip ID is not a valid library UUID.".to_string())
}

fn owned_cache_root(path: &Path) -> Result<PathBuf, String> {
    fs::create_dir_all(path).map_err(cache_io_error)?;
    path.canonicalize().map_err(cache_io_error)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, String> {
    let bytes = fs::read(path).map_err(cache_io_error)?;
    serde_json::from_slice(&bytes).map_err(|error| format!("Invalid cache metadata: {error}"))
}

fn write_json_atomic(path: &Path, value: &impl Serialize) -> Result<(), String> {
    let partial = path.with_extension("json.partial");
    let bytes = serde_json::to_vec(value)
        .map_err(|error| format!("Could not serialize cache metadata: {error}"))?;
    fs::write(&partial, bytes).map_err(cache_io_error)?;
    remove_if_file(path);
    fs::rename(&partial, path).map_err(cache_io_error)
}

fn remove_if_file(path: &Path) {
    if path.is_file() {
        let _ = fs::remove_file(path);
    }
}

fn cache_error(error: impl Into<String>) -> CacheArtifactStatus {
    CacheArtifactStatus {
        state: CacheArtifactState::Error,
        error_message: Some(error.into()),
        ..Default::default()
    }
}

fn cache_io_error(error: std::io::Error) -> String {
    format!("Media cache operation failed: {error}")
}

fn system_time_ms(value: SystemTime) -> i64 {
    value
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

fn os_arguments<const N: usize>(values: [OsString; N]) -> Vec<OsString> {
    values.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("stage13-{name}-{}", std::process::id()))
    }

    fn clip(id: &str, master: PathBuf, codec: &str) -> CacheClip {
        CacheClip {
            id: id.into(),
            master_path: master.clone(),
            fingerprint: SourceFingerprint {
                source_path: master.to_string_lossy().into_owned(),
                file_size_bytes: 10,
                file_modified_at_ms: 20,
            },
            duration_100ns: 300_000_000,
            width: 2560,
            height: 1440,
            fps_numerator: 60,
            fps_denominator: 1,
            video_codec: codec.into(),
            audio_tracks: vec![track(1, "Combined"), track(3, "VoiceChat")],
        }
    }

    fn track(index: u32, role: &str) -> ClipAudioTrack {
        ClipAudioTrack {
            stream_index: index,
            role: role.into(),
            title: Some(role.into()),
            handler_name: None,
            codec: "aac".into(),
            profile: Some("LC".into()),
            sample_rate: Some(48_000),
            channels: Some(2),
            bitrate_bps: Some(160_000),
            is_default: role == "Combined",
        }
    }

    fn string_arguments(plan: &FfmpegCachePlan) -> Vec<String> {
        plan.arguments
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn cache_paths_are_uuid_derived_and_contained() {
        let root = root("containment");
        let _ = fs::remove_dir_all(&root);
        let manager = MediaCacheManager::new(root.clone());
        let id = Uuid::new_v4().to_string();
        let thumbnail = manager.thumbnail_paths(&id).unwrap();
        let preview = manager.preview_paths(&id).unwrap();
        let editor_audio = manager.editor_audio_paths(&id, 3).unwrap();
        assert_eq!(
            thumbnail.artifact.parent().unwrap(),
            root.join("Thumbnails").canonicalize().unwrap()
        );
        assert_eq!(
            preview.artifact.parent().unwrap().parent().unwrap(),
            root.join("Previews").canonicalize().unwrap()
        );
        assert_eq!(editor_audio.artifact.parent(), preview.artifact.parent());
        assert_eq!(
            editor_audio.artifact.file_name().unwrap(),
            "editor-audio-3.m4a"
        );
        assert!(manager.thumbnail_paths("../escape").is_err());
        assert!(manager.preview_paths("C:\\escape").is_err());
        assert!(manager.editor_audio_paths("../escape", 3).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn fingerprint_change_invalidates_a_cached_artifact() {
        let root = root("fingerprint");
        let _ = fs::remove_dir_all(&root);
        let manager = MediaCacheManager::new(root.clone());
        let id = Uuid::new_v4().to_string();
        let mut clip = clip(&id, root.join("master.mp4"), "hevc");
        let paths = manager.thumbnail_paths(&id).unwrap();
        fs::write(&paths.artifact, b"jpeg").unwrap();
        write_json_atomic(
            &paths.metadata,
            &ArtifactMetadata {
                fingerprint: clip.fingerprint.clone(),
                generation_duration_ms: 1.0,
                file_size_bytes: 4,
                bitrate_bps: None,
            },
        )
        .unwrap();
        assert_eq!(
            manager.thumbnail_status(&clip).state,
            CacheArtifactState::Ready
        );
        clip.fingerprint.file_size_bytes += 1;
        assert_eq!(
            manager.thumbnail_status(&clip).state,
            CacheArtifactState::Missing
        );
        assert!(!paths.artifact.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn same_preview_and_thumbnail_requests_reserve_only_once() {
        let manager = MediaCacheManager::new(root("dedupe"));
        let id = Uuid::new_v4().to_string();
        assert!(manager.reserve_for_test(JobKey::Preview(id.clone())));
        assert!(!manager.reserve_for_test(JobKey::Preview(id.clone())));
        assert!(manager.reserve_for_test(JobKey::Thumbnail(id.clone())));
        assert!(!manager.reserve_for_test(JobKey::Thumbnail(id.clone())));
        assert!(manager.reserve_for_test(JobKey::EditorAudio(id.clone(), 3)));
        assert!(!manager.reserve_for_test(JobKey::EditorAudio(id.clone(), 3)));
        assert!(manager.reserve_for_test(JobKey::EditorAudio(id, 4)));
    }

    #[test]
    fn hevc_preview_plan_is_h264_1080p_and_h264_master_audio_plan_copies_video() {
        let id = Uuid::new_v4().to_string();
        let hevc = clip(&id, PathBuf::from("master.mp4"), "hevc");
        let preview = string_arguments(
            &build_preview_plan(&hevc, PathBuf::from("preview.partial.mp4")).unwrap(),
        );
        assert!(preview.windows(2).any(|pair| pair == ["-c:v", "libx264"]));
        assert!(preview
            .iter()
            .any(|value| value.contains("scale=1920:1080")));
        assert!(preview.windows(2).any(|pair| pair == ["-map", "0:1?"]));

        let h264 = clip(&id, PathBuf::from("master.mp4"), "h264");
        assert!(h264.is_h264());
        let audio = string_arguments(
            &build_audio_preview_plan(
                &h264,
                &track(3, "VoiceChat"),
                Path::new("master.mp4"),
                PathBuf::from("audio.partial.mp4"),
            )
            .unwrap(),
        );
        assert!(audio.windows(2).any(|pair| pair == ["-c:v", "copy"]));
        assert!(audio.windows(2).any(|pair| pair == ["-map", "0:3"]));
    }

    #[test]
    fn alternate_audio_mapping_uses_master_stream_and_preserves_unknown_label() {
        let id = Uuid::new_v4().to_string();
        let hevc = clip(&id, PathBuf::from("master.mp4"), "hevc");
        let mut unknown = track(7, "Unknown");
        unknown.title = Some("Commentary".into());
        let plan = string_arguments(
            &build_audio_preview_plan(
                &hevc,
                &unknown,
                Path::new("h264-preview.mp4"),
                PathBuf::from("audio.partial.mp4"),
            )
            .unwrap(),
        );
        assert!(plan.windows(2).any(|pair| pair == ["-map", "1:7"]));
        assert!(plan.iter().any(|value| value == "title=Commentary"));
        assert_eq!(hevc.track(3).unwrap().role, "VoiceChat");
        assert!(hevc.track(99).is_err());
    }

    #[test]
    fn editor_audio_plan_stream_copies_aac_and_reencodes_other_codecs() {
        let id = Uuid::new_v4().to_string();
        let mut source = clip(&id, PathBuf::from("master.mp4"), "hevc");
        let aac = track(3, "VoiceChat");
        let copied = string_arguments(
            &build_editor_audio_plan(&source, &aac, PathBuf::from("editor-audio-3.partial.m4a"))
                .unwrap(),
        );
        assert!(copied.windows(2).any(|pair| pair == ["-map", "0:3"]));
        assert!(copied.windows(2).any(|pair| pair == ["-c:a", "copy"]));
        assert!(copied.iter().any(|value| value == "-vn"));
        assert_eq!(copied.last().unwrap(), "editor-audio-3.partial.m4a");

        let mut opus = track(3, "VoiceChat");
        opus.codec = "opus".into();
        source.audio_tracks = vec![opus.clone()];
        let encoded = string_arguments(
            &build_editor_audio_plan(&source, &opus, PathBuf::from("editor-audio-3.partial.m4a"))
                .unwrap(),
        );
        assert!(encoded.windows(2).any(|pair| pair == ["-c:a", "aac"]));
        assert!(encoded.windows(2).any(|pair| pair == ["-ar", "48000"]));
        assert!(build_editor_audio_plan(
            &source,
            &track(99, "Unknown"),
            PathBuf::from("missing.m4a")
        )
        .is_err());
    }

    #[test]
    fn editor_audio_fingerprint_invalidation_is_stream_specific() {
        let root = root("editor-audio-fingerprint");
        let _ = fs::remove_dir_all(&root);
        let manager = MediaCacheManager::new(root.clone());
        let id = Uuid::new_v4().to_string();
        let mut source = clip(&id, root.join("master.mp4"), "hevc");
        let first = manager.editor_audio_paths(&id, 3).unwrap();
        let second = manager.editor_audio_paths(&id, 4).unwrap();
        assert_ne!(first.artifact, second.artifact);
        fs::write(&first.artifact, b"m4a").unwrap();
        write_json_atomic(
            &first.metadata,
            &ArtifactMetadata {
                fingerprint: source.fingerprint.clone(),
                generation_duration_ms: 1.0,
                file_size_bytes: 3,
                bitrate_bps: Some(160_000),
            },
        )
        .unwrap();
        assert_eq!(
            manager
                .artifact_status(
                    &first,
                    &source.fingerprint,
                    &JobKey::EditorAudio(id.clone(), 3)
                )
                .state,
            CacheArtifactState::Ready
        );
        source.fingerprint.file_modified_at_ms += 1;
        assert_eq!(
            manager
                .artifact_status(&first, &source.fingerprint, &JobKey::EditorAudio(id, 3))
                .state,
            CacheArtifactState::Missing
        );
        assert!(!first.artifact.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn thumbnail_plan_uses_a_representative_bounded_frame_and_safe_output_argument() {
        let id = Uuid::new_v4().to_string();
        let clip = clip(&id, PathBuf::from("master.mp4"), "hevc");
        let output = PathBuf::from("thumbnail.partial.jpg");
        let plan = string_arguments(&build_thumbnail_plan(&clip, output.clone()).unwrap());
        assert!(plan.windows(2).any(|pair| pair == ["-ss", "7.500"]));
        assert!(plan.windows(2).any(|pair| pair == ["-frames:v", "1"]));
        assert_eq!(plan.last().unwrap(), &output.to_string_lossy());
    }

    #[test]
    fn current_time_restoration_is_clamped_and_keeps_play_state() {
        assert_eq!(playback_restore_plan(12.5, 300_000_000, true), (12.5, true));
        assert_eq!(
            playback_restore_plan(99.0, 300_000_000, false),
            (30.0, false)
        );
        assert_eq!(
            playback_restore_plan(f64::NAN, 300_000_000, true),
            (0.0, true)
        );
    }

    #[test]
    fn cleanup_rejects_non_uuid_and_removes_only_owned_clip_cache() {
        let root = root("cleanup");
        let _ = fs::remove_dir_all(&root);
        let manager = MediaCacheManager::new(root.clone());
        let id = Uuid::new_v4().to_string();
        let paths = manager.preview_paths(&id).unwrap();
        let editor_audio = manager.editor_audio_paths(&id, 3).unwrap();
        fs::write(&paths.artifact, b"preview").unwrap();
        fs::write(&editor_audio.artifact, b"audio").unwrap();
        let outside = root.with_file_name("stage13-outside-sentinel");
        fs::write(&outside, b"keep").unwrap();
        assert!(manager.cleanup_clip("../outside").is_err());
        manager.cleanup_clip(&id).unwrap();
        assert!(!paths.artifact.exists());
        assert!(!editor_audio.artifact.exists());
        assert!(outside.exists());
        fs::remove_dir_all(root).unwrap();
        fs::remove_file(outside).unwrap();
    }
}
