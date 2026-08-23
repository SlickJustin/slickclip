use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

use crate::replay::{CompletedSegment, SavedReplayTimeline};

use super::layout::WatchPartyLayout;

const CHECKPOINT_FILE: &str = "watch-party-checkpoint.json";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WatchPartyCheckpoint {
    pub schema_version: u32,
    pub session_id: String,
    pub state: String,
    pub layout: WatchPartyLayout,
    pub main_label: String,
    pub reaction_label: String,
    pub started_at_ms: u64,
    pub segments: Vec<CompletedSegment>,
    pub last_error: Option<String>,
}

impl WatchPartyCheckpoint {
    pub fn write_atomic(&self, session_directory: &Path) -> Result<(), String> {
        let path = session_directory.join(CHECKPOINT_FILE);
        let temporary = session_directory.join(format!("{CHECKPOINT_FILE}.partial"));
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|error| format!("Could not serialize Watch Party checkpoint: {error}"))?;
        let result = (|| {
            let mut file = OpenOptions::new()
                .create(true)
                .truncate(true)
                .write(true)
                .open(&temporary)
                .map_err(|error| {
                    format!(
                        "Could not write Watch Party checkpoint '{}': {error}",
                        temporary.display()
                    )
                })?;
            file.write_all(&bytes)
                .map_err(|error| format!("Could not write Watch Party checkpoint: {error}"))?;
            file.sync_all()
                .map_err(|error| format!("Could not flush Watch Party checkpoint: {error}"))?;
            let source = wide_null(temporary.as_os_str());
            let destination = wide_null(path.as_os_str());
            unsafe {
                MoveFileExW(
                    PCWSTR(source.as_ptr()),
                    PCWSTR(destination.as_ptr()),
                    MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
                )
            }
            .map_err(|error| {
                format!(
                    "Could not atomically promote Watch Party checkpoint '{}': {error}",
                    path.display()
                )
            })
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }

    pub fn read(session_directory: &Path) -> Result<Self, String> {
        let canonical_session = session_directory.canonicalize().map_err(|error| {
            format!(
                "Could not resolve Watch Party recovery directory '{}': {error}",
                session_directory.display()
            )
        })?;
        let path = canonical_session.join(CHECKPOINT_FILE);
        let checkpoint: Self = serde_json::from_slice(&fs::read(&path).map_err(|error| {
            format!(
                "Could not read Watch Party checkpoint '{}': {error}",
                path.display()
            )
        })?)
        .map_err(|error| format!("Watch Party checkpoint is invalid: {error}"))?;
        if checkpoint.schema_version != 1 || checkpoint.segments.is_empty() {
            return Err("Watch Party checkpoint has no recoverable finalized video.".to_string());
        }
        for segment in &checkpoint.segments {
            let source = PathBuf::from(&segment.file_path);
            let canonical = source.canonicalize().map_err(|error| {
                format!(
                    "Could not resolve recovered segment '{}': {error}",
                    source.display()
                )
            })?;
            if canonical.parent() != Some(canonical_session.as_path())
                || canonical.extension().and_then(|value| value.to_str()) != Some("mp4")
                || !segment.finalized
                || fs::metadata(&canonical)
                    .map(|value| value.len())
                    .unwrap_or(0)
                    == 0
            {
                return Err(format!(
                    "Recovered segment '{}' is outside the session or is not finalized.",
                    source.display()
                ));
            }
        }
        SavedReplayTimeline::from_segments(&checkpoint.segments)?;
        Ok(checkpoint)
    }
}

fn wide_null(value: &std::ffi::OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

pub fn recoverable_sessions(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut sessions = entries
        .flatten()
        .filter_map(|entry| {
            entry
                .file_type()
                .ok()
                .filter(|kind| kind.is_dir())
                .map(|_| entry.path())
        })
        .filter(|path| {
            WatchPartyCheckpoint::read(path)
                .map(|checkpoint| checkpoint.state != "completed")
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    sessions.sort();
    sessions
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::segment::VideoFrameTimingPoint;

    fn segment(path: PathBuf) -> CompletedSegment {
        CompletedSegment {
            sequence_number: 1,
            file_path: path.to_string_lossy().into_owned(),
            start_timestamp_ms: 1,
            end_timestamp_ms: 1_001,
            actual_duration_ms: 1_000,
            segment_session_start_qpc_100ns: 0,
            segment_session_end_qpc_100ns: 10_000_000,
            first_frame_timestamp_100ns: 100,
            last_frame_timestamp_100ns: 9_666_766,
            encoded_start_pts_100ns: 0,
            encoded_last_frame_pts_100ns: 9_666_666,
            encoded_end_pts_100ns: 10_000_000,
            encoded_duration_100ns: 10_000_000,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: 10_000_000,
            frame_timing_points: (0..30)
                .map(|frame_index| VideoFrameTimingPoint {
                    frame_index,
                    output_qpc_100ns: (frame_index * 10_000_000 / 30) as i64,
                    source_qpc_100ns: 100 + (frame_index * 10_000_000 / 30) as i64,
                    encoded_pts_100ns: (frame_index * 10_000_000 / 30) as i64,
                    fresh_source: true,
                })
                .collect(),
            next_segment_first_frame_timestamp_100ns: None,
            source_frame_gap_ms: None,
            source_update_count: 30,
            fresh_output_frame_count: 30,
            held_output_frame_count: 0,
            frame_count: 30,
            encoder_creation_time_ms: 0.0,
            encoder_creation_started_ms: 0.0,
            encoder_creation_completed_ms: 0.0,
            rotation_requested_ms: None,
            first_frame_submitted_ms: Some(0.0),
            last_frame_submitted_ms: Some(966.666),
            next_first_frame_submitted_ms: None,
            codec: "H.264".to_string(),
            width: 1920,
            height: 1080,
            frame_rate: 30,
            file_size: 1,
            average_bitrate_mbps: 0.000008,
            finalized: true,
            finalization_time_ms: 0.0,
            rotation_gap_ms: None,
        }
    }

    #[test]
    fn atomic_checkpoint_recovers_only_direct_finalized_segments() {
        let root =
            std::env::temp_dir().join(format!("slickclip-watch-checkpoint-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let video = root.join("segment-000001.mp4");
        fs::write(&video, b"video").unwrap();
        let checkpoint = WatchPartyCheckpoint {
            schema_version: 1,
            session_id: "test".to_string(),
            state: "recording".to_string(),
            layout: WatchPartyLayout::ReactionsRight,
            main_label: "Main".to_string(),
            reaction_label: "Discord".to_string(),
            started_at_ms: 1,
            segments: vec![segment(video)],
            last_error: None,
        };
        checkpoint.write_atomic(&root).unwrap();
        checkpoint.write_atomic(&root).unwrap();
        assert_eq!(WatchPartyCheckpoint::read(&root).unwrap().segments.len(), 1);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn recovery_rejects_a_segment_outside_its_session() {
        let base = std::env::temp_dir().join(format!(
            "slickclip-watch-checkpoint-outside-{}",
            std::process::id()
        ));
        let session = base.join("session");
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&session).unwrap();
        let outside = base.join("outside.mp4");
        fs::write(&outside, b"video").unwrap();
        WatchPartyCheckpoint {
            schema_version: 1,
            session_id: "test".to_string(),
            state: "recording".to_string(),
            layout: WatchPartyLayout::ReactionsRight,
            main_label: "Main".to_string(),
            reaction_label: "Discord".to_string(),
            started_at_ms: 1,
            segments: vec![segment(outside)],
            last_error: None,
        }
        .write_atomic(&session)
        .unwrap();
        assert!(WatchPartyCheckpoint::read(&session).is_err());
        let _ = fs::remove_dir_all(base);
    }
}
