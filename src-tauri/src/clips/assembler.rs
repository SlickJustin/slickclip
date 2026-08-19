use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::replay::CompletedSegment;

use super::ffmpeg::FfmpegExecutable;

pub struct ClipAssemblyResult {
    pub output_path: PathBuf,
    pub file_size: u64,
    pub actual_duration_seconds: f64,
    pub earliest_timestamp_ms: u64,
    pub latest_timestamp_ms: u64,
    pub codec: String,
    pub internal_encoded_duration_seconds: f64,
    pub ffprobe_duration_seconds: Option<f64>,
    pub internal_ffprobe_difference_ms: Option<f64>,
}

pub trait ClipAssembler {
    fn assemble(
        &self,
        segments: &[CompletedSegment],
        output_directory: &Path,
        timestamp: &str,
    ) -> Result<ClipAssemblyResult, String>;
}

pub struct FfmpegClipAssembler;

impl ClipAssembler for FfmpegClipAssembler {
    fn assemble(
        &self,
        segments: &[CompletedSegment],
        output_directory: &Path,
        timestamp: &str,
    ) -> Result<ClipAssemblyResult, String> {
        validate_compatible_segments(segments)?;
        fs::create_dir_all(output_directory).map_err(|error| {
            format!(
                "Could not create the Clips directory '{}': {error}",
                output_directory.display()
            )
        })?;

        let paths = choose_output_paths(output_directory, timestamp)?;
        if let Err(error) = write_concat_manifest(&paths.manifest, segments) {
            let _ = fs::remove_file(&paths.manifest);
            return Err(error);
        }

        let mut promoted = false;
        let assembly_result = (|| {
            let ffmpeg = FfmpegExecutable::resolve()?;
            let output = ffmpeg.concat_stream_copy(&paths.manifest, &paths.partial)?;
            if !output.status.success() {
                let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
                return Err(if stderr.is_empty() {
                    format!("FFmpeg exited unsuccessfully with {}.", output.status)
                } else {
                    format!("FFmpeg could not assemble the replay: {stderr}")
                });
            }

            let ffprobe_duration_seconds =
                ffmpeg.validate_packet_timeline_if_available(&paths.partial)?;

            let partial_metadata = fs::metadata(&paths.partial).map_err(|error| {
                format!(
                    "FFmpeg reported success, but its partial output '{}' could not be verified: {error}",
                    paths.partial.display()
                )
            })?;
            if partial_metadata.len() == 0 {
                return Err("FFmpeg produced an empty replay file.".to_string());
            }
            if paths.final_path.exists() {
                return Err(format!(
                    "The final replay path '{}' appeared while the clip was being assembled; it will not be overwritten.",
                    paths.final_path.display()
                ));
            }
            fs::rename(&paths.partial, &paths.final_path).map_err(|error| {
                format!(
                    "The verified replay could not be promoted to '{}': {error}",
                    paths.final_path.display()
                )
            })?;
            promoted = true;

            let final_size = fs::metadata(&paths.final_path)
                .map_err(|error| format!("The saved replay could not be verified: {error}"))?
                .len();
            if final_size == 0 {
                return Err("The saved replay is empty after its final rename.".to_string());
            }

            let first = &segments[0];
            let last = &segments[segments.len() - 1];
            let internal_encoded_duration_seconds = segments
                .iter()
                .map(|segment| segment.encoded_duration_100ns as f64 / 10_000_000.0)
                .sum::<f64>();
            let internal_ffprobe_difference_ms = ffprobe_duration_seconds
                .map(|duration| (internal_encoded_duration_seconds - duration) * 1_000.0);
            #[cfg(debug_assertions)]
            if let Some(difference_ms) = internal_ffprobe_difference_ms {
                let tolerance_ms = (2_000.0 / f64::from(first.frame_rate.max(1))).max(50.0);
                if difference_ms.abs() > tolerance_ms {
                    return Err(format!(
                        "Internal encoded duration ({internal_encoded_duration_seconds:.6} s) differs from ffprobe ({:.6} s) by {difference_ms:.3} ms, beyond the {tolerance_ms:.3} ms development tolerance.",
                        ffprobe_duration_seconds.unwrap_or_default()
                    ));
                }
            }
            Ok(ClipAssemblyResult {
                output_path: paths.final_path.clone(),
                file_size: final_size,
                actual_duration_seconds: ffprobe_duration_seconds
                    .unwrap_or(internal_encoded_duration_seconds),
                earliest_timestamp_ms: first.start_timestamp_ms,
                latest_timestamp_ms: last.end_timestamp_ms,
                codec: first.codec.clone(),
                internal_encoded_duration_seconds,
                ffprobe_duration_seconds,
                internal_ffprobe_difference_ms,
            })
        })();

        let _ = fs::remove_file(&paths.manifest);
        if assembly_result.is_err() {
            let _ = fs::remove_file(&paths.partial);
            if promoted {
                let _ = fs::remove_file(&paths.final_path);
            }
        }
        assembly_result
    }
}

pub fn validate_compatible_segments(segments: &[CompletedSegment]) -> Result<(), String> {
    let Some(first) = segments.first() else {
        return Err("No finalized replay segments were selected.".to_string());
    };
    if !first.finalized || first.file_size == 0 {
        return Err("The first selected replay segment is incomplete or empty.".to_string());
    }

    for segment in segments {
        if !segment.finalized || segment.file_size == 0 {
            return Err(format!(
                "Replay segment {} is incomplete or empty.",
                segment.sequence_number
            ));
        }
        if segment.codec != first.codec
            || segment.width != first.width
            || segment.height != first.height
            || segment.frame_rate != first.frame_rate
        {
            return Err(format!(
                "Replay segment {} is incompatible with the selected stream. Expected {} {}x{} at {} FPS, found {} {}x{} at {} FPS.",
                segment.sequence_number,
                first.codec,
                first.width,
                first.height,
                first.frame_rate,
                segment.codec,
                segment.width,
                segment.height,
                segment.frame_rate
            ));
        }

        let path = Path::new(&segment.file_path);
        let metadata = fs::metadata(path).map_err(|error| {
            format!(
                "Replay segment {} is no longer available at '{}': {error}",
                segment.sequence_number,
                path.display()
            )
        })?;
        if metadata.len() == 0 {
            return Err(format!(
                "Replay segment {} exists but is empty.",
                segment.sequence_number
            ));
        }
    }

    for pair in segments.windows(2) {
        let previous = &pair[0];
        let current = &pair[1];
        if current.sequence_number <= previous.sequence_number
            || current.start_timestamp_ms < previous.start_timestamp_ms
        {
            return Err(format!(
                "Replay segments are not in chronological order at sequence {} followed by {}.",
                previous.sequence_number, current.sequence_number
            ));
        }
    }

    Ok(())
}

struct OutputPaths {
    final_path: PathBuf,
    partial: PathBuf,
    manifest: PathBuf,
}

fn choose_output_paths(output_directory: &Path, timestamp: &str) -> Result<OutputPaths, String> {
    for suffix in 0..1_000 {
        let stem = if suffix == 0 {
            format!("JustInReplay-{timestamp}")
        } else {
            format!("JustInReplay-{timestamp}-{suffix:03}")
        };
        let final_path = output_directory.join(format!("{stem}.mp4"));
        let partial = output_directory.join(format!("{stem}.partial.mp4"));
        let manifest = output_directory.join(format!(".{stem}.concat.txt"));
        if !final_path.exists() && !partial.exists() && !manifest.exists() {
            return Ok(OutputPaths {
                final_path,
                partial,
                manifest,
            });
        }
    }

    Err("Could not reserve a collision-safe replay filename.".to_string())
}

fn write_concat_manifest(path: &Path, segments: &[CompletedSegment]) -> Result<(), String> {
    let mut manifest = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| {
            format!(
                "Could not create the temporary concat manifest '{}': {error}",
                path.display()
            )
        })?;

    writeln!(manifest, "ffconcat version 1.0")
        .map_err(|error| format!("Could not write the concat manifest: {error}"))?;
    for segment in segments {
        let normalized = segment.file_path.replace('\\', "/");
        let escaped = normalized.replace('\'', "'\\''");
        writeln!(manifest, "file '{escaped}'")
            .map_err(|error| format!("Could not write the concat manifest: {error}"))?;
    }
    manifest
        .flush()
        .map_err(|error| format!("Could not flush the concat manifest: {error}"))
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{choose_output_paths, validate_compatible_segments, write_concat_manifest};
    use crate::replay::CompletedSegment;

    fn segment(path: &Path, sequence_number: u64, codec: &str) -> CompletedSegment {
        CompletedSegment {
            sequence_number,
            file_path: path.to_string_lossy().into_owned(),
            start_timestamp_ms: sequence_number * 2_000,
            end_timestamp_ms: (sequence_number + 1) * 2_000,
            actual_duration_ms: 2_000,
            segment_session_start_qpc_100ns: i64::try_from(sequence_number * 20_000_000).unwrap(),
            segment_session_end_qpc_100ns: i64::try_from((sequence_number + 1) * 20_000_000)
                .unwrap(),
            first_frame_timestamp_100ns: 0,
            last_frame_timestamp_100ns: 20_000_000,
            encoded_start_pts_100ns: 0,
            encoded_last_frame_pts_100ns: 19_833_334,
            encoded_end_pts_100ns: 20_000_000,
            encoded_duration_100ns: 20_000_000,
            encoded_time_base_numerator: 1,
            encoded_time_base_denominator: 10_000_000,
            frame_timing_points: vec![crate::replay::segment::VideoFrameTimingPoint {
                frame_index: 0,
                output_qpc_100ns: i64::try_from(sequence_number * 20_000_000).unwrap(),
                source_qpc_100ns: 0,
                encoded_pts_100ns: 0,
                fresh_source: true,
            }],
            next_segment_first_frame_timestamp_100ns: None,
            source_frame_gap_ms: None,
            source_update_count: 1,
            fresh_output_frame_count: 1,
            held_output_frame_count: 119,
            frame_count: 120,
            encoder_creation_time_ms: 10.0,
            encoder_creation_started_ms: 0.0,
            encoder_creation_completed_ms: 10.0,
            rotation_requested_ms: None,
            first_frame_submitted_ms: Some(0.0),
            last_frame_submitted_ms: Some(2_000.0),
            next_first_frame_submitted_ms: None,
            codec: codec.to_string(),
            width: 1920,
            height: 1080,
            frame_rate: 60,
            file_size: 4,
            average_bitrate_mbps: 0.000016,
            finalized: true,
            finalization_time_ms: 10.0,
            rotation_gap_ms: Some(2.0),
        }
    }

    fn test_directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "justin-replay-stage7-{name}-{}",
            std::process::id()
        ))
    }

    #[test]
    fn incompatible_codec_is_rejected() {
        let directory = test_directory("compatibility");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let first_path = directory.join("one.mp4");
        let second_path = directory.join("two.mp4");
        fs::write(&first_path, b"test").unwrap();
        fs::write(&second_path, b"test").unwrap();

        let segments = vec![
            segment(&first_path, 1, "H.264"),
            segment(&second_path, 2, "HEVC"),
        ];
        assert!(validate_compatible_segments(&segments).is_err());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn filename_collision_uses_a_numbered_suffix() {
        let directory = test_directory("collision");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        fs::write(
            directory.join("JustInReplay-20260815-234512.mp4"),
            b"existing",
        )
        .unwrap();

        let paths = choose_output_paths(&directory, "20260815-234512").unwrap();
        assert_eq!(
            paths.final_path.file_name().unwrap(),
            "JustInReplay-20260815-234512-001.mp4"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn manifest_preserves_windows_paths_and_chronological_order() {
        let directory = test_directory("manifest-order");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("segments.ffconcat");
        let mut first = segment(Path::new(r"C:\Replay Buffer\segment-000001.mp4"), 1, "HEVC");
        first.actual_duration_ms = 1_983;
        let second = segment(
            Path::new(r"C:\Replay Buffer\segment-'000002'.mp4"),
            2,
            "HEVC",
        );

        write_concat_manifest(&manifest, &[first, second]).unwrap();

        assert_eq!(
            fs::read_to_string(&manifest).unwrap(),
            "ffconcat version 1.0\nfile 'C:/Replay Buffer/segment-000001.mp4'\nfile 'C:/Replay Buffer/segment-'\\''000002'\\''.mp4'\n"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn manifest_does_not_override_healthy_container_duration() {
        let directory = test_directory("manifest-duration");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let manifest = directory.join("segments.ffconcat");
        let mut fractional = segment(Path::new(r"C:\Replay\fractional.mp4"), 1, "H.264");
        fractional.actual_duration_ms = 1_983;

        write_concat_manifest(&manifest, &[fractional]).unwrap();

        let contents = fs::read_to_string(&manifest).unwrap();
        assert!(!contents.contains("duration"));
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn out_of_order_segments_are_rejected() {
        let directory = test_directory("out-of-order");
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let first_path = directory.join("one.mp4");
        let second_path = directory.join("two.mp4");
        fs::write(&first_path, b"test").unwrap();
        fs::write(&second_path, b"test").unwrap();

        let segments = vec![
            segment(&second_path, 2, "H.264"),
            segment(&first_path, 1, "H.264"),
        ];
        assert!(validate_compatible_segments(&segments).is_err());
        fs::remove_dir_all(directory).unwrap();
    }
}
