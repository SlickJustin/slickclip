use rusqlite::{Connection, TransactionBehavior};

use super::models::CURRENT_SCHEMA_VERSION;

pub fn apply_migrations(connection: &mut Connection) -> Result<i64, String> {
    connection
        .execute_batch(
            "CREATE TABLE IF NOT EXISTS schema_migrations (
                version INTEGER PRIMARY KEY,
                applied_at_ms INTEGER NOT NULL
            );",
        )
        .map_err(database_error)?;
    let current: i64 = connection
        .query_row(
            "SELECT COALESCE(MAX(version), 0) FROM schema_migrations",
            [],
            |row| row.get(0),
        )
        .map_err(database_error)?;
    if current > CURRENT_SCHEMA_VERSION {
        return Err(format!(
            "The Clips database schema version {current} is newer than this app supports ({CURRENT_SCHEMA_VERSION})."
        ));
    }
    if current < 1 {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        transaction
            .execute_batch(
                "CREATE TABLE clips (
                    id TEXT PRIMARY KEY NOT NULL,
                    file_path TEXT NOT NULL UNIQUE,
                    filename TEXT NOT NULL,
                    display_name TEXT NOT NULL,
                    created_at_ms INTEGER NOT NULL,
                    library_added_at_ms INTEGER NOT NULL,
                    file_modified_at_ms INTEGER NOT NULL,
                    file_size_bytes INTEGER NOT NULL,
                    duration_100ns INTEGER NOT NULL,
                    requested_duration_seconds INTEGER,
                    width INTEGER NOT NULL,
                    height INTEGER NOT NULL,
                    fps_numerator INTEGER NOT NULL,
                    fps_denominator INTEGER NOT NULL,
                    video_codec TEXT NOT NULL,
                    video_profile TEXT,
                    video_bitrate_bps INTEGER,
                    total_bitrate_bps INTEGER,
                    capture_target_label TEXT,
                    capture_target_type TEXT,
                    favorite INTEGER NOT NULL DEFAULT 0 CHECK(favorite IN (0, 1)),
                    imported_existing_file INTEGER NOT NULL DEFAULT 0 CHECK(imported_existing_file IN (0, 1)),
                    audio_stream_count INTEGER NOT NULL DEFAULT 0,
                    default_audio_stream_title TEXT,
                    metadata_version INTEGER NOT NULL
                );
                CREATE INDEX clips_created_at_idx ON clips(created_at_ms DESC);
                CREATE INDEX clips_favorite_created_idx ON clips(favorite, created_at_ms DESC);
                CREATE TABLE clip_audio_tracks (
                    clip_id TEXT NOT NULL,
                    stream_index INTEGER NOT NULL,
                    role TEXT NOT NULL,
                    title TEXT,
                    handler_name TEXT,
                    codec TEXT NOT NULL,
                    profile TEXT,
                    sample_rate INTEGER,
                    channels INTEGER,
                    bitrate_bps INTEGER,
                    is_default INTEGER NOT NULL CHECK(is_default IN (0, 1)),
                    PRIMARY KEY(clip_id, stream_index),
                    FOREIGN KEY(clip_id) REFERENCES clips(id) ON DELETE CASCADE
                );",
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO schema_migrations(version, applied_at_ms)
                 VALUES(1, CAST(strftime('%s','now') AS INTEGER) * 1000)",
                [],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)?;
    }
    if current < 2 {
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(database_error)?;
        transaction
            .execute_batch(
                "ALTER TABLE clips ADD COLUMN play_count INTEGER NOT NULL DEFAULT 0 CHECK(play_count >= 0);
                 ALTER TABLE clips ADD COLUMN last_watched_at_ms INTEGER;
                 CREATE INDEX clips_play_count_idx ON clips(play_count DESC, created_at_ms DESC);
                 CREATE INDEX clips_last_watched_idx ON clips(last_watched_at_ms DESC);
                 CREATE TABLE collections (
                    id TEXT PRIMARY KEY NOT NULL,
                    name TEXT NOT NULL COLLATE NOCASE UNIQUE,
                    created_at_ms INTEGER NOT NULL,
                    updated_at_ms INTEGER NOT NULL
                 );
                 CREATE TABLE clip_collections (
                    clip_id TEXT NOT NULL,
                    collection_id TEXT NOT NULL,
                    added_at_ms INTEGER NOT NULL,
                    PRIMARY KEY(clip_id, collection_id),
                    FOREIGN KEY(clip_id) REFERENCES clips(id) ON DELETE CASCADE,
                    FOREIGN KEY(collection_id) REFERENCES collections(id) ON DELETE CASCADE
                 );
                 CREATE INDEX clip_collections_clip_idx ON clip_collections(clip_id);
                 CREATE INDEX clip_collections_collection_idx ON clip_collections(collection_id, added_at_ms DESC);",
            )
            .map_err(database_error)?;
        transaction
            .execute(
                "INSERT INTO schema_migrations(version, applied_at_ms)
                 VALUES(2, CAST(strftime('%s','now') AS INTEGER) * 1000)",
                [],
            )
            .map_err(database_error)?;
        transaction.commit().map_err(database_error)?;
    }
    Ok(CURRENT_SCHEMA_VERSION)
}

fn database_error(error: rusqlite::Error) -> String {
    format!("Clips database migration failed: {error}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fresh_migration_is_versioned_and_idempotent() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection
            .execute_batch("PRAGMA foreign_keys = ON;")
            .unwrap();
        assert_eq!(apply_migrations(&mut connection).unwrap(), 2);
        assert_eq!(apply_migrations(&mut connection).unwrap(), 2);
        let count: i64 = connection
            .query_row("SELECT COUNT(*) FROM schema_migrations", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn version_one_upgrade_preserves_clips_and_adds_stage18_metadata() {
        let mut connection = Connection::open_in_memory().unwrap();
        connection.execute_batch(
            "PRAGMA foreign_keys = ON;
             CREATE TABLE schema_migrations(version INTEGER PRIMARY KEY, applied_at_ms INTEGER NOT NULL);
             INSERT INTO schema_migrations VALUES(1, 1);
             CREATE TABLE clips (
                id TEXT PRIMARY KEY NOT NULL, file_path TEXT NOT NULL UNIQUE, filename TEXT NOT NULL,
                display_name TEXT NOT NULL, created_at_ms INTEGER NOT NULL, library_added_at_ms INTEGER NOT NULL,
                file_modified_at_ms INTEGER NOT NULL, file_size_bytes INTEGER NOT NULL, duration_100ns INTEGER NOT NULL,
                requested_duration_seconds INTEGER, width INTEGER NOT NULL, height INTEGER NOT NULL,
                fps_numerator INTEGER NOT NULL, fps_denominator INTEGER NOT NULL, video_codec TEXT NOT NULL,
                video_profile TEXT, video_bitrate_bps INTEGER, total_bitrate_bps INTEGER,
                capture_target_label TEXT, capture_target_type TEXT, favorite INTEGER NOT NULL DEFAULT 0,
                imported_existing_file INTEGER NOT NULL DEFAULT 0, audio_stream_count INTEGER NOT NULL DEFAULT 0,
                default_audio_stream_title TEXT, metadata_version INTEGER NOT NULL
             );
             CREATE TABLE clip_audio_tracks (
                clip_id TEXT NOT NULL, stream_index INTEGER NOT NULL, role TEXT NOT NULL, title TEXT,
                handler_name TEXT, codec TEXT NOT NULL, profile TEXT, sample_rate INTEGER, channels INTEGER,
                bitrate_bps INTEGER, is_default INTEGER NOT NULL, PRIMARY KEY(clip_id, stream_index),
                FOREIGN KEY(clip_id) REFERENCES clips(id) ON DELETE CASCADE
             );
             INSERT INTO clips VALUES('clip-1','C:/clip.mp4','clip.mp4','Existing',1,2,3,4,5,NULL,1920,1080,60,1,'hevc',NULL,NULL,NULL,NULL,NULL,1,0,0,NULL,1);"
        ).unwrap();

        assert_eq!(apply_migrations(&mut connection).unwrap(), 2);
        let row: (String, i64, Option<i64>) = connection
            .query_row(
                "SELECT display_name, play_count, last_watched_at_ms FROM clips WHERE id='clip-1'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(row, ("Existing".into(), 0, None));
    }
}
