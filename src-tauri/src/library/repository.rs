use std::collections::HashMap;

use rusqlite::types::Value;
use rusqlite::{params, params_from_iter, Connection, OptionalExtension, Row, TransactionBehavior};

use super::models::{
    ClipAudioTrack, ClipFingerprint, ClipListItem, ClipListRequest, ClipSortOrder, ClipUpsert,
    CollectionSummary, LibrarySummary, CLIP_METADATA_VERSION,
};

pub fn upsert_clip(connection: &mut Connection, clip: &ClipUpsert) -> Result<String, String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database_error)?;
    let existing: Option<(String, String, i64)> = transaction
        .query_row(
            "SELECT id, display_name, favorite FROM clips WHERE file_path = ?1",
            [&clip.file_path],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(database_error)?;
    let clip_id = existing
        .as_ref()
        .map(|value| value.0.clone())
        .unwrap_or_else(|| clip.id.clone());
    let display_name = existing
        .as_ref()
        .map(|value| value.1.as_str())
        .unwrap_or(&clip.display_name);
    let favorite = existing.as_ref().map(|value| value.2).unwrap_or(0);
    transaction
        .execute(
            "INSERT INTO clips(
                id, file_path, filename, display_name, created_at_ms, library_added_at_ms,
                file_modified_at_ms, file_size_bytes, duration_100ns, requested_duration_seconds,
                width, height, fps_numerator, fps_denominator, video_codec, video_profile,
                video_bitrate_bps, total_bitrate_bps, capture_target_label, capture_target_type,
                favorite, imported_existing_file, audio_stream_count, default_audio_stream_title,
                metadata_version
             ) VALUES(
                ?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20,
                ?21, ?22, ?23, ?24, ?25
             )
             ON CONFLICT(file_path) DO UPDATE SET
                filename=excluded.filename,
                created_at_ms=excluded.created_at_ms,
                file_modified_at_ms=excluded.file_modified_at_ms,
                file_size_bytes=excluded.file_size_bytes,
                duration_100ns=excluded.duration_100ns,
                requested_duration_seconds=COALESCE(excluded.requested_duration_seconds, clips.requested_duration_seconds),
                width=excluded.width,
                height=excluded.height,
                fps_numerator=excluded.fps_numerator,
                fps_denominator=excluded.fps_denominator,
                video_codec=excluded.video_codec,
                video_profile=excluded.video_profile,
                video_bitrate_bps=excluded.video_bitrate_bps,
                total_bitrate_bps=excluded.total_bitrate_bps,
                capture_target_label=COALESCE(excluded.capture_target_label, clips.capture_target_label),
                capture_target_type=COALESCE(excluded.capture_target_type, clips.capture_target_type),
                imported_existing_file=clips.imported_existing_file,
                audio_stream_count=excluded.audio_stream_count,
                default_audio_stream_title=excluded.default_audio_stream_title,
                metadata_version=excluded.metadata_version",
            params![
                clip_id,
                clip.file_path,
                clip.filename,
                display_name,
                clip.created_at_ms,
                clip.library_added_at_ms,
                clip.file_modified_at_ms,
                to_i64(clip.file_size_bytes)?,
                clip.duration_100ns,
                clip.requested_duration_seconds.map(i64::from),
                i64::from(clip.width),
                i64::from(clip.height),
                i64::from(clip.fps_numerator),
                i64::from(clip.fps_denominator),
                clip.video_codec,
                clip.video_profile,
                clip.video_bitrate_bps.map(to_i64).transpose()?,
                clip.total_bitrate_bps.map(to_i64).transpose()?,
                clip.capture_target_label,
                clip.capture_target_type,
                favorite,
                i64::from(clip.imported_existing_file),
                i64::try_from(clip.audio_tracks.len()).unwrap_or(i64::MAX),
                clip.audio_tracks
                    .iter()
                    .find(|track| track.is_default)
                    .and_then(|track| track.title.clone().or_else(|| track.handler_name.clone())),
                CLIP_METADATA_VERSION,
            ],
        )
        .map_err(database_error)?;
    transaction
        .execute(
            "DELETE FROM clip_audio_tracks WHERE clip_id = ?1",
            [&clip_id],
        )
        .map_err(database_error)?;
    for track in &clip.audio_tracks {
        transaction
            .execute(
                "INSERT INTO clip_audio_tracks(
                    clip_id, stream_index, role, title, handler_name, codec, profile,
                    sample_rate, channels, bitrate_bps, is_default
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    clip_id,
                    i64::from(track.stream_index),
                    track.role,
                    track.title,
                    track.handler_name,
                    track.codec,
                    track.profile,
                    track.sample_rate.map(i64::from),
                    track.channels.map(i64::from),
                    track.bitrate_bps.map(to_i64).transpose()?,
                    i64::from(track.is_default),
                ],
            )
            .map_err(database_error)?;
    }
    transaction.commit().map_err(database_error)?;
    Ok(clip_id)
}

pub fn list_clips(
    connection: &Connection,
    request: ClipListRequest,
) -> Result<(Vec<ClipListItem>, u64), String> {
    let request = request.normalized();
    let mut clauses = Vec::new();
    let mut values = Vec::<Value>::new();
    if let Some(search) = request.search_text {
        clauses.push(
            "(display_name LIKE ? ESCAPE '\\' COLLATE NOCASE OR filename LIKE ? ESCAPE '\\' COLLATE NOCASE OR capture_target_label LIKE ? ESCAPE '\\' COLLATE NOCASE)"
                .to_string(),
        );
        let pattern = format!("%{}%", escape_like(&search));
        values.extend([
            Value::Text(pattern.clone()),
            Value::Text(pattern.clone()),
            Value::Text(pattern),
        ]);
    }
    if request.favorites_only {
        clauses.push("favorite = 1".to_string());
    }
    if request.recently_watched_only {
        clauses.push("last_watched_at_ms IS NOT NULL".to_string());
    }
    if let Some(collection_id) = request.collection_id {
        clauses.push("EXISTS (SELECT 1 FROM clip_collections cc WHERE cc.clip_id = clips.id AND cc.collection_id = ?)".to_string());
        values.push(Value::Text(collection_id));
    }
    let where_sql = if clauses.is_empty() {
        String::new()
    } else {
        format!(" WHERE {}", clauses.join(" AND "))
    };
    let count_sql = format!("SELECT COUNT(*) FROM clips{where_sql}");
    let total: i64 = connection
        .query_row(&count_sql, params_from_iter(values.iter()), |row| {
            row.get(0)
        })
        .map_err(database_error)?;
    let order = match request.sort_order {
        ClipSortOrder::NewestFirst => "created_at_ms DESC, id DESC",
        ClipSortOrder::OldestFirst => "created_at_ms ASC, id ASC",
        ClipSortOrder::NameAscending => "display_name COLLATE NOCASE ASC, created_at_ms DESC",
        ClipSortOrder::NameDescending => "display_name COLLATE NOCASE DESC, created_at_ms DESC",
        ClipSortOrder::LongestFirst => "duration_100ns DESC, created_at_ms DESC",
        ClipSortOrder::ShortestFirst => "duration_100ns ASC, created_at_ms DESC",
        ClipSortOrder::LargestFirst => "file_size_bytes DESC, created_at_ms DESC",
        ClipSortOrder::SmallestFirst => "file_size_bytes ASC, created_at_ms DESC",
        ClipSortOrder::MostPlayed => "play_count DESC, last_watched_at_ms DESC, created_at_ms DESC",
        ClipSortOrder::RecentlyWatched => "last_watched_at_ms DESC, created_at_ms DESC",
    };
    let sql = format!(
        "SELECT id, file_path, filename, display_name, created_at_ms, library_added_at_ms,
                file_modified_at_ms, file_size_bytes, duration_100ns, requested_duration_seconds,
                width, height, fps_numerator, fps_denominator, video_codec, video_profile,
                video_bitrate_bps, total_bitrate_bps, capture_target_label, capture_target_type,
                favorite, imported_existing_file, audio_stream_count, default_audio_stream_title,
                metadata_version, play_count, last_watched_at_ms
         FROM clips{where_sql} ORDER BY {order} LIMIT ? OFFSET ?"
    );
    let mut query_values = values;
    query_values.push(Value::Integer(i64::from(request.limit)));
    query_values.push(Value::Integer(i64::from(request.offset)));
    let mut statement = connection.prepare(&sql).map_err(database_error)?;
    let rows = statement
        .query_map(params_from_iter(query_values.iter()), map_clip_row)
        .map_err(database_error)?;
    let mut clips = rows
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    load_audio_tracks(connection, &mut clips)?;
    load_collection_ids(connection, &mut clips)?;
    Ok((clips, u64::try_from(total).unwrap_or(0)))
}

pub fn get_clip(connection: &Connection, clip_id: &str) -> Result<Option<ClipListItem>, String> {
    let mut statement = connection
        .prepare(
            "SELECT id, file_path, filename, display_name, created_at_ms, library_added_at_ms,
                    file_modified_at_ms, file_size_bytes, duration_100ns, requested_duration_seconds,
                    width, height, fps_numerator, fps_denominator, video_codec, video_profile,
                    video_bitrate_bps, total_bitrate_bps, capture_target_label, capture_target_type,
                    favorite, imported_existing_file, audio_stream_count, default_audio_stream_title,
                    metadata_version, play_count, last_watched_at_ms
             FROM clips WHERE id = ?1",
        )
        .map_err(database_error)?;
    let clip = statement
        .query_row([clip_id], map_clip_row)
        .optional()
        .map_err(database_error)?;
    let Some(mut clip) = clip else {
        return Ok(None);
    };
    load_audio_tracks(connection, std::slice::from_mut(&mut clip))?;
    load_collection_ids(connection, std::slice::from_mut(&mut clip))?;
    Ok(Some(clip))
}

pub fn set_favorite(connection: &Connection, clip_id: &str, favorite: bool) -> Result<(), String> {
    require_one(
        connection
            .execute(
                "UPDATE clips SET favorite = ?1 WHERE id = ?2",
                params![i64::from(favorite), clip_id],
            )
            .map_err(database_error)?,
        clip_id,
    )
}

pub fn rename_display_name(
    connection: &Connection,
    clip_id: &str,
    display_name: &str,
) -> Result<(), String> {
    require_one(
        connection
            .execute(
                "UPDATE clips SET display_name = ?1 WHERE id = ?2",
                params![display_name, clip_id],
            )
            .map_err(database_error)?,
        clip_id,
    )
}

pub fn delete_clip_row(connection: &mut Connection, clip_id: &str) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database_error)?;
    require_one(
        transaction
            .execute("DELETE FROM clips WHERE id = ?1", [clip_id])
            .map_err(database_error)?,
        clip_id,
    )?;
    transaction.commit().map_err(database_error)
}

pub fn delete_rows_for_missing_paths(
    connection: &mut Connection,
    clip_ids: &[String],
) -> Result<(), String> {
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(database_error)?;
    for id in clip_ids {
        transaction
            .execute("DELETE FROM clips WHERE id = ?1", [id])
            .map_err(database_error)?;
    }
    transaction.commit().map_err(database_error)
}

pub fn all_fingerprints(connection: &Connection) -> Result<Vec<ClipFingerprint>, String> {
    let mut statement = connection
        .prepare("SELECT id, file_path, file_size_bytes, file_modified_at_ms FROM clips")
        .map_err(database_error)?;
    let result = statement
        .query_map([], |row| {
            Ok(ClipFingerprint {
                id: row.get(0)?,
                file_path: row.get(1)?,
                file_size_bytes: from_i64(row.get(2)?),
                file_modified_at_ms: row.get(3)?,
            })
        })
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error);
    result
}

pub fn count_clips(connection: &Connection) -> Result<u64, String> {
    let value: i64 = connection
        .query_row("SELECT COUNT(*) FROM clips", [], |row| row.get(0))
        .map_err(database_error)?;
    Ok(u64::try_from(value).unwrap_or(0))
}

pub fn library_summary(connection: &Connection) -> Result<LibrarySummary, String> {
    connection
        .query_row(
            "SELECT COUNT(*), COALESCE(SUM(file_size_bytes), 0),
                    COALESCE(SUM(CASE WHEN favorite = 1 THEN 1 ELSE 0 END), 0),
                    (SELECT COUNT(*) FROM collections)
             FROM clips",
            [],
            |row| {
                Ok(LibrarySummary {
                    clip_count: from_i64(row.get(0)?),
                    total_size_bytes: from_i64(row.get(1)?),
                    favorites_count: from_i64(row.get(2)?),
                    collections_count: from_i64(row.get(3)?),
                })
            },
        )
        .map_err(database_error)
}

pub fn record_clip_watch(
    connection: &Connection,
    clip_id: &str,
    watched_at_ms: i64,
) -> Result<ClipListItem, String> {
    require_one(
        connection
            .execute(
                "UPDATE clips SET play_count = play_count + 1, last_watched_at_ms = ?1 WHERE id = ?2",
                params![watched_at_ms, clip_id],
            )
            .map_err(database_error)?,
        clip_id,
    )?;
    get_clip(connection, clip_id)?
        .ok_or_else(|| "The watched clip disappeared from the Library.".to_string())
}

pub fn list_collections(connection: &Connection) -> Result<Vec<CollectionSummary>, String> {
    let mut statement = connection
        .prepare(
            "SELECT c.id, c.name, c.created_at_ms, c.updated_at_ms, COUNT(cc.clip_id)
             FROM collections c LEFT JOIN clip_collections cc ON cc.collection_id = c.id
             GROUP BY c.id ORDER BY c.name COLLATE NOCASE ASC",
        )
        .map_err(database_error)?;
    let collections = statement
        .query_map([], map_collection_row)
        .map_err(database_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(database_error)?;
    Ok(collections)
}

pub fn get_collection(
    connection: &Connection,
    collection_id: &str,
) -> Result<Option<CollectionSummary>, String> {
    connection
        .query_row(
            "SELECT c.id, c.name, c.created_at_ms, c.updated_at_ms, COUNT(cc.clip_id)
             FROM collections c LEFT JOIN clip_collections cc ON cc.collection_id = c.id
             WHERE c.id = ?1 GROUP BY c.id",
            [collection_id],
            map_collection_row,
        )
        .optional()
        .map_err(database_error)
}

pub fn create_collection(
    connection: &Connection,
    id: &str,
    name: &str,
    now_ms: i64,
) -> Result<CollectionSummary, String> {
    connection
        .execute(
            "INSERT INTO collections(id, name, created_at_ms, updated_at_ms) VALUES(?1, ?2, ?3, ?3)",
            params![id, name, now_ms],
        )
        .map_err(collection_database_error)?;
    get_collection(connection, id)?
        .ok_or_else(|| "The created collection could not be loaded.".to_string())
}

pub fn rename_collection(
    connection: &Connection,
    collection_id: &str,
    name: &str,
    now_ms: i64,
) -> Result<CollectionSummary, String> {
    let changed = connection
        .execute(
            "UPDATE collections SET name = ?1, updated_at_ms = ?2 WHERE id = ?3",
            params![name, now_ms, collection_id],
        )
        .map_err(collection_database_error)?;
    if changed != 1 {
        return Err(format!("No collection exists with ID '{collection_id}'."));
    }
    get_collection(connection, collection_id)?
        .ok_or_else(|| "The renamed collection could not be loaded.".to_string())
}

pub fn delete_collection(connection: &Connection, collection_id: &str) -> Result<(), String> {
    let changed = connection
        .execute("DELETE FROM collections WHERE id = ?1", [collection_id])
        .map_err(database_error)?;
    if changed == 1 {
        Ok(())
    } else {
        Err(format!("No collection exists with ID '{collection_id}'."))
    }
}

pub fn set_clip_collection(
    connection: &Connection,
    clip_id: &str,
    collection_id: &str,
    included: bool,
    now_ms: i64,
) -> Result<ClipListItem, String> {
    if get_clip(connection, clip_id)?.is_none() {
        return Err(format!("No library clip exists with ID '{clip_id}'."));
    }
    if get_collection(connection, collection_id)?.is_none() {
        return Err(format!("No collection exists with ID '{collection_id}'."));
    }
    if included {
        connection
            .execute(
                "INSERT OR IGNORE INTO clip_collections(clip_id, collection_id, added_at_ms) VALUES(?1, ?2, ?3)",
                params![clip_id, collection_id, now_ms],
            )
            .map_err(database_error)?;
    } else {
        connection
            .execute(
                "DELETE FROM clip_collections WHERE clip_id = ?1 AND collection_id = ?2",
                params![clip_id, collection_id],
            )
            .map_err(database_error)?;
    }
    get_clip(connection, clip_id)?
        .ok_or_else(|| "The updated clip disappeared from the Library.".to_string())
}

fn load_audio_tracks(connection: &Connection, clips: &mut [ClipListItem]) -> Result<(), String> {
    if clips.is_empty() {
        return Ok(());
    }
    let ids = clips
        .iter()
        .enumerate()
        .map(|(index, clip)| (clip.id.clone(), index))
        .collect::<HashMap<_, _>>();
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT clip_id, stream_index, role, title, handler_name, codec, profile,
                sample_rate, channels, bitrate_bps, is_default
         FROM clip_audio_tracks WHERE clip_id IN ({placeholders})
         ORDER BY clip_id, stream_index"
    );
    let mut statement = connection.prepare(&sql).map_err(database_error)?;
    let parameters = ids
        .keys()
        .map(|id| Value::Text(id.clone()))
        .collect::<Vec<_>>();
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            Ok((
                row.get::<_, String>(0)?,
                ClipAudioTrack {
                    stream_index: u32::try_from(row.get::<_, i64>(1)?).unwrap_or(0),
                    role: row.get(2)?,
                    title: row.get(3)?,
                    handler_name: row.get(4)?,
                    codec: row.get(5)?,
                    profile: row.get(6)?,
                    sample_rate: row
                        .get::<_, Option<i64>>(7)?
                        .and_then(|value| u32::try_from(value).ok()),
                    channels: row
                        .get::<_, Option<i64>>(8)?
                        .and_then(|value| u16::try_from(value).ok()),
                    bitrate_bps: row.get::<_, Option<i64>>(9)?.map(from_i64),
                    is_default: row.get::<_, i64>(10)? != 0,
                },
            ))
        })
        .map_err(database_error)?;
    for row in rows {
        let (id, track) = row.map_err(database_error)?;
        if let Some(index) = ids.get(&id) {
            clips[*index].audio_tracks.push(track);
        }
    }
    Ok(())
}

fn load_collection_ids(connection: &Connection, clips: &mut [ClipListItem]) -> Result<(), String> {
    if clips.is_empty() {
        return Ok(());
    }
    let ids = clips
        .iter()
        .enumerate()
        .map(|(index, clip)| (clip.id.clone(), index))
        .collect::<HashMap<_, _>>();
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT clip_id, collection_id FROM clip_collections WHERE clip_id IN ({placeholders}) ORDER BY collection_id"
    );
    let parameters = ids
        .keys()
        .map(|id| Value::Text(id.clone()))
        .collect::<Vec<_>>();
    let mut statement = connection.prepare(&sql).map_err(database_error)?;
    let rows = statement
        .query_map(params_from_iter(parameters.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })
        .map_err(database_error)?;
    for row in rows {
        let (clip_id, collection_id) = row.map_err(database_error)?;
        if let Some(index) = ids.get(&clip_id) {
            clips[*index].collection_ids.push(collection_id);
        }
    }
    Ok(())
}

fn map_clip_row(row: &Row<'_>) -> rusqlite::Result<ClipListItem> {
    Ok(ClipListItem {
        id: row.get(0)?,
        file_path: row.get(1)?,
        filename: row.get(2)?,
        display_name: row.get(3)?,
        created_at_ms: row.get(4)?,
        library_added_at_ms: row.get(5)?,
        file_modified_at_ms: row.get(6)?,
        file_size_bytes: from_i64(row.get(7)?),
        duration_100ns: row.get(8)?,
        requested_duration_seconds: row
            .get::<_, Option<i64>>(9)?
            .and_then(|value| u32::try_from(value).ok()),
        width: u32::try_from(row.get::<_, i64>(10)?).unwrap_or(0),
        height: u32::try_from(row.get::<_, i64>(11)?).unwrap_or(0),
        fps_numerator: u32::try_from(row.get::<_, i64>(12)?).unwrap_or(0),
        fps_denominator: u32::try_from(row.get::<_, i64>(13)?).unwrap_or(1),
        video_codec: row.get(14)?,
        video_profile: row.get(15)?,
        video_bitrate_bps: row.get::<_, Option<i64>>(16)?.map(from_i64),
        total_bitrate_bps: row.get::<_, Option<i64>>(17)?.map(from_i64),
        capture_target_label: row.get(18)?,
        capture_target_type: row.get(19)?,
        favorite: row.get::<_, i64>(20)? != 0,
        imported_existing_file: row.get::<_, i64>(21)? != 0,
        audio_stream_count: u32::try_from(row.get::<_, i64>(22)?).unwrap_or(0),
        default_audio_stream_title: row.get(23)?,
        metadata_version: row.get(24)?,
        play_count: from_i64(row.get(25)?),
        last_watched_at_ms: row.get(26)?,
        collection_ids: Vec::new(),
        audio_tracks: Vec::new(),
    })
}

fn map_collection_row(row: &Row<'_>) -> rusqlite::Result<CollectionSummary> {
    Ok(CollectionSummary {
        id: row.get(0)?,
        name: row.get(1)?,
        created_at_ms: row.get(2)?,
        updated_at_ms: row.get(3)?,
        clip_count: from_i64(row.get(4)?),
    })
}

fn escape_like(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_")
}

fn require_one(changed: usize, clip_id: &str) -> Result<(), String> {
    if changed == 1 {
        Ok(())
    } else {
        Err(format!("No library clip exists with ID '{clip_id}'."))
    }
}

fn to_i64(value: u64) -> Result<i64, String> {
    i64::try_from(value).map_err(|_| "Clip metadata exceeds SQLite integer range.".to_string())
}

fn from_i64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn database_error(error: rusqlite::Error) -> String {
    format!("Clips database operation failed: {error}")
}

fn collection_database_error(error: rusqlite::Error) -> String {
    if matches!(error, rusqlite::Error::SqliteFailure(ref value, _) if value.extended_code == 2067)
    {
        "A collection with that name already exists.".to_string()
    } else {
        database_error(error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::database::LibraryDatabase;

    fn clip(id: &str, path: &str, created: i64) -> ClipUpsert {
        ClipUpsert {
            id: id.into(),
            file_path: path.into(),
            filename: format!("{id}.mp4"),
            display_name: id.into(),
            created_at_ms: created,
            library_added_at_ms: created,
            file_modified_at_ms: created,
            file_size_bytes: 100,
            duration_100ns: 10_000_000,
            requested_duration_seconds: Some(30),
            width: 1920,
            height: 1080,
            fps_numerator: 60,
            fps_denominator: 1,
            video_codec: "hevc".into(),
            video_profile: Some("Main".into()),
            video_bitrate_bps: Some(10_000_000),
            total_bitrate_bps: Some(10_500_000),
            capture_target_label: Some("Game's 100% Window".into()),
            capture_target_type: Some("window".into()),
            imported_existing_file: false,
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

    #[test]
    fn insert_update_children_favorite_rename_search_order_and_pagination() {
        let (mut connection, _) = LibraryDatabase::initialize_in_memory().unwrap();
        upsert_clip(&mut connection, &clip("older", "C:/Clips/older.mp4", 1)).unwrap();
        upsert_clip(&mut connection, &clip("newer", "C:/Clips/newer.mp4", 2)).unwrap();
        set_favorite(&connection, "newer", true).unwrap();
        rename_display_name(&connection, "newer", "Jake's 100% _clip").unwrap();

        let (newest, total) = list_clips(
            &connection,
            ClipListRequest {
                limit: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(total, 2);
        assert_eq!(newest[0].id, "newer");
        assert!(newest[0].favorite);
        assert_eq!(newest[0].audio_tracks.len(), 1);

        let (search, _) = list_clips(
            &connection,
            ClipListRequest {
                search_text: Some("Jake's 100% _clip".into()),
                limit: 100,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(search.len(), 1);
        let (page, _) = list_clips(
            &connection,
            ClipListRequest {
                limit: 1,
                offset: 1,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(page[0].id, "older");
    }

    #[test]
    fn duplicate_path_updates_without_duplicate_and_cascade_deletes_audio() {
        let (mut connection, _) = LibraryDatabase::initialize_in_memory().unwrap();
        let first = clip("first", "C:/Clips/same.mp4", 1);
        upsert_clip(&mut connection, &first).unwrap();
        let mut changed = clip("second", "C:/Clips/same.mp4", 2);
        changed.audio_tracks.push(ClipAudioTrack {
            stream_index: 2,
            role: "Game".into(),
            title: Some("Game".into()),
            handler_name: None,
            codec: "aac".into(),
            profile: Some("LC".into()),
            sample_rate: Some(48_000),
            channels: Some(2),
            bitrate_bps: Some(192_000),
            is_default: false,
        });
        assert_eq!(upsert_clip(&mut connection, &changed).unwrap(), "first");
        assert_eq!(count_clips(&connection).unwrap(), 1);
        delete_clip_row(&mut connection, "first").unwrap();
        let audio_count: i64 = connection
            .query_row("SELECT COUNT(*) FROM clip_audio_tracks", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(audio_count, 0);
    }

    #[test]
    fn favorite_and_display_name_persist_after_database_reopen() {
        let root = std::env::temp_dir().join(format!("stage12-persistence-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let database_path = root.join("clips.db");
        let (database, _) = LibraryDatabase::initialize(database_path.clone()).unwrap();
        {
            let mut connection = database.open().unwrap();
            upsert_clip(
                &mut connection,
                &clip("persistent", "C:/Clips/persistent.mp4", 1),
            )
            .unwrap();
            set_favorite(&connection, "persistent", true).unwrap();
            rename_display_name(&connection, "persistent", "Persistent Name").unwrap();
        }
        drop(database);

        let (reopened, _) = LibraryDatabase::initialize(database_path).unwrap();
        let saved = get_clip(&reopened.open().unwrap(), "persistent")
            .unwrap()
            .unwrap();
        assert!(saved.favorite);
        assert_eq!(saved.display_name, "Persistent Name");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn collections_watch_sort_filters_cascades_and_summary_compose() {
        let (mut connection, _) = LibraryDatabase::initialize_in_memory().unwrap();
        let mut first = clip("first", "C:/Clips/first.mp4", 1);
        first.display_name = "GTA Snow 雪".into();
        first.file_size_bytes = 125;
        first.duration_100ns = 50_000_000;
        let mut second = clip("second", "C:/Clips/second.mp4", 2);
        second.file_size_bytes = 250;
        second.duration_100ns = 100_000_000;
        let mut third = clip("third", "C:/Clips/third.mp4", 3);
        third.file_size_bytes = 500;
        upsert_clip(&mut connection, &first).unwrap();
        upsert_clip(&mut connection, &second).unwrap();
        upsert_clip(&mut connection, &third).unwrap();
        set_favorite(&connection, "first", true).unwrap();

        create_collection(&connection, "funny", "Funny 雪", 10).unwrap();
        create_collection(&connection, "best", "Best Clips", 11).unwrap();
        assert!(create_collection(&connection, "duplicate", "FUNNY 雪", 12).is_err());
        set_clip_collection(&connection, "first", "funny", true, 20).unwrap();
        set_clip_collection(&connection, "first", "best", true, 21).unwrap();
        set_clip_collection(&connection, "second", "funny", true, 22).unwrap();
        set_clip_collection(&connection, "first", "funny", true, 23).unwrap();
        let membership_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM clip_collections WHERE clip_id='first'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(membership_count, 2);

        let (composed, _) = list_clips(
            &connection,
            ClipListRequest {
                search_text: Some("gta".into()),
                favorites_only: true,
                collection_id: Some("funny".into()),
                sort_order: ClipSortOrder::NameDescending,
                limit: 100,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            composed
                .iter()
                .map(|item| item.id.as_str())
                .collect::<Vec<_>>(),
            ["first"]
        );
        assert_eq!(composed[0].collection_ids.len(), 2);

        let watched_once = record_clip_watch(&connection, "second", 100).unwrap();
        assert_eq!(watched_once.play_count, 1);
        assert_eq!(watched_once.last_watched_at_ms, Some(100));
        record_clip_watch(&connection, "second", 200).unwrap();
        record_clip_watch(&connection, "first", 300).unwrap();
        let (most_played, _) = list_clips(
            &connection,
            ClipListRequest {
                sort_order: ClipSortOrder::MostPlayed,
                limit: 100,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(most_played[0].id, "second");
        let (recent, total) = list_clips(
            &connection,
            ClipListRequest {
                recently_watched_only: true,
                sort_order: ClipSortOrder::RecentlyWatched,
                limit: 100,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(total, 2);
        assert_eq!(recent[0].id, "first");

        let totals = library_summary(&connection).unwrap();
        assert_eq!(totals.clip_count, 3);
        assert_eq!(totals.total_size_bytes, 875);
        assert_eq!(totals.favorites_count, 1);
        assert_eq!(totals.collections_count, 2);

        rename_collection(&connection, "funny", "Funny Stuff", 400).unwrap();
        assert_eq!(
            get_clip(&connection, "first")
                .unwrap()
                .unwrap()
                .collection_ids
                .len(),
            2
        );
        set_clip_collection(&connection, "first", "best", false, 401).unwrap();
        assert_eq!(
            get_clip(&connection, "first")
                .unwrap()
                .unwrap()
                .collection_ids,
            ["funny"]
        );
        delete_collection(&connection, "funny").unwrap();
        assert!(get_clip(&connection, "first").unwrap().is_some());
        assert!(get_clip(&connection, "first")
            .unwrap()
            .unwrap()
            .collection_ids
            .is_empty());

        set_clip_collection(&connection, "second", "best", true, 500).unwrap();
        delete_clip_row(&mut connection, "second").unwrap();
        let orphan_count: i64 = connection
            .query_row(
                "SELECT COUNT(*) FROM clip_collections WHERE clip_id='second'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(orphan_count, 0);
        let totals = library_summary(&connection).unwrap();
        assert_eq!(totals.clip_count, 2);
        assert_eq!(totals.total_size_bytes, 625);
    }
}
