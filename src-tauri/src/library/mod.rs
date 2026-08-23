pub(crate) mod clipboard;
mod database;
pub(crate) mod export;
mod media;
mod migrations;
mod models;
mod reconcile;
mod repository;
mod safety;
mod storage;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

pub use export::EditorExportManager;
pub use models::{
    ClipActionResponse, ClipAudioTrack, ClipIdRequest, ClipListRequest, ClipListResponse,
    ClipMutationResponse, ClipPlaybackInfo, ClipPlaybackInfoResponse, CollectionIdRequest,
    CollectionMutationResponse, CollectionsResponse, CreateCollectionRequest, LibraryTelemetry,
    PrepareClipAudioRequest, PrepareClipMediaRequest, PrepareClipMediaResponse, ReconcileResponse,
    RenameClipRequest, RenameCollectionRequest, SetClipCollectionRequest, SetFavoriteRequest,
    SetPinnedRequest, StorageCleanupExecuteRequest, StorageCleanupExecutionResponse,
    StorageCleanupPreviewRequest, StorageCleanupPreviewResponse,
};

use database::LibraryDatabase;
use media::{media_response, CacheClip, MediaCacheManager};
use models::{ClipListItem, ClipUpsert, ReconciliationTelemetry};
use reconcile::{reconcile, FfprobeMediaInspector};
use repository::{
    cleanup_candidates, count_clips, create_collection, delete_clip_row, delete_collection,
    get_clip, library_summary, list_clips as query_clips, list_collections, record_clip_watch,
    rename_collection, rename_display_name, set_clip_collection, set_favorite, set_pinned,
    upsert_clip,
};
use safety::validate_owned_clip;
use storage::{build_cleanup_preview, same_cleanup_scope, StoredCleanupPlan};

#[derive(Clone, Debug)]
pub struct SavedClipMetadata {
    pub file_path: PathBuf,
    pub created_at_ms: i64,
    pub duration_100ns: i64,
    pub requested_duration_seconds: u32,
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub video_codec: String,
    pub video_profile: Option<String>,
    pub video_bitrate_bps: Option<u64>,
    pub total_bitrate_bps: Option<u64>,
    pub capture_target_label: Option<String>,
    pub capture_target_type: Option<String>,
    pub audio_tracks: Vec<ClipAudioTrack>,
}

#[derive(Clone, Debug)]
pub struct SavedClipIndexResult {
    pub clip_id: String,
    pub insertion_ms: f64,
}

#[derive(Clone)]
pub struct ClipLibraryManager {
    database: Option<LibraryDatabase>,
    clips_root: Arc<PathBuf>,
    initialization_error: Arc<Option<String>>,
    telemetry: Arc<Mutex<LibraryTelemetry>>,
    reconciliation_running: Arc<AtomicBool>,
    media_cache: MediaCacheManager,
    pending_cleanup: Arc<Mutex<Option<StoredCleanupPlan>>>,
    cleanup_execution: Arc<Mutex<()>>,
}

impl ClipLibraryManager {
    pub fn initialize(database_path: PathBuf, clips_root: PathBuf) -> Self {
        let cache_root = database_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| database_path.with_extension("cache"));
        let database_result = LibraryDatabase::initialize(database_path.clone());
        let (database, schema_version, initialization_error) = match database_result {
            Ok((database, version)) => (Some(database), version, None),
            Err(error) => (None, 0, Some(error)),
        };
        let manager = Self {
            database,
            clips_root: Arc::new(clips_root),
            initialization_error: Arc::new(initialization_error),
            telemetry: Arc::new(Mutex::new(LibraryTelemetry {
                database_path: database_path.to_string_lossy().into_owned(),
                schema_version,
                ..Default::default()
            })),
            reconciliation_running: Arc::new(AtomicBool::new(false)),
            media_cache: MediaCacheManager::new(cache_root),
            pending_cleanup: Arc::new(Mutex::new(None)),
            cleanup_execution: Arc::new(Mutex::new(())),
        };
        manager.refresh_indexed_count();
        manager
    }

    pub fn start_initial_reconciliation(&self, app: AppHandle) {
        let manager = self.clone();
        let _ = thread::Builder::new()
            .name("justin-replay-library-initial-reconcile".into())
            .spawn(move || {
                let result = manager.reconcile_now();
                let _ = app.emit(
                    "clip-library-changed",
                    if result.is_ok() {
                        "initial-reconciliation-complete"
                    } else {
                        "initial-reconciliation-failed"
                    },
                );
            });
    }

    pub fn index_saved_clip(
        &self,
        metadata: SavedClipMetadata,
    ) -> Result<SavedClipIndexResult, String> {
        self.index_clip(metadata, None)
    }

    pub(crate) fn index_exported_clip(
        &self,
        metadata: SavedClipMetadata,
        display_name: String,
    ) -> Result<SavedClipIndexResult, String> {
        self.index_clip(metadata, Some(display_name))
    }

    fn index_clip(
        &self,
        metadata: SavedClipMetadata,
        display_name_override: Option<String>,
    ) -> Result<SavedClipIndexResult, String> {
        let started = Instant::now();
        let database = self.database()?;
        let canonical = validate_owned_clip(&self.clips_root, &metadata.file_path)?;
        let file_metadata = std::fs::metadata(&canonical).map_err(|error| {
            format!("Could not inspect the newly saved clip for indexing: {error}")
        })?;
        let filename = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .ok_or_else(|| "The saved clip filename is not valid Unicode.".to_string())?
            .to_string();
        let display_name = display_name_override.unwrap_or_else(|| {
            canonical
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("Replay")
                .to_string()
        });
        let upsert = ClipUpsert {
            id: Uuid::new_v4().to_string(),
            file_path: canonical.to_string_lossy().into_owned(),
            filename,
            display_name,
            created_at_ms: metadata.created_at_ms,
            library_added_at_ms: now_ms(),
            file_modified_at_ms: system_time_ms(
                file_metadata.modified().unwrap_or(SystemTime::now()),
            ),
            file_size_bytes: file_metadata.len(),
            duration_100ns: metadata.duration_100ns,
            requested_duration_seconds: Some(metadata.requested_duration_seconds),
            width: metadata.width,
            height: metadata.height,
            fps_numerator: metadata.fps_numerator,
            fps_denominator: metadata.fps_denominator,
            video_codec: metadata.video_codec,
            video_profile: metadata.video_profile,
            video_bitrate_bps: metadata.video_bitrate_bps,
            total_bitrate_bps: metadata.total_bitrate_bps,
            capture_target_label: metadata.capture_target_label,
            capture_target_type: metadata.capture_target_type,
            imported_existing_file: false,
            audio_tracks: metadata.audio_tracks,
        };
        let clip_id = upsert_clip(&mut database.open()?, &upsert)?;
        let insertion_ms = started.elapsed().as_secs_f64() * 1_000.0;
        {
            let mut telemetry = self.lock_telemetry();
            telemetry.newest_saved_clip_id = Some(clip_id.clone());
            telemetry.newest_saved_clip_indexed = Some(true);
            telemetry.newest_saved_clip_insertion_ms = Some(insertion_ms);
        }
        self.refresh_indexed_count();
        Ok(SavedClipIndexResult {
            clip_id,
            insertion_ms,
        })
    }

    pub fn record_saved_clip_index_failure(&self, elapsed_ms: f64) {
        let mut telemetry = self.lock_telemetry();
        telemetry.newest_saved_clip_id = None;
        telemetry.newest_saved_clip_indexed = Some(false);
        telemetry.newest_saved_clip_insertion_ms = Some(elapsed_ms);
    }

    fn list(&self, request: ClipListRequest) -> ClipListResponse {
        let started = Instant::now();
        let result = self.database().and_then(|database| {
            let connection = database.open()?;
            let (clips, total_count) = query_clips(&connection, request)?;
            Ok((clips, total_count, library_summary(&connection)?))
        });
        let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
        self.lock_telemetry().last_list_query_duration_ms = Some(elapsed);
        match result {
            Ok((clips, total_count, summary)) => ClipListResponse {
                success: true,
                clips,
                total_count,
                summary: Some(summary),
                telemetry: self.telemetry_snapshot(),
                error_message: None,
            },
            Err(error) => ClipListResponse {
                success: false,
                clips: Vec::new(),
                total_count: 0,
                summary: None,
                telemetry: self.telemetry_snapshot(),
                error_message: Some(error),
            },
        }
    }

    fn reconcile_response(&self) -> ReconcileResponse {
        match self.reconcile_now() {
            Ok(result) => ReconcileResponse {
                success: true,
                result: Some(result),
                telemetry: self.telemetry_snapshot(),
                error_message: None,
            },
            Err(error) => ReconcileResponse {
                success: false,
                result: None,
                telemetry: self.telemetry_snapshot(),
                error_message: Some(error),
            },
        }
    }

    fn reconcile_now(&self) -> Result<ReconciliationTelemetry, String> {
        if self
            .reconciliation_running
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err("A Clips reconciliation is already running.".into());
        }
        struct Reset<'a>(&'a AtomicBool);
        impl Drop for Reset<'_> {
            fn drop(&mut self) {
                self.0.store(false, Ordering::Release);
            }
        }
        let _reset = Reset(&self.reconciliation_running);
        let database = self.database()?;
        let result = reconcile(database, &self.clips_root, &FfprobeMediaInspector)?;
        self.lock_telemetry().last_reconciliation = Some(result.clone());
        self.refresh_indexed_count();
        Ok(result)
    }

    fn mutate_favorite(&self, request: SetFavoriteRequest) -> ClipMutationResponse {
        self.mutate_clip(&request.clip_id, |connection| {
            set_favorite(connection, &request.clip_id, request.favorite)
        })
    }

    fn mutate_pinned(&self, request: SetPinnedRequest) -> ClipMutationResponse {
        let _cleanup_guard = self
            .cleanup_execution
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *self
            .pending_cleanup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        self.mutate_clip(&request.clip_id, |connection| {
            set_pinned(connection, &request.clip_id, request.pinned)
        })
    }

    fn preview_storage_cleanup(
        &self,
        request: StorageCleanupPreviewRequest,
    ) -> StorageCleanupPreviewResponse {
        *self
            .pending_cleanup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        let result = (|| {
            let database = self.database()?;
            let connection = database.open()?;
            let summary = library_summary(&connection)?;
            let candidates = cleanup_candidates(&connection)?;
            let (preview, plan) = build_cleanup_preview(request.quota_bytes, &summary, candidates)?;
            for candidate in &preview.candidates {
                let (clip, _) = self.resolved_clip(&candidate.clip_id)?;
                if clip.pinned {
                    return Err(format!(
                        "Protected clip '{}' entered a cleanup preview.",
                        clip.display_name
                    ));
                }
            }
            *self
                .pending_cleanup
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()) = plan;
            Ok(preview)
        })();
        result.unwrap_or_else(|error| StorageCleanupPreviewResponse {
            success: false,
            plan_id: None,
            quota_bytes: request.quota_bytes,
            total_size_bytes: 0,
            bytes_over_quota: 0,
            planned_reclaim_bytes: 0,
            remaining_size_bytes: 0,
            protected_count: 0,
            protected_size_bytes: 0,
            can_meet_quota: false,
            candidates: Vec::new(),
            error_message: Some(error),
        })
    }

    fn execute_storage_cleanup(
        &self,
        request: StorageCleanupExecuteRequest,
    ) -> StorageCleanupExecutionResponse {
        let _cleanup_guard = self
            .cleanup_execution
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let stored = self
            .pending_cleanup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        let mut deleted_count = 0_u64;
        let mut deleted_bytes = 0_u64;
        let result = (|| {
            let stored = stored
                .ok_or_else(|| "The cleanup preview expired. Preview cleanup again.".to_string())?;
            if stored.plan_id != request.plan_id {
                return Err("The cleanup preview token is invalid or expired.".into());
            }
            let database = self.database()?;
            let connection = database.open()?;
            let summary = library_summary(&connection)?;
            let candidates = cleanup_candidates(&connection)?;
            let (_, current) = build_cleanup_preview(stored.quota_bytes, &summary, candidates)?;
            let current = current
                .ok_or_else(|| "The Library no longer needs cleanup. Preview again.".to_string())?;
            if !same_cleanup_scope(&stored, &current) {
                return Err("The Library changed after the preview. No clips were deleted; preview cleanup again.".into());
            }
            for (clip_id, _) in &stored.candidates {
                let (clip, _) = self.resolved_clip(clip_id)?;
                if clip.pinned {
                    return Err("A clip became protected after the preview. No clips were deleted; preview again.".into());
                }
            }
            for (clip_id, size) in &stored.candidates {
                self.delete_internal(clip_id, true)?;
                deleted_count += 1;
                deleted_bytes = deleted_bytes.saturating_add(*size);
            }
            let remaining_size_bytes = library_summary(&database.open()?)?.total_size_bytes;
            Ok(remaining_size_bytes)
        })();
        match result {
            Ok(remaining_size_bytes) => StorageCleanupExecutionResponse {
                success: true,
                deleted_count,
                deleted_bytes,
                remaining_size_bytes,
                error_message: None,
            },
            Err(error) => {
                let remaining_size_bytes = self
                    .database()
                    .and_then(|database| library_summary(&database.open()?))
                    .map(|summary| summary.total_size_bytes)
                    .unwrap_or(0);
                StorageCleanupExecutionResponse {
                    success: false,
                    deleted_count,
                    deleted_bytes,
                    remaining_size_bytes,
                    error_message: Some(if deleted_count == 0 {
                        error
                    } else {
                        format!("Cleanup stopped after deleting {deleted_count} clip(s): {error}")
                    }),
                }
            }
        }
    }

    fn record_watch(&self, clip_id: &str) -> ClipMutationResponse {
        let result = self
            .database()
            .and_then(|database| record_clip_watch(&database.open()?, clip_id, now_ms()));
        mutation_result(result)
    }

    fn collections(&self) -> CollectionsResponse {
        match self
            .database()
            .and_then(|database| list_collections(&database.open()?))
        {
            Ok(collections) => CollectionsResponse {
                success: true,
                collections,
                error_message: None,
            },
            Err(error) => CollectionsResponse {
                success: false,
                collections: Vec::new(),
                error_message: Some(error),
            },
        }
    }

    fn create_collection(&self, request: CreateCollectionRequest) -> CollectionMutationResponse {
        let result = validate_collection_name(&request.name).and_then(|name| {
            let database = self.database()?;
            create_collection(
                &database.open()?,
                &Uuid::new_v4().to_string(),
                &name,
                now_ms(),
            )
        });
        collection_mutation_result(result)
    }

    fn rename_collection(&self, request: RenameCollectionRequest) -> CollectionMutationResponse {
        let result = validate_collection_name(&request.name).and_then(|name| {
            let database = self.database()?;
            rename_collection(&database.open()?, &request.collection_id, &name, now_ms())
        });
        collection_mutation_result(result)
    }

    fn delete_collection(&self, collection_id: &str) -> ClipActionResponse {
        action_result(
            self.database()
                .and_then(|database| delete_collection(&database.open()?, collection_id)),
        )
    }

    fn set_collection_membership(&self, request: SetClipCollectionRequest) -> ClipMutationResponse {
        let result = self.database().and_then(|database| {
            set_clip_collection(
                &database.open()?,
                &request.clip_id,
                &request.collection_id,
                request.included,
                now_ms(),
            )
        });
        mutation_result(result)
    }

    fn rename(&self, request: RenameClipRequest) -> ClipMutationResponse {
        let name = request.display_name.trim();
        if name.chars().count() > 120 {
            return mutation_error("Clip display names may not exceed 120 characters.");
        }
        let result = (|| {
            let database = self.database()?;
            let connection = database.open()?;
            let clip = get_clip(&connection, &request.clip_id)?
                .ok_or_else(|| format!("No library clip exists with ID '{}'.", request.clip_id))?;
            let resolved = if name.is_empty() {
                Path::new(&clip.filename)
                    .file_stem()
                    .and_then(|value| value.to_str())
                    .unwrap_or(&clip.filename)
                    .to_string()
            } else {
                name.to_string()
            };
            rename_display_name(&connection, &request.clip_id, &resolved)?;
            get_clip(&connection, &request.clip_id)?
                .ok_or_else(|| "The renamed clip disappeared from the library.".to_string())
        })();
        mutation_result(result)
    }

    fn mutate_clip(
        &self,
        clip_id: &str,
        mutation: impl FnOnce(&rusqlite::Connection) -> Result<(), String>,
    ) -> ClipMutationResponse {
        let result = (|| {
            let database = self.database()?;
            let connection = database.open()?;
            mutation(&connection)?;
            get_clip(&connection, clip_id)?
                .ok_or_else(|| "The updated clip disappeared from the library.".to_string())
        })();
        mutation_result(result)
    }

    fn trusted_clip_path(&self, clip_id: &str) -> Result<PathBuf, String> {
        self.resolved_clip(clip_id).map(|(_, path)| path)
    }

    pub(crate) fn resolved_clip(&self, clip_id: &str) -> Result<(ClipListItem, PathBuf), String> {
        let database = self.database()?;
        let connection = database.open()?;
        let clip = get_clip(&connection, clip_id)?
            .ok_or_else(|| format!("No library clip exists with ID '{clip_id}'."))?;
        let path = validate_owned_clip(&self.clips_root, Path::new(&clip.file_path))?;
        Ok((clip, path))
    }

    pub(crate) fn clip_by_id(&self, clip_id: &str) -> Result<Option<ClipListItem>, String> {
        let database = self.database()?;
        get_clip(&database.open()?, clip_id)
    }

    fn cache_clip(&self, clip_id: &str) -> Result<(ClipListItem, CacheClip), String> {
        let (clip, path) = self.resolved_clip(clip_id)?;
        let cache_clip = CacheClip::from_library(&clip, path)?;
        Ok((clip, cache_clip))
    }

    fn playback_info(&self, clip_id: &str) -> ClipPlaybackInfoResponse {
        match self.cache_clip(clip_id) {
            Ok((clip, cache_clip)) => ClipPlaybackInfoResponse {
                success: true,
                info: Some(ClipPlaybackInfo {
                    clip_id: clip.id,
                    display_name: clip.display_name,
                    master_path: clip.file_path,
                    master_codec: clip.video_codec,
                    width: clip.width,
                    height: clip.height,
                    duration_100ns: clip.duration_100ns,
                    audio_tracks: clip.audio_tracks,
                    cache_root: self.media_cache.root().to_string_lossy().into_owned(),
                    preview: self.media_cache.preview_status(&cache_clip),
                    thumbnail: self.media_cache.thumbnail_status(&cache_clip),
                }),
                error_message: None,
            },
            Err(error) => ClipPlaybackInfoResponse {
                success: false,
                info: None,
                error_message: Some(error),
            },
        }
    }

    fn request_thumbnail(
        &self,
        request: PrepareClipMediaRequest,
        app: AppHandle,
    ) -> PrepareClipMediaResponse {
        match self.cache_clip(&request.clip_id) {
            Ok((clip, cache_clip)) => media_response(
                self.media_cache
                    .request_thumbnail(cache_clip, request.retry, app),
                "Thumbnail",
                None,
                request.current_time_seconds,
                clip.duration_100ns,
                request.was_playing,
            ),
            Err(error) => media_command_error(error),
        }
    }

    fn prepare_preview(
        &self,
        request: PrepareClipMediaRequest,
        app: AppHandle,
    ) -> PrepareClipMediaResponse {
        match self.cache_clip(&request.clip_id) {
            Ok((clip, cache_clip)) => media_response(
                self.media_cache
                    .request_preview(cache_clip, request.retry, app),
                "H264 Proxy",
                Some("Combined".into()),
                request.current_time_seconds,
                clip.duration_100ns,
                request.was_playing,
            ),
            Err(error) => media_command_error(error),
        }
    }

    fn prepare_audio(
        &self,
        request: PrepareClipAudioRequest,
        app: AppHandle,
    ) -> PrepareClipMediaResponse {
        match self
            .cache_clip(&request.clip_id)
            .and_then(|(clip, cache_clip)| {
                let track = cache_clip.track(request.stream_index)?;
                Ok((clip, cache_clip, track))
            }) {
            Ok((clip, cache_clip, track)) => {
                let role = track.role.clone();
                media_response(
                    self.media_cache
                        .request_audio(cache_clip, track, request.retry, app),
                    "Audio Preview",
                    Some(role),
                    request.current_time_seconds,
                    clip.duration_100ns,
                    request.was_playing,
                )
            }
            Err(error) => media_command_error(error),
        }
    }

    fn prepare_editor_audio(
        &self,
        request: PrepareClipAudioRequest,
        app: AppHandle,
    ) -> PrepareClipMediaResponse {
        match self
            .cache_clip(&request.clip_id)
            .and_then(|(clip, cache_clip)| {
                let track = cache_clip.track(request.stream_index)?;
                Ok((clip, cache_clip, track))
            }) {
            Ok((clip, cache_clip, track)) => {
                let role = track.role.clone();
                media_response(
                    self.media_cache
                        .request_editor_audio(cache_clip, track, request.retry, app),
                    "Editor Audio Stem",
                    Some(role),
                    request.current_time_seconds,
                    clip.duration_100ns,
                    request.was_playing,
                )
            }
            Err(error) => media_command_error(error),
        }
    }

    fn open_clip(&self, clip_id: &str) -> ClipActionResponse {
        action_result((|| {
            let path = self.trusted_clip_path(clip_id)?;
            tauri_plugin_opener::open_path(path, None::<&str>)
                .map_err(|error| format!("Windows could not open the clip: {error}"))
        })())
    }

    fn open_folder(&self, clip_id: &str) -> ClipActionResponse {
        action_result((|| {
            let path = self.trusted_clip_path(clip_id)?;
            tauri_plugin_opener::reveal_item_in_dir(path)
                .map_err(|error| format!("Windows could not reveal the clip: {error}"))
        })())
    }

    fn delete(&self, clip_id: &str) -> ClipActionResponse {
        let _cleanup_guard = self
            .cleanup_execution
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *self
            .pending_cleanup
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
        action_result(self.delete_internal(clip_id, false))
    }

    fn delete_internal(&self, clip_id: &str, require_unprotected: bool) -> Result<(), String> {
        let (clip, path) = self.resolved_clip(clip_id)?;
        if require_unprotected && clip.pinned {
            return Err(format!(
                "Refused to clean up protected clip '{}'.",
                clip.display_name
            ));
        }
        std::fs::remove_file(&path).map_err(|error| {
            format!(
                "Could not permanently delete clip '{}': {error}",
                path.display()
            )
        })?;
        let database = self.database()?;
        delete_clip_row(&mut database.open()?, clip_id).map_err(|error| {
                format!(
                    "The MP4 was deleted, but its database row could not be removed: {error}. Refresh Clips to reconcile it."
                )
            })?;
        let _ = self.media_cache.cleanup_clip(clip_id);
        self.refresh_indexed_count();
        Ok(())
    }

    fn database(&self) -> Result<&LibraryDatabase, String> {
        self.database.as_ref().ok_or_else(|| {
            self.initialization_error
                .as_ref()
                .clone()
                .unwrap_or_else(|| "The Clips library is unavailable.".into())
        })
    }

    fn refresh_indexed_count(&self) {
        if let Ok(database) = self.database() {
            if let Ok(connection) = database.open() {
                if let Ok(count) = count_clips(&connection) {
                    self.lock_telemetry().indexed_clip_count = count;
                }
            }
        }
    }

    fn telemetry_snapshot(&self) -> LibraryTelemetry {
        let mut value = self.lock_telemetry().clone();
        value.reconciliation_running = self.reconciliation_running.load(Ordering::Acquire);
        value
    }

    fn lock_telemetry(&self) -> MutexGuard<'_, LibraryTelemetry> {
        self.telemetry
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    pub fn database_path(&self) -> Option<&Path> {
        self.database.as_ref().map(LibraryDatabase::path)
    }
}

fn mutation_result(result: Result<ClipListItem, String>) -> ClipMutationResponse {
    match result {
        Ok(clip) => ClipMutationResponse {
            success: true,
            clip: Some(clip),
            error_message: None,
        },
        Err(error) => mutation_error(error),
    }
}

fn mutation_error(error: impl Into<String>) -> ClipMutationResponse {
    ClipMutationResponse {
        success: false,
        clip: None,
        error_message: Some(error.into()),
    }
}

fn collection_mutation_result(
    result: Result<models::CollectionSummary, String>,
) -> CollectionMutationResponse {
    match result {
        Ok(collection) => CollectionMutationResponse {
            success: true,
            collection: Some(collection),
            error_message: None,
        },
        Err(error) => CollectionMutationResponse {
            success: false,
            collection: None,
            error_message: Some(error),
        },
    }
}

fn validate_collection_name(value: &str) -> Result<String, String> {
    let name = value.trim();
    if name.is_empty() {
        return Err("Collection names cannot be empty.".to_string());
    }
    if name.chars().count() > 80 {
        return Err("Collection names may not exceed 80 characters.".to_string());
    }
    Ok(name.to_string())
}

#[cfg(test)]
mod stage18_tests {
    use super::validate_collection_name;

    #[test]
    fn collection_names_are_trimmed_unicode_and_bounded() {
        assert_eq!(
            validate_collection_name("  Funny 雪  ").unwrap(),
            "Funny 雪"
        );
        assert!(validate_collection_name("   ").is_err());
        assert!(validate_collection_name(&"x".repeat(81)).is_err());
    }
}

fn action_result(result: Result<(), String>) -> ClipActionResponse {
    match result {
        Ok(()) => ClipActionResponse {
            success: true,
            error_message: None,
        },
        Err(error) => ClipActionResponse {
            success: false,
            error_message: Some(error),
        },
    }
}

fn media_command_error(error: impl Into<String>) -> PrepareClipMediaResponse {
    let error = error.into();
    PrepareClipMediaResponse {
        success: false,
        artifact: models::CacheArtifactStatus {
            state: models::CacheArtifactState::Error,
            error_message: Some(error.clone()),
            ..Default::default()
        },
        playback_source: None,
        selected_audio_role: None,
        restore_at_seconds: 0.0,
        resume_playing: false,
        error_message: Some(error),
    }
}

fn now_ms() -> i64 {
    system_time_ms(SystemTime::now())
}

fn system_time_ms(value: SystemTime) -> i64 {
    value
        .duration_since(UNIX_EPOCH)
        .map(|duration| i64::try_from(duration.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

#[tauri::command]
pub async fn list_clips(
    manager: State<'_, ClipLibraryManager>,
    request: ClipListRequest,
) -> Result<ClipListResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.list(request))
        .await
        .map_err(|error| format!("The Clips query worker failed: {error}"))
}

#[tauri::command]
pub async fn list_collections_command(
    manager: State<'_, ClipLibraryManager>,
) -> Result<CollectionsResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.collections())
        .await
        .map_err(|error| format!("The collections query worker failed: {error}"))
}

#[tauri::command]
pub async fn create_collection_command(
    manager: State<'_, ClipLibraryManager>,
    request: CreateCollectionRequest,
) -> Result<CollectionMutationResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.create_collection(request))
        .await
        .map_err(|error| format!("The collection creation worker failed: {error}"))
}

#[tauri::command]
pub async fn rename_collection_command(
    manager: State<'_, ClipLibraryManager>,
    request: RenameCollectionRequest,
) -> Result<CollectionMutationResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.rename_collection(request))
        .await
        .map_err(|error| format!("The collection rename worker failed: {error}"))
}

#[tauri::command]
pub async fn delete_collection_command(
    manager: State<'_, ClipLibraryManager>,
    request: CollectionIdRequest,
) -> Result<ClipActionResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.delete_collection(&request.collection_id))
        .await
        .map_err(|error| format!("The collection deletion worker failed: {error}"))
}

#[tauri::command]
pub async fn set_clip_collection_membership(
    manager: State<'_, ClipLibraryManager>,
    request: SetClipCollectionRequest,
) -> Result<ClipMutationResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.set_collection_membership(request))
        .await
        .map_err(|error| format!("The collection membership worker failed: {error}"))
}

#[tauri::command]
pub async fn record_clip_watch_command(
    manager: State<'_, ClipLibraryManager>,
    request: ClipIdRequest,
) -> Result<ClipMutationResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.record_watch(&request.clip_id))
        .await
        .map_err(|error| format!("The watch update worker failed: {error}"))
}

#[tauri::command]
pub async fn refresh_clip_library(
    manager: State<'_, ClipLibraryManager>,
) -> Result<ReconcileResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.reconcile_response())
        .await
        .map_err(|error| format!("The Clips reconciliation worker failed: {error}"))
}

#[tauri::command]
pub async fn set_clip_favorite(
    manager: State<'_, ClipLibraryManager>,
    request: SetFavoriteRequest,
) -> Result<ClipMutationResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.mutate_favorite(request))
        .await
        .map_err(|error| format!("The favorite update worker failed: {error}"))
}

#[tauri::command]
pub async fn set_clip_pinned(
    manager: State<'_, ClipLibraryManager>,
    request: SetPinnedRequest,
) -> Result<ClipMutationResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.mutate_pinned(request))
        .await
        .map_err(|error| format!("The protection update worker failed: {error}"))
}

#[tauri::command]
pub async fn preview_storage_cleanup(
    manager: State<'_, ClipLibraryManager>,
    request: StorageCleanupPreviewRequest,
) -> Result<StorageCleanupPreviewResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.preview_storage_cleanup(request))
        .await
        .map_err(|error| format!("The storage preview worker failed: {error}"))
}

#[tauri::command]
pub async fn execute_storage_cleanup(
    manager: State<'_, ClipLibraryManager>,
    request: StorageCleanupExecuteRequest,
) -> Result<StorageCleanupExecutionResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.execute_storage_cleanup(request))
        .await
        .map_err(|error| format!("The storage cleanup worker failed: {error}"))
}

#[tauri::command]
pub async fn rename_clip_display_name(
    manager: State<'_, ClipLibraryManager>,
    request: RenameClipRequest,
) -> Result<ClipMutationResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.rename(request))
        .await
        .map_err(|error| format!("The clip rename worker failed: {error}"))
}

#[tauri::command]
pub async fn open_clip_file(
    manager: State<'_, ClipLibraryManager>,
    request: ClipIdRequest,
) -> Result<ClipActionResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.open_clip(&request.clip_id))
        .await
        .map_err(|error| format!("The clip open worker failed: {error}"))
}

#[tauri::command]
pub async fn open_clip_folder(
    manager: State<'_, ClipLibraryManager>,
    request: ClipIdRequest,
) -> Result<ClipActionResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.open_folder(&request.clip_id))
        .await
        .map_err(|error| format!("The folder open worker failed: {error}"))
}

#[tauri::command]
pub async fn delete_clip(
    manager: State<'_, ClipLibraryManager>,
    request: ClipIdRequest,
) -> Result<ClipActionResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.delete(&request.clip_id))
        .await
        .map_err(|error| format!("The clip deletion worker failed: {error}"))
}

#[tauri::command]
pub async fn get_clip_playback_info(
    manager: State<'_, ClipLibraryManager>,
    request: ClipIdRequest,
) -> Result<ClipPlaybackInfoResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.playback_info(&request.clip_id))
        .await
        .map_err(|error| format!("The clip playback-info worker failed: {error}"))
}

#[tauri::command]
pub async fn request_clip_thumbnail(
    manager: State<'_, ClipLibraryManager>,
    app: AppHandle,
    request: PrepareClipMediaRequest,
) -> Result<PrepareClipMediaResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.request_thumbnail(request, app))
        .await
        .map_err(|error| format!("The thumbnail request worker failed: {error}"))
}

#[tauri::command]
pub async fn prepare_clip_preview(
    manager: State<'_, ClipLibraryManager>,
    app: AppHandle,
    request: PrepareClipMediaRequest,
) -> Result<PrepareClipMediaResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.prepare_preview(request, app))
        .await
        .map_err(|error| format!("The preview request worker failed: {error}"))
}

#[tauri::command]
pub async fn prepare_clip_audio_preview(
    manager: State<'_, ClipLibraryManager>,
    app: AppHandle,
    request: PrepareClipAudioRequest,
) -> Result<PrepareClipMediaResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.prepare_audio(request, app))
        .await
        .map_err(|error| format!("The audio-preview request worker failed: {error}"))
}

#[tauri::command]
pub async fn prepare_editor_audio_preview(
    manager: State<'_, ClipLibraryManager>,
    app: AppHandle,
    request: PrepareClipAudioRequest,
) -> Result<PrepareClipMediaResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.prepare_editor_audio(request, app))
        .await
        .map_err(|error| format!("The Editor audio-preview request worker failed: {error}"))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::models::CURRENT_SCHEMA_VERSION;
    use super::*;

    fn saved_metadata(path: PathBuf, created_at_ms: i64) -> SavedClipMetadata {
        SavedClipMetadata {
            file_path: path,
            created_at_ms,
            duration_100ns: 10_000_000,
            requested_duration_seconds: 30,
            width: 1920,
            height: 1080,
            fps_numerator: 60,
            fps_denominator: 1,
            video_codec: "h264".into(),
            video_profile: None,
            video_bitrate_bps: None,
            total_bitrate_bps: None,
            capture_target_label: None,
            capture_target_type: None,
            audio_tracks: Vec::new(),
        }
    }

    #[test]
    fn database_initialization_failure_is_a_library_error_not_a_panic() {
        let root = std::env::temp_dir().join(format!("stage12-manager-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let blocking_file = root.join("not-a-directory");
        fs::write(&blocking_file, b"file").unwrap();
        let manager =
            ClipLibraryManager::initialize(blocking_file.join("clips.db"), root.join("Clips"));
        assert!(manager.database_path().is_none());
        assert!(!manager.list(ClipListRequest::default()).success);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn indexing_failure_never_deletes_the_saved_mp4() {
        let root =
            std::env::temp_dir().join(format!("stage12-index-failure-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let clips = root.join("Clips");
        fs::create_dir_all(&clips).unwrap();
        let clip = clips.join("saved.mp4");
        fs::write(&clip, b"valid user artifact").unwrap();
        let blocking_file = root.join("not-a-directory");
        fs::write(&blocking_file, b"file").unwrap();
        let manager = ClipLibraryManager::initialize(blocking_file.join("clips.db"), clips);
        let result = manager.index_saved_clip(SavedClipMetadata {
            file_path: clip.clone(),
            created_at_ms: 0,
            duration_100ns: 10_000_000,
            requested_duration_seconds: 30,
            width: 1920,
            height: 1080,
            fps_numerator: 60,
            fps_denominator: 1,
            video_codec: "hevc".into(),
            video_profile: None,
            video_bitrate_bps: None,
            total_bitrate_bps: None,
            capture_target_label: None,
            capture_target_type: None,
            audio_tracks: Vec::new(),
        });
        assert!(result.is_err());
        assert!(clip.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn delete_removes_the_owned_mp4_and_its_database_row() {
        let root = std::env::temp_dir().join(format!("stage12-delete-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let clips = root.join("Clips");
        fs::create_dir_all(&clips).unwrap();
        let clip = clips.join("saved.mp4");
        fs::write(&clip, b"permanent clip").unwrap();
        let manager = ClipLibraryManager::initialize(root.join("clips.db"), clips);
        let indexed = manager
            .index_saved_clip(SavedClipMetadata {
                file_path: clip.clone(),
                created_at_ms: 0,
                duration_100ns: 10_000_000,
                requested_duration_seconds: 30,
                width: 1920,
                height: 1080,
                fps_numerator: 60,
                fps_denominator: 1,
                video_codec: "hevc".into(),
                video_profile: None,
                video_bitrate_bps: None,
                total_bitrate_bps: None,
                capture_target_label: None,
                capture_target_type: None,
                audio_tracks: Vec::new(),
            })
            .unwrap();
        let cached_preview = manager
            .media_cache
            .preview_artifact_for_test(&indexed.clip_id);
        fs::write(&cached_preview, b"cached preview").unwrap();

        assert!(manager.delete(&indexed.clip_id).success);
        assert!(!clip.exists());
        assert!(!cached_preview.exists());
        assert_eq!(
            count_clips(&manager.database().unwrap().open().unwrap()).unwrap(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn storage_cleanup_deletes_oldest_unprotected_and_preserves_protected_file() {
        let root = std::env::temp_dir().join(format!("stage24-cleanup-{}", Uuid::new_v4()));
        let clips = root.join("Clips");
        fs::create_dir_all(&clips).unwrap();
        let oldest = clips.join("oldest.mp4");
        let protected = clips.join("protected.mp4");
        let newest = clips.join("newest.mp4");
        fs::write(&oldest, b"oldest").unwrap();
        fs::write(&protected, b"protected").unwrap();
        fs::write(&newest, b"newest").unwrap();
        let manager = ClipLibraryManager::initialize(root.join("clips.db"), clips);
        let oldest_id = manager
            .index_saved_clip(saved_metadata(oldest.clone(), 1))
            .unwrap()
            .clip_id;
        let protected_id = manager
            .index_saved_clip(saved_metadata(protected.clone(), 2))
            .unwrap()
            .clip_id;
        let newest_id = manager
            .index_saved_clip(saved_metadata(newest.clone(), 3))
            .unwrap()
            .clip_id;
        let connection = manager.database().unwrap().open().unwrap();
        connection
            .execute("UPDATE clips SET file_size_bytes = 734003200", [])
            .unwrap();
        set_pinned(&connection, &protected_id, true).unwrap();
        drop(connection);

        let preview = manager.preview_storage_cleanup(StorageCleanupPreviewRequest {
            quota_bytes: storage::MIN_QUOTA_BYTES,
        });
        assert!(preview.success);
        assert_eq!(
            preview
                .candidates
                .iter()
                .map(|item| item.clip_id.as_str())
                .collect::<Vec<_>>(),
            vec![oldest_id.as_str(), newest_id.as_str()]
        );
        let execution = manager.execute_storage_cleanup(StorageCleanupExecuteRequest {
            plan_id: preview.plan_id.unwrap(),
        });
        assert!(execution.success);
        assert_eq!(execution.deleted_count, 2);
        assert!(!oldest.exists());
        assert!(protected.exists());
        assert!(!newest.exists());
        assert!(manager.clip_by_id(&protected_id).unwrap().unwrap().pinned);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn storage_preview_rejects_database_path_outside_owned_clips_without_deleting_it() {
        let root = std::env::temp_dir().join(format!("stage24-path-safety-{}", Uuid::new_v4()));
        let clips = root.join("Clips");
        fs::create_dir_all(&clips).unwrap();
        let owned = clips.join("owned.mp4");
        let outside = root.join("outside.mp4");
        fs::write(&owned, b"owned").unwrap();
        fs::write(&outside, b"outside").unwrap();
        let manager = ClipLibraryManager::initialize(root.join("clips.db"), clips);
        let id = manager
            .index_saved_clip(saved_metadata(owned.clone(), 1))
            .unwrap()
            .clip_id;
        let connection = manager.database().unwrap().open().unwrap();
        connection
            .execute(
                "UPDATE clips SET file_path = ?1, file_size_bytes = ?2 WHERE id = ?3",
                rusqlite::params![
                    outside.to_string_lossy(),
                    i64::try_from(storage::MIN_QUOTA_BYTES + 1).unwrap(),
                    id
                ],
            )
            .unwrap();
        drop(connection);

        let preview = manager.preview_storage_cleanup(StorageCleanupPreviewRequest {
            quota_bytes: storage::MIN_QUOTA_BYTES,
        });
        assert!(!preview.success);
        assert!(preview.error_message.unwrap().contains("outside"));
        assert!(outside.exists());
        assert!(owned.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn trusted_id_resolves_playback_and_missing_source_is_a_controlled_error() {
        let root = std::env::temp_dir().join(format!("stage13-playback-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let clips = root.join("Clips");
        fs::create_dir_all(&clips).unwrap();
        let clip = clips.join("trusted.mp4");
        let outside = root.join("outside.mp4");
        fs::write(&clip, b"master").unwrap();
        fs::write(&outside, b"outside").unwrap();
        let manager = ClipLibraryManager::initialize(root.join("clips.db"), clips);
        let indexed = manager
            .index_saved_clip(SavedClipMetadata {
                file_path: clip.clone(),
                created_at_ms: 0,
                duration_100ns: 10_000_000,
                requested_duration_seconds: 30,
                width: 1920,
                height: 1080,
                fps_numerator: 60,
                fps_denominator: 1,
                video_codec: "h264".into(),
                video_profile: None,
                video_bitrate_bps: None,
                total_bitrate_bps: None,
                capture_target_label: None,
                capture_target_type: None,
                audio_tracks: Vec::new(),
            })
            .unwrap();
        let response = manager.playback_info(&indexed.clip_id);
        assert!(response.success);
        assert_eq!(
            response.info.unwrap().master_path,
            clip.canonicalize().unwrap().to_string_lossy()
        );
        assert!(
            !manager
                .playback_info(outside.to_string_lossy().as_ref())
                .success
        );
        assert!(outside.exists());

        fs::remove_file(&clip).unwrap();
        let missing = manager.playback_info(&indexed.clip_id);
        assert!(!missing.success);
        assert!(missing.error_message.unwrap().contains("Could not resolve"));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cache_cleanup_failure_does_not_block_master_deletion() {
        let root =
            std::env::temp_dir().join(format!("stage13-delete-cache-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let clips = root.join("Clips");
        fs::create_dir_all(&clips).unwrap();
        let clip = clips.join("delete.mp4");
        fs::write(&clip, b"master").unwrap();
        let manager = ClipLibraryManager::initialize(root.join("clips.db"), clips);
        let indexed = manager
            .index_saved_clip(SavedClipMetadata {
                file_path: clip.clone(),
                created_at_ms: 0,
                duration_100ns: 10_000_000,
                requested_duration_seconds: 30,
                width: 1920,
                height: 1080,
                fps_numerator: 60,
                fps_denominator: 1,
                video_codec: "h264".into(),
                video_profile: None,
                video_bitrate_bps: None,
                total_bitrate_bps: None,
                capture_target_label: None,
                capture_target_type: None,
                audio_tracks: Vec::new(),
            })
            .unwrap();
        fs::write(root.join("Previews"), b"blocks cache directory").unwrap();
        assert!(manager.delete(&indexed.clip_id).success);
        assert!(!clip.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn verified_export_indexes_with_friendly_name_and_flattened_metadata() {
        let root = std::env::temp_dir().join(format!("stage17-export-index-{}", Uuid::new_v4()));
        let clips = root.join("Clips");
        fs::create_dir_all(&clips).unwrap();
        let output = clips.join("physical-edited.mp4");
        fs::write(&output, b"verified permanent export").unwrap();
        let manager = ClipLibraryManager::initialize(root.join("clips.db"), clips);
        let indexed = manager
            .index_exported_clip(
                SavedClipMetadata {
                    file_path: output.clone(),
                    created_at_ms: 1,
                    duration_100ns: 123_456_780,
                    requested_duration_seconds: 13,
                    width: 2560,
                    height: 1440,
                    fps_numerator: 60,
                    fps_denominator: 1,
                    video_codec: "h264".into(),
                    video_profile: Some("High".into()),
                    video_bitrate_bps: Some(12_000_000),
                    total_bitrate_bps: Some(12_192_000),
                    capture_target_label: None,
                    capture_target_type: None,
                    audio_tracks: vec![ClipAudioTrack {
                        stream_index: 1,
                        role: "Combined".into(),
                        title: Some("Combined".into()),
                        handler_name: Some("Combined".into()),
                        codec: "aac".into(),
                        profile: Some("LC".into()),
                        sample_rate: Some(48_000),
                        channels: Some(2),
                        bitrate_bps: Some(192_000),
                        is_default: true,
                    }],
                },
                "Source Raid - Edited".into(),
            )
            .unwrap();
        let clip = manager.clip_by_id(&indexed.clip_id).unwrap().unwrap();
        assert_eq!(clip.display_name, "Source Raid - Edited");
        assert_eq!(
            clip.file_size_bytes,
            b"verified permanent export".len() as u64
        );
        assert_eq!(clip.duration_100ns, 123_456_780);
        assert_eq!(clip.video_codec, "h264");
        assert_eq!(clip.audio_tracks.len(), 1);
        assert_eq!(clip.audio_tracks[0].role, "Combined");
        assert!(clip.audio_tracks[0].is_default);
        assert!(output.exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_version_constant_is_current() {
        assert_eq!(CURRENT_SCHEMA_VERSION, 3);
    }
}
