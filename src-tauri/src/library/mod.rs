mod database;
mod migrations;
mod models;
mod reconcile;
mod repository;
mod safety;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

pub use models::{
    ClipActionResponse, ClipAudioTrack, ClipIdRequest, ClipListRequest, ClipListResponse,
    ClipMutationResponse, LibraryTelemetry, ReconcileResponse, RenameClipRequest,
    SetFavoriteRequest,
};

use database::LibraryDatabase;
use models::{ClipListItem, ClipUpsert, ReconciliationTelemetry};
use reconcile::{reconcile, FfprobeMediaInspector};
use repository::{
    count_clips, delete_clip_row, get_clip, list_clips as query_clips, rename_display_name,
    set_favorite, upsert_clip,
};
use safety::validate_owned_clip;

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
}

impl ClipLibraryManager {
    pub fn initialize(database_path: PathBuf, clips_root: PathBuf) -> Self {
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
        let display_name = canonical
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Replay")
            .to_string();
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
        let result = self
            .database()
            .and_then(|database| query_clips(&database.open()?, request));
        let elapsed = started.elapsed().as_secs_f64() * 1_000.0;
        self.lock_telemetry().last_list_query_duration_ms = Some(elapsed);
        match result {
            Ok((clips, total_count)) => ClipListResponse {
                success: true,
                clips,
                total_count,
                telemetry: self.telemetry_snapshot(),
                error_message: None,
            },
            Err(error) => ClipListResponse {
                success: false,
                clips: Vec::new(),
                total_count: 0,
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
        let database = self.database()?;
        let connection = database.open()?;
        let clip = get_clip(&connection, clip_id)?
            .ok_or_else(|| format!("No library clip exists with ID '{clip_id}'."))?;
        validate_owned_clip(&self.clips_root, Path::new(&clip.file_path))
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
        action_result((|| {
            let path = self.trusted_clip_path(clip_id)?;
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
            self.refresh_indexed_count();
            Ok(())
        })())
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

#[cfg(test)]
mod tests {
    use std::fs;

    use super::models::CURRENT_SCHEMA_VERSION;
    use super::*;

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

        assert!(manager.delete(&indexed.clip_id).success);
        assert!(!clip.exists());
        assert_eq!(
            count_clips(&manager.database().unwrap().open().unwrap()).unwrap(),
            0
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn schema_version_constant_is_current() {
        assert_eq!(CURRENT_SCHEMA_VERSION, 1);
    }
}
