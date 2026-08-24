use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde_json::Value;
use uuid::Uuid;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, FILE_ATTRIBUTE_REPARSE_POINT, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

// Only these direct children contain durable data whose lifecycle SlickClip owns. Tauri/WebView
// runtime state and the disposable ReplayBuffer share the application-data root but are not
// migration inputs.
const PERSISTENT_APP_DATA_DIRECTORIES: [&str; 3] = ["Library", "Preferences", "WatchParty"];

pub fn migrate_legacy_installation(
    legacy_app_data: &Path,
    slickclip_app_data: &Path,
    legacy_video_root: &Path,
    slickclip_video_root: &Path,
) -> Result<(), String> {
    migrate_owned_app_data(legacy_app_data, slickclip_app_data)?;
    migrate_tree_without_overwrite(legacy_video_root, slickclip_video_root)?;

    let legacy_clips = legacy_video_root.join("Clips");
    let slickclip_clips = slickclip_video_root.join("Clips");
    let library_root = slickclip_app_data.join("Library");
    rewrite_library_paths(
        &library_root.join("clips.db"),
        &legacy_clips,
        &slickclip_clips,
    )?;
    rewrite_cache_metadata(&library_root, &legacy_clips, &slickclip_clips)?;
    Ok(())
}

fn migrate_owned_app_data(source_root: &Path, destination_root: &Path) -> Result<(), String> {
    validate_migration_root(source_root)?;
    validate_migration_root(destination_root)?;

    for directory in PERSISTENT_APP_DATA_DIRECTORIES {
        preflight_tree_without_overwrite(
            &source_root.join(directory),
            &destination_root.join(directory),
        )?;
    }
    for directory in PERSISTENT_APP_DATA_DIRECTORIES {
        migrate_tree_after_preflight(
            &source_root.join(directory),
            &destination_root.join(directory),
        )?;
    }

    remove_directory_if_empty(source_root)
}

fn validate_migration_root(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    reject_reparse_point(path)?;
    if !path.is_dir() {
        return Err(format!(
            "SlickClip migration expected '{}' to be a directory.",
            path.display()
        ));
    }
    Ok(())
}

fn migrate_tree_without_overwrite(source: &Path, destination: &Path) -> Result<(), String> {
    preflight_tree_without_overwrite(source, destination)?;
    migrate_tree_after_preflight(source, destination)
}

fn preflight_tree_without_overwrite(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    validate_tree(source)?;
    if !source.is_dir() {
        return Err(format!(
            "SlickClip migration expected '{}' to be a directory.",
            source.display()
        ));
    }
    if destination.exists() {
        reject_reparse_point(destination)?;
        if !destination.is_dir() {
            return Err(format!(
                "SlickClip migration destination '{}' is not a directory.",
                destination.display()
            ));
        }
        preflight_merge(source, destination)?;
    }
    Ok(())
}

fn migrate_tree_after_preflight(source: &Path, destination: &Path) -> Result<(), String> {
    if !source.exists() {
        return Ok(());
    }
    if destination.exists() {
        merge_tree(source, destination)?;
    } else {
        let parent = destination.parent().ok_or_else(|| {
            format!(
                "Migration destination '{}' has no parent.",
                destination.display()
            )
        })?;
        fs::create_dir_all(parent).map_err(migration_io_error)?;
        fs::rename(source, destination).map_err(migration_io_error)?;
    }
    Ok(())
}

fn remove_directory_if_empty(path: &Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    reject_reparse_point(path)?;
    if fs::read_dir(path)
        .map_err(migration_io_error)?
        .next()
        .is_none()
    {
        fs::remove_dir(path).map_err(migration_io_error)?;
    }
    Ok(())
}

fn validate_tree(path: &Path) -> Result<(), String> {
    reject_reparse_point(path)?;
    if !path.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(path).map_err(migration_io_error)? {
        validate_tree(&entry.map_err(migration_io_error)?.path())?;
    }
    Ok(())
}

fn preflight_merge(source: &Path, destination: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(migration_io_error)? {
        let entry = entry.map_err(migration_io_error)?;
        let source_path = entry.path();
        reject_reparse_point(&source_path)?;
        let destination_path = destination.join(entry.file_name());
        let file_type = entry.file_type().map_err(migration_io_error)?;
        if !destination_path.exists() {
            continue;
        }
        reject_reparse_point(&destination_path)?;
        if file_type.is_dir() && destination_path.is_dir() {
            preflight_merge(&source_path, &destination_path)?;
        } else {
            return Err(format!(
                "SlickClip migration found both legacy and current data at '{}'. Nothing was overwritten; move or back up the conflicting item and launch again.",
                destination_path.display()
            ));
        }
    }
    Ok(())
}

fn merge_tree(source: &Path, destination: &Path) -> Result<(), String> {
    fs::create_dir_all(destination).map_err(migration_io_error)?;
    for entry in fs::read_dir(source).map_err(migration_io_error)? {
        let entry = entry.map_err(migration_io_error)?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type().map_err(migration_io_error)?.is_dir() && destination_path.is_dir() {
            merge_tree(&source_path, &destination_path)?;
        } else {
            fs::rename(&source_path, &destination_path).map_err(migration_io_error)?;
        }
    }
    fs::remove_dir(source).map_err(migration_io_error)
}

fn reject_reparse_point(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(migration_io_error)?;
    if metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT.0 != 0
    {
        Err(format!(
            "SlickClip migration refused to follow the symbolic link or reparse point '{}'.",
            path.display()
        ))
    } else {
        Ok(())
    }
}

fn rewrite_library_paths(
    database_path: &Path,
    legacy_clips: &Path,
    slickclip_clips: &Path,
) -> Result<(), String> {
    if !database_path.is_file() {
        return Ok(());
    }
    let mut connection = Connection::open(database_path).map_err(database_error)?;
    let has_clips: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='clips')",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if !has_clips {
        return Ok(());
    }
    let mut statement = connection
        .prepare("SELECT id, file_path FROM clips")
        .map_err(database_error)?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    drop(statement);

    let affected = rows
        .iter()
        .filter(|(_, stored_path)| {
            Path::new(stored_path)
                .parent()
                .is_some_and(|parent| paths_equal_ignoring_case(parent, legacy_clips))
        })
        .cloned()
        .collect::<Vec<_>>();
    if affected.is_empty() {
        return Ok(());
    }
    let mut updates = Vec::new();
    let mut destinations = Vec::<(PathBuf, String)>::new();
    for (id, stored_path) in affected {
        let stored = Path::new(&stored_path);
        let filename = stored
            .file_name()
            .ok_or_else(|| format!("Legacy clip row '{id}' has no filename."))?;
        let migrated = slickclip_clips.join(filename);
        if stored.exists() && migrated.exists() {
            return Err(format!(
                "SlickClip migration found both legacy and current clip data for Library row '{id}'. Nothing was overwritten."
            ));
        }
        if !migrated.exists() {
            // A missing file remains under the existing reconciliation behavior. In particular,
            // never invent a destination merely because a row resembles a legacy path.
            continue;
        }
        let canonical_root = slickclip_clips.canonicalize().map_err(migration_io_error)?;
        let canonical = migrated.canonicalize().map_err(|error| {
            format!(
                "Migrated clip '{}' is missing or inaccessible: {error}",
                migrated.display()
            )
        })?;
        if canonical.parent() != Some(canonical_root.as_path())
            || !canonical.is_file()
            || !canonical
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("mp4"))
        {
            return Err(format!(
                "Migrated Library row '{id}' did not resolve to a direct-child MP4 in '{}'.",
                slickclip_clips.display()
            ));
        }
        if let Some((_, other_id)) = destinations
            .iter()
            .find(|(destination, _)| paths_equal_ignoring_case(destination, &canonical))
        {
            return Err(format!(
                "Legacy Library rows '{other_id}' and '{id}' both resolve to '{}'. Nothing was changed.",
                canonical.display()
            ));
        }
        destinations.push((canonical.clone(), id.clone()));
        let current_rows = rows
            .iter()
            .filter(|(row_id, row_path)| {
                row_id != &id && paths_equal_ignoring_case(Path::new(row_path), &canonical)
            })
            .map(|(row_id, _)| row_id.clone())
            .collect::<Vec<_>>();
        if current_rows.len() > 1 {
            return Err(format!(
                "Migrated clip '{}' has multiple current Library records. Nothing was changed.",
                canonical.display()
            ));
        }
        updates.push(LibraryPathUpdate {
            legacy_id: id,
            duplicate_current_id: current_rows.into_iter().next(),
            current_path: canonical.to_string_lossy().into_owned(),
        });
    }
    if updates.is_empty() {
        return Ok(());
    }
    let transaction = connection.transaction().map_err(database_error)?;
    for update in updates {
        if let Some(ref current_id) = update.duplicate_current_id {
            merge_reconciled_duplicate(&transaction, &update, current_id)?;
        } else {
            transaction
                .execute(
                    "UPDATE clips SET file_path = ?1 WHERE id = ?2",
                    params![update.current_path, update.legacy_id],
                )
                .map_err(database_error)?;
        }
    }
    transaction.commit().map_err(database_error)
}

struct LibraryPathUpdate {
    legacy_id: String,
    duplicate_current_id: Option<String>,
    current_path: String,
}

struct MutableClipMetadata {
    display_name: String,
    favorite: bool,
    pinned: bool,
    play_count: i64,
    last_watched_at_ms: Option<i64>,
}

fn merge_reconciled_duplicate(
    transaction: &Transaction<'_>,
    update: &LibraryPathUpdate,
    current_id: &str,
) -> Result<(), String> {
    let legacy = mutable_clip_metadata(transaction, &update.legacy_id)?;
    let current = mutable_clip_metadata(transaction, current_id)?;
    let default_name = Path::new(&update.current_path)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let display_name = merged_display_name(
        default_name,
        &legacy.display_name,
        &current.display_name,
        &update.legacy_id,
        current_id,
    )?;

    transaction
        .execute(
            "INSERT OR IGNORE INTO clip_collections(clip_id, collection_id, added_at_ms)
             SELECT ?1, collection_id, added_at_ms FROM clip_collections WHERE clip_id = ?2",
            params![update.legacy_id, current_id],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM clip_audio_tracks WHERE clip_id = ?1",
            [current_id],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM clip_collections WHERE clip_id = ?1",
            [current_id],
        )
        .map_err(database_error)?;
    transaction
        .execute("DELETE FROM clips WHERE id = ?1", [current_id])
        .map_err(database_error)?;
    transaction
        .execute(
            "UPDATE clips SET
                file_path = ?1,
                display_name = ?2,
                favorite = ?3,
                pinned = ?4,
                play_count = ?5,
                last_watched_at_ms = ?6
             WHERE id = ?7",
            params![
                update.current_path,
                display_name,
                i64::from(legacy.favorite || current.favorite),
                i64::from(legacy.pinned || current.pinned),
                legacy.play_count.saturating_add(current.play_count),
                latest_timestamp(legacy.last_watched_at_ms, current.last_watched_at_ms),
                update.legacy_id,
            ],
        )
        .map_err(database_error)?;
    Ok(())
}

fn mutable_clip_metadata(
    transaction: &Transaction<'_>,
    clip_id: &str,
) -> Result<MutableClipMetadata, String> {
    transaction
        .query_row(
            "SELECT display_name, favorite, pinned, play_count, last_watched_at_ms
             FROM clips WHERE id = ?1",
            [clip_id],
            |row| {
                Ok(MutableClipMetadata {
                    display_name: row.get(0)?,
                    favorite: row.get::<_, i64>(1)? != 0,
                    pinned: row.get::<_, i64>(2)? != 0,
                    play_count: row.get(3)?,
                    last_watched_at_ms: row.get(4)?,
                })
            },
        )
        .optional()
        .map_err(database_error)?
        .ok_or_else(|| format!("SlickClip Library migration could not find clip '{clip_id}'."))
}

fn merged_display_name(
    default_name: &str,
    legacy_name: &str,
    current_name: &str,
    legacy_id: &str,
    current_id: &str,
) -> Result<String, String> {
    if legacy_name == current_name || current_name == default_name {
        Ok(legacy_name.to_string())
    } else if legacy_name == default_name {
        Ok(current_name.to_string())
    } else {
        Err(format!(
            "Legacy clip '{legacy_id}' and reconciled clip '{current_id}' have different custom names. Nothing was changed."
        ))
    }
}

fn latest_timestamp(left: Option<i64>, right: Option<i64>) -> Option<i64> {
    match (left, right) {
        (Some(left), Some(right)) => Some(left.max(right)),
        (left, right) => left.or(right),
    }
}

fn rewrite_cache_metadata(
    library_root: &Path,
    legacy_clips: &Path,
    slickclip_clips: &Path,
) -> Result<(), String> {
    if !library_root.is_dir() {
        return Ok(());
    }
    let mut pending = vec![library_root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).map_err(migration_io_error)? {
            let entry = entry.map_err(migration_io_error)?;
            let path = entry.path();
            reject_reparse_point(&path)?;
            if entry.file_type().map_err(migration_io_error)?.is_dir() {
                pending.push(path);
                continue;
            }
            if !path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("json"))
            {
                continue;
            }
            let Ok(bytes) = fs::read(&path) else {
                continue;
            };
            let Ok(mut json) = serde_json::from_slice::<Value>(&bytes) else {
                continue;
            };
            if rewrite_json_paths(&mut json, legacy_clips, slickclip_clips) {
                let output = serde_json::to_vec_pretty(&json).map_err(|error| {
                    format!("Could not serialize migrated cache metadata: {error}")
                })?;
                atomic_replace(&path, &output)?;
            }
        }
    }
    Ok(())
}

fn rewrite_json_paths(value: &mut Value, legacy_root: &Path, slickclip_root: &Path) -> bool {
    match value {
        Value::String(text) => replace_path_prefix(text, legacy_root, slickclip_root),
        Value::Array(values) => values.iter_mut().fold(false, |changed, value| {
            rewrite_json_paths(value, legacy_root, slickclip_root) || changed
        }),
        Value::Object(values) => values.values_mut().fold(false, |changed, value| {
            rewrite_json_paths(value, legacy_root, slickclip_root) || changed
        }),
        _ => false,
    }
}

fn replace_path_prefix(value: &mut String, legacy_root: &Path, slickclip_root: &Path) -> bool {
    let normalized_value = normalized_windows_path_text(value);
    let legacy = normalized_windows_path(legacy_root);
    if normalized_value.len() < legacy.len()
        || !normalized_value[..legacy.len()].eq_ignore_ascii_case(&legacy)
        || normalized_value
            .as_bytes()
            .get(legacy.len())
            .is_some_and(|separator| *separator != b'\\' && *separator != b'/')
    {
        return false;
    }
    let current = normalized_windows_path(slickclip_root);
    let verbatim = value
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\"));
    *value = format!(
        "{}{}{}",
        if verbatim { r"\\?\" } else { "" },
        current,
        &normalized_value[legacy.len()..]
    );
    true
}

fn paths_equal_ignoring_case(left: &Path, right: &Path) -> bool {
    normalized_windows_path(left).eq_ignore_ascii_case(&normalized_windows_path(right))
}

fn normalized_windows_path(path: &Path) -> String {
    normalized_windows_path_text(&path.to_string_lossy())
}

fn normalized_windows_path_text(value: &str) -> String {
    let normalized = value.replace('/', "\\");
    if normalized
        .get(..8)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\UNC\"))
    {
        format!(r"\\{}", &normalized[8..])
    } else if normalized
        .get(..4)
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(r"\\?\"))
    {
        normalized[4..].to_string()
    } else {
        normalized
    }
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("Migration path '{}' has no parent.", path.display()))?;
    let temporary = parent.join(format!(".slickclip-migration-{}.tmp", Uuid::new_v4()));
    fs::write(&temporary, bytes).map_err(migration_io_error)?;
    let source = wide_null(temporary.as_os_str());
    let destination = wide_null(path.as_os_str());
    let result = unsafe {
        MoveFileExW(
            PCWSTR(source.as_ptr()),
            PCWSTR(destination.as_ptr()),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    }
    .map_err(|error| format!("Could not atomically migrate '{}': {error}", path.display()));
    if result.is_err() {
        let _ = fs::remove_file(temporary);
    }
    result
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn migration_io_error(error: std::io::Error) -> String {
    format!("SlickClip data migration failed: {error}")
}

fn database_error(error: rusqlite::Error) -> String {
    format!("SlickClip Library migration failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{ClipAudioTrack, ClipLibraryManager, SavedClipMetadata};

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("slickclip-stage25-{name}-{}", Uuid::new_v4()))
    }

    fn saved_metadata(path: PathBuf, created_at_ms: i64) -> SavedClipMetadata {
        SavedClipMetadata {
            file_path: path,
            created_at_ms,
            duration_100ns: 30_000_000,
            requested_duration_seconds: 3,
            width: 1920,
            height: 1080,
            fps_numerator: 60,
            fps_denominator: 1,
            video_codec: "h264".into(),
            video_profile: Some("High".into()),
            video_bitrate_bps: Some(8_000_000),
            total_bitrate_bps: Some(8_192_000),
            capture_target_label: Some("Migration fixture".into()),
            capture_target_type: Some("window".into()),
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
        }
    }

    fn clone_clip_row(
        connection: &Connection,
        source_id: &str,
        new_id: &str,
        new_path: &Path,
        display_name: &str,
    ) {
        let filename = new_path.file_name().unwrap().to_string_lossy();
        connection
            .execute(
                "INSERT INTO clips
                 SELECT ?1, ?2, ?3, ?4, created_at_ms, library_added_at_ms,
                        file_modified_at_ms, file_size_bytes, duration_100ns,
                        requested_duration_seconds, width, height, fps_numerator,
                        fps_denominator, video_codec, video_profile, video_bitrate_bps,
                        total_bitrate_bps, capture_target_label, capture_target_type,
                        favorite, imported_existing_file, audio_stream_count,
                        default_audio_stream_title, metadata_version, play_count,
                        last_watched_at_ms, pinned
                 FROM clips WHERE id = ?5",
                params![
                    new_id,
                    new_path.to_string_lossy(),
                    filename,
                    display_name,
                    source_id
                ],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO clip_audio_tracks
                 SELECT ?1, stream_index, role, title, handler_name, codec, profile,
                        sample_rate, channels, bitrate_bps, is_default
                 FROM clip_audio_tracks WHERE clip_id = ?2",
                params![new_id, source_id],
            )
            .unwrap();
    }

    #[test]
    fn migrates_paths_database_and_cache_metadata_idempotently() {
        let base = root("complete");
        let legacy_app = base.join("com.replayapp.desktop");
        let current_app = base.join("com.slickclip.desktop");
        let legacy_video = base.join("Videos").join("JustIn Replay");
        let current_video = base.join("Videos").join("SlickClip");
        let legacy_clip = legacy_video.join("Clips").join("legacy.mp4");
        fs::create_dir_all(legacy_clip.parent().unwrap()).unwrap();
        fs::write(&legacy_clip, b"clip").unwrap();
        let preferences = legacy_app.join("Preferences").join("ui-preferences.json");
        fs::create_dir_all(preferences.parent().unwrap()).unwrap();
        fs::write(&preferences, br#"{"playerVolume":0.37}"#).unwrap();
        let library = legacy_app.join("Library");
        fs::create_dir_all(library.join("Previews").join("clip-1")).unwrap();
        let database = library.join("clips.db");
        let connection = Connection::open(&database).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE clips(
                    id TEXT PRIMARY KEY,
                    file_path TEXT NOT NULL,
                    favorite INTEGER NOT NULL,
                    pinned INTEGER NOT NULL,
                    play_count INTEGER NOT NULL
                );
                CREATE TABLE collections(id TEXT PRIMARY KEY, name TEXT NOT NULL);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO clips VALUES('clip-1', ?1, 1, 1, 7)",
                [legacy_clip.to_string_lossy()],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO collections VALUES('collection-1', 'Highlights')",
                [],
            )
            .unwrap();
        drop(connection);
        let cache = library.join("Previews").join("clip-1").join("preview.json");
        fs::write(
            &cache,
            serde_json::to_vec(&serde_json::json!({
                "fingerprint": { "sourcePath": legacy_clip },
                "sourcePathCopy": legacy_clip,
            }))
            .unwrap(),
        )
        .unwrap();

        migrate_legacy_installation(&legacy_app, &current_app, &legacy_video, &current_video)
            .unwrap();
        migrate_legacy_installation(&legacy_app, &current_app, &legacy_video, &current_video)
            .unwrap();

        let migrated_clip = current_video
            .join("Clips")
            .join("legacy.mp4")
            .canonicalize()
            .unwrap();
        assert_eq!(fs::read(&migrated_clip).unwrap(), b"clip");
        let migrated_database =
            Connection::open(current_app.join("Library").join("clips.db")).unwrap();
        let stored: (String, i64, i64, i64) = migrated_database
            .query_row(
                "SELECT file_path, favorite, pinned, play_count FROM clips",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .unwrap();
        assert_eq!(PathBuf::from(stored.0), migrated_clip);
        assert_eq!((stored.1, stored.2, stored.3), (1, 1, 7));
        let collection: String = migrated_database
            .query_row("SELECT name FROM collections", [], |row| row.get(0))
            .unwrap();
        assert_eq!(collection, "Highlights");
        assert_eq!(
            fs::read(current_app.join("Preferences").join("ui-preferences.json")).unwrap(),
            br#"{"playerVolume":0.37}"#
        );
        let cache_text = fs::read_to_string(
            current_app
                .join("Library")
                .join("Previews")
                .join("clip-1")
                .join("preview.json"),
        )
        .unwrap();
        assert!(cache_text.contains("SlickClip"));
        assert!(!cache_text.contains("JustIn Replay"));
        assert!(!legacy_app.exists());
        assert!(!legacy_video.exists());
        drop(migrated_database);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn repairs_verbatim_legacy_path_and_preserves_identity_and_metadata() {
        let base = root("verbatim-path");
        let current_app = base.join("com.slickclip.desktop");
        let legacy_video = base.join("Videos").join("JustIn Replay");
        let current_video = base.join("Videos").join("SlickClip");
        let legacy_clip = legacy_video
            .join("Clips")
            .join("JustInReplay-20260816-040724.mp4");
        fs::create_dir_all(legacy_clip.parent().unwrap()).unwrap();
        fs::write(&legacy_clip, b"real observed fixture").unwrap();
        let database = current_app.join("Library").join("clips.db");
        let manager = ClipLibraryManager::initialize(database.clone(), legacy_video.join("Clips"));
        let original_id = manager
            .index_saved_clip(saved_metadata(legacy_clip.clone(), 100))
            .unwrap()
            .clip_id;
        drop(manager);
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE clips SET display_name = 'Migration Favorite', favorite = 1,
                                  pinned = 1, play_count = 7, last_watched_at_ms = 1234
                 WHERE id = ?1",
                [&original_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO collections(id, name, created_at_ms, updated_at_ms)
                 VALUES('collection-1', 'Highlights', 1, 1)",
                [],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO clip_collections(clip_id, collection_id, added_at_ms)
                 VALUES(?1, 'collection-1', 1)",
                [&original_id],
            )
            .unwrap();
        let stored_before: String = connection
            .query_row(
                "SELECT file_path FROM clips WHERE id = ?1",
                [&original_id],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stored_before.starts_with(r"\\?\"));
        drop(connection);

        fs::create_dir_all(current_video.parent().unwrap()).unwrap();
        fs::rename(&legacy_video, &current_video).unwrap();
        rewrite_library_paths(
            &database,
            &legacy_video.join("Clips"),
            &current_video.join("Clips"),
        )
        .unwrap();

        let connection = Connection::open(&database).unwrap();
        let stored: (String, String, i64, i64, i64, Option<i64>) = connection
            .query_row(
                "SELECT id, file_path, favorite, pinned, play_count, last_watched_at_ms
                 FROM clips",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored.0, original_id);
        assert_eq!(
            PathBuf::from(stored.1),
            current_video
                .join("Clips")
                .join("JustInReplay-20260816-040724.mp4")
                .canonicalize()
                .unwrap()
        );
        assert_eq!(
            (stored.2, stored.3, stored.4, stored.5),
            (1, 1, 7, Some(1234))
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM clip_collections", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        let audio: (String, i64) = connection
            .query_row(
                "SELECT role, is_default FROM clip_audio_tracks WHERE clip_id = ?1",
                [&stored.0],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(audio, ("Combined".into(), 1));
        drop(connection);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn consolidates_reconciled_duplicate_without_losing_user_metadata() {
        let base = root("reconciled-duplicate");
        let current_app = base.join("com.slickclip.desktop");
        let legacy_video = base.join("Videos").join("JustIn Replay");
        let current_video = base.join("Videos").join("SlickClip");
        let legacy_clip = legacy_video.join("Clips").join("duplicate.mp4");
        fs::create_dir_all(legacy_clip.parent().unwrap()).unwrap();
        fs::write(&legacy_clip, b"one physical clip").unwrap();
        let database = current_app.join("Library").join("clips.db");
        let manager = ClipLibraryManager::initialize(database.clone(), legacy_video.join("Clips"));
        let legacy_id = manager
            .index_saved_clip(saved_metadata(legacy_clip.clone(), 100))
            .unwrap()
            .clip_id;
        drop(manager);
        fs::create_dir_all(current_video.parent().unwrap()).unwrap();
        fs::rename(&legacy_video, &current_video).unwrap();
        let current_clip = current_video
            .join("Clips")
            .join("duplicate.mp4")
            .canonicalize()
            .unwrap();
        let current_id = "reconciled-current-id";
        let connection = Connection::open(&database).unwrap();
        connection
            .execute(
                "UPDATE clips SET display_name = 'Original custom name', favorite = 1,
                                  play_count = 3, last_watched_at_ms = 100
                 WHERE id = ?1",
                [&legacy_id],
            )
            .unwrap();
        clone_clip_row(
            &connection,
            &legacy_id,
            current_id,
            &current_clip,
            "duplicate",
        );
        connection
            .execute(
                "UPDATE clips SET favorite = 0, pinned = 1, play_count = 2,
                                  last_watched_at_ms = 200 WHERE id = ?1",
                [current_id],
            )
            .unwrap();
        connection
            .execute_batch(
                "INSERT INTO collections VALUES('legacy-collection', 'Legacy', 1, 1);
                 INSERT INTO collections VALUES('current-collection', 'Current', 1, 1);",
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO clip_collections VALUES(?1, 'legacy-collection', 1)",
                [&legacy_id],
            )
            .unwrap();
        connection
            .execute(
                "INSERT INTO clip_collections VALUES(?1, 'current-collection', 2)",
                [current_id],
            )
            .unwrap();
        drop(connection);

        rewrite_library_paths(
            &database,
            &legacy_video.join("Clips"),
            &current_video.join("Clips"),
        )
        .unwrap();

        let connection = Connection::open(&database).unwrap();
        let stored: (String, String, String, i64, i64, i64, Option<i64>) = connection
            .query_row(
                "SELECT id, file_path, display_name, favorite, pinned, play_count,
                        last_watched_at_ms FROM clips",
                [],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                        row.get(5)?,
                        row.get(6)?,
                    ))
                },
            )
            .unwrap();
        assert_eq!(stored.0, legacy_id);
        assert_eq!(PathBuf::from(stored.1), current_clip);
        assert_eq!(stored.2, "Original custom name");
        assert_eq!(
            (stored.3, stored.4, stored.5, stored.6),
            (1, 1, 5, Some(200))
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM clip_collections", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
        assert_eq!(
            connection
                .query_row("SELECT COUNT(*) FROM clip_audio_tracks", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            1
        );
        drop(connection);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn current_external_and_missing_rows_are_not_guessed_or_rewritten() {
        let base = root("unrelated-paths");
        let current_app = base.join("com.slickclip.desktop");
        let legacy_clips = base.join("Videos").join("JustIn Replay").join("Clips");
        let current_clips = base.join("Videos").join("SlickClip").join("Clips");
        let current_clip = current_clips.join("current.mp4");
        fs::create_dir_all(&current_clips).unwrap();
        fs::write(&current_clip, b"current").unwrap();
        let database = current_app.join("Library").join("clips.db");
        let manager = ClipLibraryManager::initialize(database.clone(), current_clips.clone());
        let current_id = manager
            .index_saved_clip(saved_metadata(current_clip.clone(), 100))
            .unwrap()
            .clip_id;
        drop(manager);
        let connection = Connection::open(&database).unwrap();
        let missing_path = legacy_clips.join("missing.mp4");
        let external_path = base.join("External").join("outside.mp4");
        clone_clip_row(
            &connection,
            &current_id,
            "missing-id",
            &missing_path,
            "missing",
        );
        clone_clip_row(
            &connection,
            &current_id,
            "external-id",
            &external_path,
            "outside",
        );
        let current_before: String = connection
            .query_row(
                "SELECT file_path FROM clips WHERE id = ?1",
                [&current_id],
                |row| row.get(0),
            )
            .unwrap();
        drop(connection);

        rewrite_library_paths(&database, &legacy_clips, &current_clips).unwrap();

        let connection = Connection::open(&database).unwrap();
        let path_for = |id: &str| {
            connection
                .query_row("SELECT file_path FROM clips WHERE id = ?1", [id], |row| {
                    row.get::<_, String>(0)
                })
                .unwrap()
        };
        assert_eq!(path_for(&current_id), current_before);
        assert_eq!(PathBuf::from(path_for("missing-id")), missing_path);
        assert_eq!(PathBuf::from(path_for("external-id")), external_path);
        drop(connection);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn legacy_and_current_clip_copies_fail_closed_without_database_changes() {
        let base = root("clip-collision");
        let current_app = base.join("com.slickclip.desktop");
        let legacy_video = base.join("Videos").join("JustIn Replay");
        let current_video = base.join("Videos").join("SlickClip");
        let legacy_clip = legacy_video.join("Clips").join("collision.mp4");
        let current_clip = current_video.join("Clips").join("collision.mp4");
        fs::create_dir_all(legacy_clip.parent().unwrap()).unwrap();
        fs::create_dir_all(current_clip.parent().unwrap()).unwrap();
        fs::write(&legacy_clip, b"legacy bytes").unwrap();
        fs::write(&current_clip, b"current bytes").unwrap();
        let database = current_app.join("Library").join("clips.db");
        let manager = ClipLibraryManager::initialize(database.clone(), legacy_video.join("Clips"));
        let id = manager
            .index_saved_clip(saved_metadata(legacy_clip.clone(), 100))
            .unwrap()
            .clip_id;
        drop(manager);
        let before: String = Connection::open(&database)
            .unwrap()
            .query_row("SELECT file_path FROM clips WHERE id = ?1", [&id], |row| {
                row.get(0)
            })
            .unwrap();

        let error = migrate_legacy_installation(
            &base.join("com.replayapp.desktop"),
            &current_app,
            &legacy_video,
            &current_video,
        )
        .unwrap_err();

        assert!(error.contains("both legacy and current data"));
        assert_eq!(fs::read(&legacy_clip).unwrap(), b"legacy bytes");
        assert_eq!(fs::read(&current_clip).unwrap(), b"current bytes");
        let after: String = Connection::open(&database)
            .unwrap()
            .query_row("SELECT file_path FROM clips WHERE id = ?1", [&id], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(after, before);
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn cache_path_rewrite_recognizes_verbatim_legacy_prefix() {
        let legacy = PathBuf::from(r"C:\Users\Fixture\Videos\JustIn Replay\Clips");
        let current = PathBuf::from(r"C:\Users\Fixture\Videos\SlickClip\Clips");
        let mut value = r"\\?\C:\Users\Fixture\Videos\JustIn Replay\Clips\clip.mp4".to_string();

        assert!(replace_path_prefix(&mut value, &legacy, &current));
        assert_eq!(
            value,
            r"\\?\C:\Users\Fixture\Videos\SlickClip\Clips\clip.mp4"
        );
    }

    #[test]
    fn reconciled_rename_is_preserved_and_conflicting_custom_names_fail_closed() {
        assert_eq!(
            merged_display_name("clip", "clip", "Renamed clip", "legacy", "current").unwrap(),
            "Renamed clip"
        );
        assert!(merged_display_name(
            "clip",
            "Legacy rename",
            "Current rename",
            "legacy",
            "current"
        )
        .unwrap_err()
        .contains("Nothing was changed"));
    }

    #[test]
    fn current_install_without_legacy_data_is_a_safe_noop() {
        let base = root("current-only");
        let legacy_app = base.join("com.replayapp.desktop");
        let current_app = base.join("com.slickclip.desktop");
        let legacy_video = base.join("Videos").join("JustIn Replay");
        let current_video = base.join("Videos").join("SlickClip");
        let database = current_app.join("Library").join("clips.db");
        fs::create_dir_all(database.parent().unwrap()).unwrap();
        Connection::open(&database)
            .unwrap()
            .execute_batch("CREATE TABLE clips(id TEXT PRIMARY KEY, file_path TEXT NOT NULL);")
            .unwrap();

        migrate_legacy_installation(&legacy_app, &current_app, &legacy_video, &current_video)
            .unwrap();

        assert!(database.exists());
        assert!(!current_video.join("Clips").exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn meaningful_app_data_on_both_sides_refuses_the_migration() {
        let base = root("collision");
        let legacy_app = base.join("com.replayapp.desktop");
        let current_app = base.join("com.slickclip.desktop");
        let legacy_video = base.join("Videos").join("JustIn Replay");
        let current_video = base.join("Videos").join("SlickClip");
        fs::create_dir_all(legacy_app.join("Library")).unwrap();
        fs::create_dir_all(current_app.join("Library")).unwrap();
        fs::write(legacy_app.join("Library").join("clips.db"), b"legacy").unwrap();
        fs::write(current_app.join("Library").join("clips.db"), b"current").unwrap();

        let error =
            migrate_legacy_installation(&legacy_app, &current_app, &legacy_video, &current_video)
                .unwrap_err();

        assert!(error.contains("both legacy and current data"));
        assert_eq!(
            fs::read(legacy_app.join("Library").join("clips.db")).unwrap(),
            b"legacy"
        );
        assert_eq!(
            fs::read(current_app.join("Library").join("clips.db")).unwrap(),
            b"current"
        );
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn runtime_overlap_does_not_block_legacy_owned_data() {
        let base = root("runtime-overlap");
        let legacy_app = base.join("com.replayapp.desktop");
        let current_app = base.join("com.slickclip.desktop");
        let legacy_video = base.join("Videos").join("JustIn Replay");
        let current_video = base.join("Videos").join("SlickClip");
        let legacy_runtime = legacy_app
            .join("EBWebView")
            .join("Crashpad")
            .join("metadata");
        let current_runtime = current_app
            .join("EBWebView")
            .join("Crashpad")
            .join("metadata");
        let legacy_preferences = legacy_app.join("Preferences").join("ui-preferences.json");
        fs::create_dir_all(legacy_runtime.parent().unwrap()).unwrap();
        fs::create_dir_all(current_runtime.parent().unwrap()).unwrap();
        fs::create_dir_all(legacy_preferences.parent().unwrap()).unwrap();
        fs::write(&legacy_runtime, b"legacy runtime").unwrap();
        fs::write(&current_runtime, b"current runtime").unwrap();
        fs::write(&legacy_preferences, br#"{"playerVolume":0.5}"#).unwrap();

        migrate_legacy_installation(&legacy_app, &current_app, &legacy_video, &current_video)
            .unwrap();

        assert_eq!(fs::read(&legacy_runtime).unwrap(), b"legacy runtime");
        assert_eq!(fs::read(&current_runtime).unwrap(), b"current runtime");
        assert_eq!(
            fs::read(current_app.join("Preferences").join("ui-preferences.json")).unwrap(),
            br#"{"playerVolume":0.5}"#
        );
        assert!(!legacy_preferences.exists());
        fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn runtime_only_roots_have_no_user_data_collision() {
        let base = root("runtime-only");
        let legacy_app = base.join("com.replayapp.desktop");
        let current_app = base.join("com.slickclip.desktop");
        let legacy_video = base.join("Videos").join("JustIn Replay");
        let current_video = base.join("Videos").join("SlickClip");
        let legacy_runtime = legacy_app.join("EBWebView").join("GPUCache").join("data_0");
        let current_runtime = current_app
            .join("EBWebView")
            .join("GPUCache")
            .join("data_0");
        fs::create_dir_all(legacy_runtime.parent().unwrap()).unwrap();
        fs::create_dir_all(current_runtime.parent().unwrap()).unwrap();
        fs::write(&legacy_runtime, b"legacy runtime").unwrap();
        fs::write(&current_runtime, b"current runtime").unwrap();

        migrate_legacy_installation(&legacy_app, &current_app, &legacy_video, &current_video)
            .unwrap();

        assert_eq!(fs::read(&legacy_runtime).unwrap(), b"legacy runtime");
        assert_eq!(fs::read(&current_runtime).unwrap(), b"current runtime");
        assert!(!current_app.join("Library").exists());
        assert!(!current_app.join("Preferences").exists());
        fs::remove_dir_all(base).unwrap();
    }
}
