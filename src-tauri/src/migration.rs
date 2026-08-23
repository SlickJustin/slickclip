use std::ffi::OsStr;
use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::fs::MetadataExt;
use std::path::Path;

#[cfg(test)]
use std::path::PathBuf;

use rusqlite::{params, Connection};
use serde_json::Value;
use uuid::Uuid;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, FILE_ATTRIBUTE_REPARSE_POINT, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

pub fn migrate_legacy_installation(
    legacy_app_data: &Path,
    slickclip_app_data: &Path,
    legacy_video_root: &Path,
    slickclip_video_root: &Path,
) -> Result<(), String> {
    migrate_tree_without_overwrite(legacy_app_data, slickclip_app_data)?;
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

fn migrate_tree_without_overwrite(source: &Path, destination: &Path) -> Result<(), String> {
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
        .into_iter()
        .filter(|(_, stored_path)| {
            Path::new(stored_path)
                .parent()
                .is_some_and(|parent| paths_equal_ignoring_case(parent, legacy_clips))
        })
        .collect::<Vec<_>>();
    if affected.is_empty() {
        return Ok(());
    }
    let canonical_root = slickclip_clips.canonicalize().map_err(migration_io_error)?;
    let mut updates = Vec::new();
    for (id, stored_path) in affected {
        let stored = Path::new(&stored_path);
        let filename = stored
            .file_name()
            .ok_or_else(|| format!("Legacy clip row '{id}' has no filename."))?;
        let migrated = slickclip_clips.join(filename);
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
        updates.push((id, canonical.to_string_lossy().into_owned()));
    }
    let transaction = connection.transaction().map_err(database_error)?;
    for (id, path) in updates {
        transaction
            .execute(
                "UPDATE clips SET file_path = ?1 WHERE id = ?2",
                params![path, id],
            )
            .map_err(database_error)?;
    }
    transaction.commit().map_err(database_error)
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
    let legacy = legacy_root.to_string_lossy();
    if value.len() < legacy.len()
        || !value[..legacy.len()].eq_ignore_ascii_case(&legacy)
        || value
            .as_bytes()
            .get(legacy.len())
            .is_some_and(|separator| *separator != b'\\' && *separator != b'/')
    {
        return false;
    }
    *value = format!(
        "{}{}",
        slickclip_root.to_string_lossy(),
        &value[legacy.len()..]
    );
    true
}

fn paths_equal_ignoring_case(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
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

    fn root(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("slickclip-stage25-{name}-{}", Uuid::new_v4()))
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
    fn merge_refuses_collisions_without_overwriting_either_file() {
        let base = root("collision");
        let legacy = base.join("legacy");
        let current = base.join("current");
        fs::create_dir_all(legacy.join("Library")).unwrap();
        fs::create_dir_all(current.join("Library")).unwrap();
        fs::write(legacy.join("Library").join("clips.db"), b"legacy").unwrap();
        fs::write(current.join("Library").join("clips.db"), b"current").unwrap();
        assert!(migrate_tree_without_overwrite(&legacy, &current).is_err());
        assert_eq!(
            fs::read(legacy.join("Library").join("clips.db")).unwrap(),
            b"legacy"
        );
        assert_eq!(
            fs::read(current.join("Library").join("clips.db")).unwrap(),
            b"current"
        );
        fs::remove_dir_all(base).unwrap();
    }
}
