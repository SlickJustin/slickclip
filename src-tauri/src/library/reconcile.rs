use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use uuid::Uuid;

use crate::clips::ffmpeg::{FfmpegExecutable, MediaProbeReport};

use super::database::LibraryDatabase;
use super::models::{ClipAudioTrack, ClipUpsert, ReconciliationTelemetry};
use super::repository::{all_fingerprints, delete_rows_for_missing_paths, upsert_clip};
use super::safety::{is_reconciliation_candidate, owned_missing_path, validate_owned_clip};

#[derive(Clone, Debug)]
pub struct InspectedMedia {
    pub duration_100ns: i64,
    pub width: u32,
    pub height: u32,
    pub fps_numerator: u32,
    pub fps_denominator: u32,
    pub video_codec: String,
    pub video_profile: Option<String>,
    pub video_bitrate_bps: Option<u64>,
    pub total_bitrate_bps: Option<u64>,
    pub audio_tracks: Vec<ClipAudioTrack>,
}

pub trait MediaInspector: Send + Sync {
    fn inspect(&self, path: &Path) -> Result<InspectedMedia, String>;
}

pub struct FfprobeMediaInspector;

impl MediaInspector for FfprobeMediaInspector {
    fn inspect(&self, path: &Path) -> Result<InspectedMedia, String> {
        let ffmpeg = FfmpegExecutable::resolve()?;
        inspected_from_probe(&ffmpeg.inspect_media(path)?)
    }
}

pub fn reconcile(
    database: &LibraryDatabase,
    clips_root: &Path,
    inspector: &dyn MediaInspector,
) -> Result<ReconciliationTelemetry, String> {
    let started = Instant::now();
    fs::create_dir_all(clips_root).map_err(|error| {
        format!(
            "Could not create permanent Clips directory '{}': {error}",
            clips_root.display()
        )
    })?;
    let mut connection = database.open()?;
    let fingerprints = all_fingerprints(&connection)?;
    let by_path = fingerprints
        .iter()
        .map(|value| (value.file_path.clone(), value.clone()))
        .collect::<HashMap<_, _>>();
    let mut seen = HashSet::new();
    let mut telemetry = ReconciliationTelemetry::default();

    let entries = fs::read_dir(clips_root).map_err(|error| {
        format!(
            "Could not scan permanent Clips directory '{}': {error}",
            clips_root.display()
        )
    })?;
    for entry in entries {
        let entry = match entry {
            Ok(value) => value,
            Err(error) => {
                telemetry.failed += 1;
                telemetry
                    .errors
                    .push(format!("Could not read a Clips directory entry: {error}"));
                continue;
            }
        };
        let path = entry.path();
        if !is_reconciliation_candidate(&path) || !path.is_file() {
            continue;
        }
        telemetry.scanned_files += 1;
        let canonical = match validate_owned_clip(clips_root, &path) {
            Ok(value) => value,
            Err(error) => {
                telemetry.failed += 1;
                telemetry.errors.push(error);
                continue;
            }
        };
        let path_string = canonical.to_string_lossy().into_owned();
        seen.insert(path_string.clone());
        let metadata = match fs::metadata(&canonical) {
            Ok(value) => value,
            Err(error) => {
                telemetry.failed += 1;
                telemetry.errors.push(format!(
                    "Could not inspect '{}': {error}",
                    canonical.display()
                ));
                continue;
            }
        };
        let modified_ms = system_time_ms(metadata.modified().unwrap_or(UNIX_EPOCH));
        if by_path.get(&path_string).is_some_and(|existing| {
            existing.file_size_bytes == metadata.len()
                && existing.file_modified_at_ms == modified_ms
        }) {
            telemetry.unchanged += 1;
            continue;
        }
        let inspected = match inspector.inspect(&canonical) {
            Ok(value) => value,
            Err(error) => {
                telemetry.failed += 1;
                telemetry.errors.push(format!(
                    "Could not import '{}': {error}",
                    canonical.display()
                ));
                continue;
            }
        };
        let filename = canonical
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("Replay.mp4")
            .to_string();
        let display_name = canonical
            .file_stem()
            .and_then(|value| value.to_str())
            .unwrap_or("Replay")
            .to_string();
        let existing = by_path.get(&path_string);
        let upsert = ClipUpsert {
            id: existing
                .map(|value| value.id.clone())
                .unwrap_or_else(|| Uuid::new_v4().to_string()),
            file_path: path_string,
            filename,
            display_name,
            created_at_ms: system_time_ms(
                metadata
                    .created()
                    .unwrap_or_else(|_| metadata.modified().unwrap_or(SystemTime::now())),
            ),
            library_added_at_ms: now_ms(),
            file_modified_at_ms: modified_ms,
            file_size_bytes: metadata.len(),
            duration_100ns: inspected.duration_100ns,
            requested_duration_seconds: None,
            width: inspected.width,
            height: inspected.height,
            fps_numerator: inspected.fps_numerator,
            fps_denominator: inspected.fps_denominator,
            video_codec: inspected.video_codec,
            video_profile: inspected.video_profile,
            video_bitrate_bps: inspected.video_bitrate_bps,
            total_bitrate_bps: inspected.total_bitrate_bps,
            capture_target_label: None,
            capture_target_type: None,
            imported_existing_file: true,
            audio_tracks: inspected.audio_tracks,
        };
        match upsert_clip(&mut connection, &upsert) {
            Ok(_) if existing.is_some() => telemetry.updated += 1,
            Ok(_) => telemetry.added += 1,
            Err(error) => {
                telemetry.failed += 1;
                telemetry.errors.push(error);
            }
        }
    }

    let missing = fingerprints
        .iter()
        .filter(|value| {
            !seen.contains(&value.file_path)
                && !Path::new(&value.file_path).is_file()
                && owned_missing_path(clips_root, Path::new(&value.file_path))
        })
        .map(|value| value.id.clone())
        .collect::<Vec<_>>();
    if !missing.is_empty() {
        delete_rows_for_missing_paths(&mut connection, &missing)?;
        telemetry.removed = missing.len() as u64;
    }
    telemetry.duration_ms = started.elapsed().as_secs_f64() * 1_000.0;
    Ok(telemetry)
}

pub fn inspected_from_probe(report: &MediaProbeReport) -> Result<InspectedMedia, String> {
    let videos = report
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .collect::<Vec<_>>();
    if videos.len() != 1 {
        return Err(format!(
            "Expected exactly one video stream, found {}.",
            videos.len()
        ));
    }
    let video = videos[0];
    let duration_seconds = video
        .duration
        .as_deref()
        .and_then(parse_number)
        .or_else(|| {
            report
                .format
                .as_ref()
                .and_then(|format| format.duration.as_deref())
                .and_then(parse_number)
        })
        .filter(|value| *value > 0.0)
        .ok_or_else(|| "Imported clip has no valid duration.".to_string())?;
    let (fps_numerator, fps_denominator) = video
        .avg_frame_rate
        .as_deref()
        .and_then(|value| parse_rational(value).ok())
        .or_else(|| {
            video
                .r_frame_rate
                .as_deref()
                .and_then(|value| parse_rational(value).ok())
        })
        .ok_or_else(|| "Imported clip has no valid frame rate.".to_string())?;
    let audio_tracks = report
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .map(|stream| {
            let title = stream.tags.title.clone();
            let handler = stream.tags.handler_name.clone();
            ClipAudioTrack {
                stream_index: stream.index,
                role: audio_role(title.as_deref().or(handler.as_deref())).to_string(),
                title,
                handler_name: handler,
                codec: stream
                    .codec_name
                    .clone()
                    .unwrap_or_else(|| "unknown".into()),
                profile: stream.profile.clone(),
                sample_rate: stream
                    .sample_rate
                    .as_deref()
                    .and_then(|value| value.parse().ok()),
                channels: stream.channels,
                bitrate_bps: stream
                    .bit_rate
                    .as_deref()
                    .and_then(|value| value.parse().ok()),
                is_default: stream.disposition.is_default == 1,
            }
        })
        .collect::<Vec<_>>();
    Ok(InspectedMedia {
        duration_100ns: (duration_seconds * 10_000_000.0).round() as i64,
        width: video.width.unwrap_or(0),
        height: video.height.unwrap_or(0),
        fps_numerator,
        fps_denominator,
        video_codec: video.codec_name.clone().unwrap_or_else(|| "unknown".into()),
        video_profile: video.profile.clone(),
        video_bitrate_bps: video
            .bit_rate
            .as_deref()
            .and_then(|value| value.parse().ok()),
        total_bitrate_bps: report
            .format
            .as_ref()
            .and_then(|format| format.bit_rate.as_deref())
            .and_then(|value| value.parse().ok()),
        audio_tracks,
    })
}

fn parse_rational(value: &str) -> Result<(u32, u32), String> {
    let (numerator, denominator) = value
        .split_once('/')
        .ok_or_else(|| format!("Invalid frame-rate rational '{value}'."))?;
    let numerator = numerator
        .parse::<u32>()
        .map_err(|_| format!("Invalid frame-rate numerator '{numerator}'."))?;
    let denominator = denominator
        .parse::<u32>()
        .map_err(|_| format!("Invalid frame-rate denominator '{denominator}'."))?;
    if numerator == 0 || denominator == 0 {
        return Err(format!("Invalid zero frame rate '{value}'."));
    }
    Ok((numerator, denominator))
}

fn audio_role(title: Option<&str>) -> &'static str {
    match title
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .as_str()
    {
        "combined" => "Combined",
        "game" => "Game",
        "voice chat" | "voicechat" | "discord" => "VoiceChat",
        "microphone" | "mic" => "Microphone",
        "other" => "Other",
        _ => "Unknown",
    }
}

fn parse_number(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|value| value.is_finite())
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

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::library::database::LibraryDatabase;
    use crate::library::models::ClipListRequest;
    use crate::library::repository::{count_clips, list_clips};

    struct MockInspector {
        calls: AtomicUsize,
    }

    impl MediaInspector for MockInspector {
        fn inspect(&self, _path: &Path) -> Result<InspectedMedia, String> {
            self.calls.fetch_add(1, Ordering::Relaxed);
            Ok(InspectedMedia {
                duration_100ns: 300_000_000,
                width: 2560,
                height: 1440,
                fps_numerator: 60,
                fps_denominator: 1,
                video_codec: "hevc".into(),
                video_profile: Some("Main".into()),
                video_bitrate_bps: Some(14_000_000),
                total_bitrate_bps: Some(14_500_000),
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
            })
        }
    }

    fn test_root() -> PathBuf {
        std::env::temp_dir().join(format!("stage12-reconcile-{}", std::process::id()))
    }

    #[test]
    fn new_unchanged_changed_missing_and_ignored_files_reconcile_without_duplicates() {
        let root = test_root();
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let db_path = root.join("Library").join("clips.db");
        let clips = root.join("Clips");
        fs::create_dir_all(&clips).unwrap();
        let (database, _) = LibraryDatabase::initialize(db_path).unwrap();
        let inspector = MockInspector {
            calls: AtomicUsize::new(0),
        };
        let clip = clips.join("existing.mp4");
        fs::write(&clip, b"one").unwrap();
        fs::write(clips.join("ignored.partial.mp4"), b"partial").unwrap();
        fs::write(clips.join("manifest.txt"), b"text").unwrap();

        let first = reconcile(&database, &clips, &inspector).unwrap();
        assert_eq!((first.added, first.scanned_files), (1, 1));
        assert_eq!(inspector.calls.load(Ordering::Relaxed), 1);
        let second = reconcile(&database, &clips, &inspector).unwrap();
        assert_eq!(second.unchanged, 1);
        assert_eq!(inspector.calls.load(Ordering::Relaxed), 1);

        fs::write(&clip, b"materially changed").unwrap();
        let third = reconcile(&database, &clips, &inspector).unwrap();
        assert_eq!(third.updated, 1);
        assert_eq!(inspector.calls.load(Ordering::Relaxed), 2);
        let connection = database.open().unwrap();
        let (rows, _) = list_clips(&connection, ClipListRequest::default()).unwrap();
        assert_eq!(rows[0].audio_tracks.len(), 1);
        drop(connection);

        fs::remove_file(&clip).unwrap();
        let fourth = reconcile(&database, &clips, &inspector).unwrap();
        assert_eq!(fourth.removed, 1);
        assert_eq!(count_clips(&database.open().unwrap()).unwrap(), 0);
        fs::remove_dir_all(root).unwrap();
    }
}
