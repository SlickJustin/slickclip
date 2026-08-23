use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use serde::Serialize;

use crate::replay::{CompletedSegment, ReplaySaveSnapshot};

use super::audio_render::{render_audio_tracks, AudioRenderDiagnostics, RenderedAudioTrack};
use super::ffmpeg::{build_audio_mux_plan, FfmpegExecutable, MediaProbeReport, MediaProbeStream};

static SAVE_WORKSPACE_COUNTER: AtomicU64 = AtomicU64::new(0);
#[cfg(debug_assertions)]
const AUDIO_DURATION_TOLERANCE_SECONDS: f64 = 0.150;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalAudioStreamDiagnostics {
    pub stream_index: u32,
    pub title: String,
    pub codec: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub is_default: bool,
    pub duration_seconds: Option<f64>,
    pub bitrate_kbps: Option<f64>,
}

#[derive(Clone, Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct FinalMuxDiagnostics {
    pub ffmpeg_duration_ms: f64,
    pub ffmpeg_exit_status: String,
    pub final_stream_count: usize,
    pub video_stream_count: usize,
    pub audio_stream_count: usize,
    pub audio_titles: Vec<String>,
    pub audio_streams: Vec<FinalAudioStreamDiagnostics>,
    pub video_bitrate_mbps: Option<f64>,
    pub video_profile: Option<String>,
    pub total_bitrate_mbps: Option<f64>,
    pub container_duration_seconds: Option<f64>,
    pub filter_graph: Option<String>,
    pub verified: bool,
}

#[derive(Clone, Debug)]
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
    pub audio_render_diagnostics: Vec<AudioRenderDiagnostics>,
    pub final_mux: FinalMuxDiagnostics,
}

#[derive(Clone, Debug)]
pub struct ClipAssemblyFailure {
    pub message: String,
    pub temporary_workspace_path: Option<PathBuf>,
    pub temporary_video_path: Option<PathBuf>,
    pub temporary_artifacts_retained: bool,
}

impl ClipAssemblyFailure {
    pub fn without_artifacts(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            temporary_workspace_path: None,
            temporary_video_path: None,
            temporary_artifacts_retained: false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum ClipAssemblyPhase {
    AssemblingVideo,
    RenderingAudio,
    Muxing,
    Verifying,
    Promoting,
}

pub struct FfmpegClipAssembler;

impl FfmpegClipAssembler {
    pub fn assemble(
        &self,
        snapshot: &ReplaySaveSnapshot,
        output_directory: &Path,
        timestamp: &str,
        on_phase: &dyn Fn(ClipAssemblyPhase),
    ) -> Result<ClipAssemblyResult, ClipAssemblyFailure> {
        let mut workspace = None::<SaveWorkspace>;
        let result = (|| -> Result<ClipAssemblyResult, String> {
            validate_compatible_segments(&snapshot.segments)?;
            fs::create_dir_all(output_directory).map_err(|error| {
                format!(
                    "Could not create the Clips directory '{}': {error}",
                    output_directory.display()
                )
            })?;
            let final_path = choose_output_path(output_directory, timestamp)?;
            let active_workspace = create_workspace(output_directory, timestamp)?;
            write_concat_manifest(&active_workspace.manifest, &snapshot.segments)?;
            workspace = Some(active_workspace);
            let paths = workspace.as_ref().expect("workspace was assigned");

            let ffmpeg = FfmpegExecutable::resolve()?;
            on_phase(ClipAssemblyPhase::AssemblingVideo);
            let output = ffmpeg.concat_stream_copy(&paths.manifest, &paths.video_only)?;
            if !output.status.success() {
                return Err(ffmpeg_failure("assemble the video-only replay", &output));
            }
            let video_duration = ffmpeg
                .validate_packet_timeline_if_available(&paths.video_only)?
                .ok_or_else(|| {
                    "ffprobe is required to validate the Stage 11 video-only assembly.".to_string()
                })?;
            let video_size = nonempty_file_size(&paths.video_only, "video-only replay")?;

            on_phase(ClipAssemblyPhase::RenderingAudio);
            let rendered_audio = render_audio_tracks(
                &snapshot.audio_snapshot_tracks,
                &snapshot.video_timeline,
                &paths.directory,
            )?;
            on_phase(ClipAssemblyPhase::Muxing);
            let mux_started = Instant::now();
            let mut mux_filter_graph = None;
            let mut mux_audio_titles = None;
            let ffmpeg_exit_status = if rendered_audio.is_empty() {
                fs::copy(&paths.video_only, &paths.final_partial).map_err(|error| {
                    format!("Could not stage the video-only replay in its atomic output: {error}")
                })?;
                "video-only stream copy".to_string()
            } else {
                let plan = build_audio_mux_plan(
                    &paths.video_only,
                    &rendered_audio,
                    snapshot.video_timeline.clip_playback_duration_100ns,
                    &paths.final_partial,
                )?;
                mux_filter_graph = Some(plan.filter_graph.clone());
                mux_audio_titles = Some(plan.audio_titles.clone());
                let output = ffmpeg.mux_audio(&plan)?;
                if !output.status.success() {
                    return Err(ffmpeg_failure("mux the final audio streams", &output));
                }
                output.status.to_string()
            };
            let ffmpeg_duration_ms = mux_started.elapsed().as_secs_f64() * 1_000.0;
            nonempty_file_size(&paths.final_partial, "final partial replay")?;

            on_phase(ClipAssemblyPhase::Verifying);
            let source_probe = ffmpeg.inspect_media(&paths.video_only)?;
            let final_probe = ffmpeg.inspect_media(&paths.final_partial)?;
            let mut final_mux = verify_final_media(
                &source_probe,
                &final_probe,
                &rendered_audio,
                video_duration,
                video_size,
            )?;
            final_mux.ffmpeg_duration_ms = ffmpeg_duration_ms;
            final_mux.ffmpeg_exit_status = ffmpeg_exit_status;
            final_mux.filter_graph = mux_filter_graph;
            if let Some(titles) = mux_audio_titles {
                debug_assert_eq!(final_mux.audio_titles, titles);
            }

            let final_size = nonempty_file_size(&paths.final_partial, "verified partial replay")?;
            let first = &snapshot.segments[0];
            let last = snapshot.segments.last().expect("validated segments");
            let internal_duration =
                snapshot.video_timeline.clip_playback_duration_100ns as f64 / 10_000_000.0;
            let difference_ms = (internal_duration - video_duration) * 1_000.0;
            #[cfg(debug_assertions)]
            {
                let tolerance_ms = (2_000.0 / f64::from(first.frame_rate.max(1))).max(50.0);
                if difference_ms.abs() > tolerance_ms {
                    return Err(format!(
                        "Internal video duration ({internal_duration:.6} s) differs from ffprobe ({video_duration:.6} s) by {difference_ms:.3} ms, beyond the {tolerance_ms:.3} ms development tolerance."
                    ));
                }
            }

            on_phase(ClipAssemblyPhase::Promoting);
            if final_path.exists() {
                return Err(format!(
                    "The final replay path '{}' appeared while Save was running; it will not be overwritten.",
                    final_path.display()
                ));
            }
            fs::rename(&paths.final_partial, &final_path).map_err(|error| {
                format!(
                    "The verified replay could not be atomically promoted to '{}': {error}",
                    final_path.display()
                )
            })?;
            Ok(ClipAssemblyResult {
                output_path: final_path,
                file_size: final_size,
                actual_duration_seconds: final_mux
                    .container_duration_seconds
                    .unwrap_or(video_duration),
                earliest_timestamp_ms: first.start_timestamp_ms,
                latest_timestamp_ms: last.end_timestamp_ms,
                codec: first.codec.clone(),
                internal_encoded_duration_seconds: internal_duration,
                ffprobe_duration_seconds: Some(video_duration),
                internal_ffprobe_difference_ms: Some(difference_ms),
                audio_render_diagnostics: rendered_audio
                    .iter()
                    .map(|track| track.diagnostics.clone())
                    .collect(),
                final_mux,
            })
        })();

        match result {
            Ok(result) => {
                if let Some(paths) = &workspace {
                    let _ = cleanup_workspace(&paths.temp_root, &paths.directory);
                }
                Ok(result)
            }
            Err(message) => {
                let temporary_workspace_path =
                    workspace.as_ref().map(|paths| paths.directory.clone());
                let temporary_video_path = workspace
                    .as_ref()
                    .filter(|paths| paths.video_only.is_file())
                    .map(|paths| paths.video_only.clone());
                let retain = cfg!(debug_assertions) && temporary_video_path.is_some();
                if !retain {
                    if let Some(paths) = &workspace {
                        let _ = cleanup_workspace(&paths.temp_root, &paths.directory);
                    }
                }
                Err(ClipAssemblyFailure {
                    message,
                    temporary_workspace_path,
                    temporary_video_path,
                    temporary_artifacts_retained: retain,
                })
            }
        }
    }
}

fn ffmpeg_failure(action: &str, output: &std::process::Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    if stderr.is_empty() {
        format!(
            "FFmpeg could not {action}; it exited with {}.",
            output.status
        )
    } else {
        format!("FFmpeg could not {action}: {stderr}")
    }
}

fn nonempty_file_size(path: &Path, description: &str) -> Result<u64, String> {
    let size = fs::metadata(path)
        .map_err(|error| {
            format!(
                "Could not inspect {description} '{}': {error}",
                path.display()
            )
        })?
        .len();
    if size == 0 {
        Err(format!("The {description} is empty."))
    } else {
        Ok(size)
    }
}

fn verify_final_media(
    source: &MediaProbeReport,
    final_report: &MediaProbeReport,
    rendered_audio: &[RenderedAudioTrack],
    video_duration_seconds: f64,
    video_size: u64,
) -> Result<FinalMuxDiagnostics, String> {
    let source_video = exactly_one_video(source, "temporary video")?;
    let final_video = exactly_one_video(final_report, "final replay")?;
    if source_video.codec_name != final_video.codec_name
        || source_video.width != final_video.width
        || source_video.height != final_video.height
        || source_video.r_frame_rate != final_video.r_frame_rate
        || source_video.avg_frame_rate != final_video.avg_frame_rate
    {
        return Err(
            "Final mux changed the verified video codec, resolution, or frame rate.".to_string(),
        );
    }
    let audio = final_report
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("audio"))
        .collect::<Vec<_>>();
    let expected_audio_count = if rendered_audio.is_empty() {
        0
    } else {
        rendered_audio.len() + 1
    };
    if audio.len() != expected_audio_count {
        return Err(format!(
            "Final replay has {} audio streams; expected {expected_audio_count}.",
            audio.len()
        ));
    }
    let mut expected_titles = Vec::new();
    if !rendered_audio.is_empty() {
        expected_titles.push("Combined".to_string());
        expected_titles.extend(
            rendered_audio
                .iter()
                .map(|track| super::ffmpeg::track_title(track.track_role).to_string()),
        );
    }
    let mut streams = Vec::new();
    for (index, stream) in audio.iter().enumerate() {
        let title = stream
            .tags
            .title
            .as_ref()
            .or(stream.tags.handler_name.as_ref())
            .cloned()
            .unwrap_or_default();
        if title != expected_titles[index] {
            return Err(format!(
                "Final audio stream {index} is titled '{title}', expected '{}'.",
                expected_titles[index]
            ));
        }
        if stream.codec_name.as_deref() != Some("aac")
            || stream.profile.as_deref() != Some("LC")
            || stream.sample_rate.as_deref() != Some("48000")
            || stream.channels != Some(2)
        {
            return Err(format!(
                "Final audio stream '{title}' is not AAC-LC-compatible 48 kHz stereo."
            ));
        }
        let is_default = stream.disposition.is_default == 1;
        if is_default != (index == 0) {
            return Err("Combined must be the only default audio stream.".to_string());
        }
        let duration = stream
            .duration
            .as_deref()
            .and_then(parse_number)
            .ok_or_else(|| format!("Final audio stream '{title}' has no valid duration."))?;
        #[cfg(debug_assertions)]
        if (duration - video_duration_seconds).abs() > AUDIO_DURATION_TOLERANCE_SECONDS {
            return Err(format!(
                "Final audio stream '{title}' differs materially from video duration {video_duration_seconds:.6} s (audio {:.6} s).",
                duration
            ));
        }
        streams.push(FinalAudioStreamDiagnostics {
            stream_index: stream.index,
            title,
            codec: stream.codec_name.clone().unwrap_or_default(),
            sample_rate: stream
                .sample_rate
                .as_deref()
                .and_then(|value| value.parse().ok())
                .unwrap_or_default(),
            channels: stream.channels.unwrap_or_default(),
            is_default,
            duration_seconds: Some(duration),
            bitrate_kbps: stream
                .bit_rate
                .as_deref()
                .and_then(parse_number)
                .map(|value| value / 1_000.0),
        });
    }
    let format_duration = final_report
        .format
        .as_ref()
        .and_then(|format| format.duration.as_deref())
        .and_then(parse_number);
    let total_bitrate = final_report
        .format
        .as_ref()
        .and_then(|format| format.bit_rate.as_deref())
        .and_then(parse_number)
        .map(|value| value / 1_000_000.0);
    let video_bitrate = source_video
        .bit_rate
        .as_deref()
        .and_then(parse_number)
        .map(|value| value / 1_000_000.0)
        .or_else(|| {
            (video_duration_seconds > 0.0)
                .then_some(video_size as f64 * 8.0 / video_duration_seconds / 1_000_000.0)
        });
    Ok(FinalMuxDiagnostics {
        ffmpeg_duration_ms: 0.0,
        ffmpeg_exit_status: String::new(),
        final_stream_count: final_report.streams.len(),
        video_stream_count: 1,
        audio_stream_count: audio.len(),
        audio_titles: expected_titles,
        audio_streams: streams,
        video_bitrate_mbps: video_bitrate,
        video_profile: source_video.profile.clone(),
        total_bitrate_mbps: total_bitrate,
        container_duration_seconds: format_duration,
        filter_graph: None,
        verified: true,
    })
}

fn exactly_one_video<'a>(
    report: &'a MediaProbeReport,
    description: &str,
) -> Result<&'a MediaProbeStream, String> {
    let videos = report
        .streams
        .iter()
        .filter(|stream| stream.codec_type.as_deref() == Some("video"))
        .collect::<Vec<_>>();
    if videos.len() == 1 {
        Ok(videos[0])
    } else {
        Err(format!(
            "The {description} has {} video streams; expected exactly one.",
            videos.len()
        ))
    }
}

fn parse_number(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|value| value.is_finite())
}

struct SaveWorkspace {
    temp_root: PathBuf,
    directory: PathBuf,
    manifest: PathBuf,
    video_only: PathBuf,
    final_partial: PathBuf,
}

fn create_workspace(output_directory: &Path, timestamp: &str) -> Result<SaveWorkspace, String> {
    let temp_root = output_directory.join(".slickclip-temp");
    fs::create_dir_all(&temp_root)
        .map_err(|error| format!("Could not create the Save temp root: {error}"))?;
    for _ in 0..1_000 {
        let id = SAVE_WORKSPACE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let directory = temp_root.join(format!("save-{timestamp}-{}-{id}", std::process::id()));
        match fs::create_dir(&directory) {
            Ok(()) => {
                return Ok(SaveWorkspace {
                    temp_root,
                    manifest: directory.join("video.ffconcat"),
                    video_only: directory.join("video-only.mp4"),
                    final_partial: directory.join("final.partial.mp4"),
                    directory,
                });
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Could not create a Save workspace: {error}")),
        }
    }
    Err("Could not reserve a unique Save workspace.".to_string())
}

fn cleanup_workspace(temp_root: &Path, workspace: &Path) -> Result<(), String> {
    if workspace.parent() != Some(temp_root)
        || !workspace
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("save-"))
    {
        return Err(format!(
            "Refused to clean a path outside the owned Save temp root: '{}'.",
            workspace.display()
        ));
    }
    if workspace.exists() {
        fs::remove_dir_all(workspace)
            .map_err(|error| format!("Could not clean Save workspace: {error}"))?;
    }
    Ok(())
}

fn choose_output_path(output_directory: &Path, timestamp: &str) -> Result<PathBuf, String> {
    for suffix in 0..1_000 {
        let stem = if suffix == 0 {
            format!("SlickClip-{timestamp}")
        } else {
            format!("SlickClip-{timestamp}-{suffix:03}")
        };
        let final_path = output_directory.join(format!("{stem}.mp4"));
        if !final_path.exists() {
            return Ok(final_path);
        }
    }
    Err("Could not reserve a collision-safe replay filename.".to_string())
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

    use crate::audio::AudioFormatMetadata;
    use crate::replay::AudioTrackRole;

    use super::super::audio_render::{AudioRenderDiagnostics, RenderedAudioTrack};
    use super::super::ffmpeg::{
        MediaProbeDisposition, MediaProbeFormat, MediaProbeReport, MediaProbeStream, MediaProbeTags,
    };
    use super::{
        choose_output_path, cleanup_workspace, create_workspace, validate_compatible_segments,
        verify_final_media, write_concat_manifest,
    };
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
        std::env::temp_dir().join(format!("slickclip-stage7-{name}-{}", std::process::id()))
    }

    fn probe_stream(
        index: u32,
        kind: &str,
        title: Option<&str>,
        default: bool,
    ) -> MediaProbeStream {
        MediaProbeStream {
            index,
            codec_name: Some(if kind == "video" { "hevc" } else { "aac" }.into()),
            profile: Some(if kind == "video" { "Main" } else { "LC" }.into()),
            codec_type: Some(kind.into()),
            width: (kind == "video").then_some(2560),
            height: (kind == "video").then_some(1440),
            pix_fmt: (kind == "video").then(|| "yuv420p".into()),
            r_frame_rate: (kind == "video").then(|| "60/1".into()),
            avg_frame_rate: (kind == "video").then(|| "60/1".into()),
            sample_rate: (kind == "audio").then(|| "48000".into()),
            channels: (kind == "audio").then_some(2),
            duration: Some("30.000000".into()),
            bit_rate: Some(
                if kind == "video" {
                    "14000000"
                } else {
                    "192000"
                }
                .into(),
            ),
            tags: MediaProbeTags {
                title: title.map(str::to_string),
                handler_name: title.map(str::to_string),
            },
            disposition: MediaProbeDisposition {
                is_default: u8::from(default),
            },
        }
    }

    fn probe(streams: Vec<MediaProbeStream>) -> MediaProbeReport {
        MediaProbeReport {
            streams,
            format: Some(MediaProbeFormat {
                format_name: Some("mov,mp4,m4a,3gp,3g2,mj2".into()),
                duration: Some("30.000000".into()),
                bit_rate: Some("14500000".into()),
            }),
        }
    }

    fn rendered(role: AudioTrackRole) -> RenderedAudioTrack {
        RenderedAudioTrack {
            track_role: role,
            path: PathBuf::from(format!("{}.wav", role.directory_name())),
            diagnostics: AudioRenderDiagnostics {
                track_role: role,
                selected_segment_sequence_numbers: vec![1],
                source_format: AudioFormatMetadata {
                    sample_format: "IEEE float".into(),
                    format_tag: 3,
                    sample_rate: 48_000,
                    channel_count: 2,
                    bits_per_sample: 32,
                    valid_bits_per_sample: Some(32),
                    block_align: 8,
                    average_bytes_per_second: 384_000,
                    channel_mask: None,
                    sub_format: None,
                },
                source_frames_available: 1_440_000,
                frames_trimmed_before: 0,
                frames_trimmed_after: 0,
                leading_silence_frames: 0,
                trailing_silence_frames: 0,
                rendered_frame_count: 1_440_000,
                rendered_duration_seconds: 30.0,
                rendered_wav_size: 11_520_056,
                warnings: Vec::new(),
            },
        }
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
        fs::write(directory.join("SlickClip-20260815-234512.mp4"), b"existing").unwrap();

        let path = choose_output_path(&directory, "20260815-234512").unwrap();
        assert_eq!(
            path.file_name().unwrap(),
            "SlickClip-20260815-234512-001.mp4"
        );
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn repeated_saves_receive_distinct_owned_workspaces() {
        let directory = test_directory("workspaces");
        fs::create_dir_all(&directory).unwrap();
        let first = create_workspace(&directory, "stamp").unwrap();
        let second = create_workspace(&directory, "stamp").unwrap();
        assert_ne!(first.directory, second.directory);
        assert_eq!(first.directory.parent(), Some(first.temp_root.as_path()));
        cleanup_workspace(&first.temp_root, &first.directory).unwrap();
        cleanup_workspace(&second.temp_root, &second.directory).unwrap();
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn workspace_cleanup_refuses_paths_outside_owned_temp_root() {
        let directory = test_directory("cleanup-safety");
        let temp_root = directory.join(".slickclip-temp");
        let outside = directory.join("save-outside");
        fs::create_dir_all(&outside).unwrap();
        assert!(cleanup_workspace(&temp_root, &outside).is_err());
        assert!(outside.exists());
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn permanent_output_reservation_never_creates_partial_files() {
        let directory = test_directory("atomic-output");
        fs::create_dir_all(&directory).unwrap();
        let path = choose_output_path(&directory, "stamp").unwrap();
        assert!(!path.exists());
        assert_eq!(fs::read_dir(&directory).unwrap().count(), 0);
        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn final_verification_accepts_combined_default_and_individual_non_default() {
        let source = probe(vec![probe_stream(0, "video", None, true)]);
        let final_report = probe(vec![
            probe_stream(0, "video", None, true),
            probe_stream(1, "audio", Some("Combined"), true),
            probe_stream(2, "audio", Some("Game"), false),
        ]);
        let verified = verify_final_media(
            &source,
            &final_report,
            &[rendered(AudioTrackRole::Game)],
            30.0,
            52_000_000,
        )
        .unwrap();
        assert!(verified.verified);
        assert_eq!(verified.audio_titles, ["Combined", "Game"]);
        assert_eq!(verified.audio_stream_count, 2);
    }

    #[test]
    fn final_verification_rejects_multiple_default_audio_streams() {
        let source = probe(vec![probe_stream(0, "video", None, true)]);
        let final_report = probe(vec![
            probe_stream(0, "video", None, true),
            probe_stream(1, "audio", Some("Combined"), true),
            probe_stream(2, "audio", Some("Game"), true),
        ]);
        assert!(verify_final_media(
            &source,
            &final_report,
            &[rendered(AudioTrackRole::Game)],
            30.0,
            52_000_000,
        )
        .is_err());
    }

    #[test]
    fn final_verification_preserves_video_only_saves_without_sources() {
        let source = probe(vec![probe_stream(0, "video", None, true)]);
        let final_report = probe(vec![probe_stream(0, "video", None, true)]);
        let verified = verify_final_media(&source, &final_report, &[], 30.0, 52_000_000).unwrap();
        assert_eq!(verified.video_stream_count, 1);
        assert_eq!(verified.audio_stream_count, 0);
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
