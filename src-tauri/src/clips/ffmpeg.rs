use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::Deserialize;

use crate::replay::AudioTrackRole;

use super::audio_render::RenderedAudioTrack;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const BELOW_NORMAL_PRIORITY_CLASS: u32 = 0x0000_4000;

pub struct FfmpegExecutable {
    program: OsString,
}

#[derive(Clone, Debug)]
pub struct AudioMuxCommandPlan {
    pub arguments: Vec<OsString>,
    pub filter_graph: String,
    pub audio_titles: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MediaProbeReport {
    #[serde(default)]
    pub streams: Vec<MediaProbeStream>,
    pub format: Option<MediaProbeFormat>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MediaProbeStream {
    pub index: u32,
    pub codec_name: Option<String>,
    pub profile: Option<String>,
    pub codec_type: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub r_frame_rate: Option<String>,
    pub avg_frame_rate: Option<String>,
    pub sample_rate: Option<String>,
    pub channels: Option<u16>,
    pub duration: Option<String>,
    pub bit_rate: Option<String>,
    #[serde(default)]
    pub tags: MediaProbeTags,
    #[serde(default)]
    pub disposition: MediaProbeDisposition,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MediaProbeTags {
    pub title: Option<String>,
    pub handler_name: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub struct MediaProbeDisposition {
    #[serde(default, rename = "default")]
    pub is_default: u8,
}

#[derive(Clone, Debug, Deserialize)]
pub struct MediaProbeFormat {
    pub duration: Option<String>,
    pub bit_rate: Option<String>,
}

impl FfmpegExecutable {
    pub fn resolve() -> Result<Self, String> {
        if let Some(configured) = std::env::var_os("JUSTIN_REPLAY_FFMPEG") {
            if configured.is_empty() {
                return Err(
                    "JUSTIN_REPLAY_FFMPEG is set but does not contain an executable path."
                        .to_string(),
                );
            }
            let path = PathBuf::from(&configured);
            if !path.is_file() {
                return Err(format!(
                    "JUSTIN_REPLAY_FFMPEG points to '{}', but that file does not exist.",
                    path.display()
                ));
            }
            probe(&configured).map_err(|error| {
                format!(
                    "The FFmpeg executable configured by JUSTIN_REPLAY_FFMPEG could not be used: {error}"
                )
            })?;
            return Ok(Self {
                program: configured,
            });
        }

        let mut errors = Vec::new();
        for candidate in ["ffmpeg.exe", "ffmpeg"] {
            match probe(OsStr::new(candidate)) {
                Ok(()) => {
                    return Ok(Self {
                        program: OsString::from(candidate),
                    });
                }
                Err(error) => errors.push(format!("{candidate}: {error}")),
            }
        }

        Err(format!(
            "FFmpeg is unavailable for this development build. Set JUSTIN_REPLAY_FFMPEG to ffmpeg.exe or add FFmpeg to PATH. Attempts: {}",
            errors.join("; ")
        ))
    }

    pub fn concat_stream_copy(
        &self,
        manifest_path: &Path,
        partial_output_path: &Path,
    ) -> Result<Output, String> {
        let mut command = Command::new(&self.program);
        command
            .arg("-hide_banner")
            .arg("-loglevel")
            .arg("error")
            .arg("-nostdin")
            .arg("-f")
            .arg("concat")
            .arg("-safe")
            .arg("0")
            .arg("-i")
            .arg(manifest_path)
            .arg("-map")
            .arg("0:v:0")
            .arg("-c")
            .arg("copy")
            .arg("-an")
            .arg("-n")
            .arg(partial_output_path)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        hide_console_window(&mut command);

        command
            .output()
            .map_err(|error| format!("Could not launch FFmpeg: {error}"))
    }

    pub fn mux_audio(&self, plan: &AudioMuxCommandPlan) -> Result<Output, String> {
        let mut command = Command::new(&self.program);
        command
            .args(&plan.arguments)
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        hide_console_window(&mut command);
        command
            .output()
            .map_err(|error| format!("Could not launch FFmpeg audio mux: {error}"))
    }

    pub(crate) fn run_cache_arguments(
        &self,
        arguments: &[OsString],
        description: &str,
    ) -> Result<(), String> {
        let mut command = Command::new(&self.program);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        hide_console_window(&mut command);
        lower_cache_priority(&mut command);
        let output = command
            .output()
            .map_err(|error| format!("Could not launch FFmpeg to {description}: {error}"))?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if stderr.is_empty() {
            format!(
                "FFmpeg could not {description}; it exited with {}.",
                output.status
            )
        } else {
            format!("FFmpeg could not {description}: {stderr}")
        })
    }

    pub fn inspect_media(&self, output_path: &Path) -> Result<MediaProbeReport, String> {
        let ffprobe = self.resolve_ffprobe().ok_or_else(|| {
            "ffprobe is required to verify the Stage 11 video and audio streams.".to_string()
        })?;
        let mut command = Command::new(ffprobe);
        command
            .arg("-v")
            .arg("error")
            .arg("-show_streams")
            .arg("-show_format")
            .arg("-show_entries")
            .arg("stream=index,codec_name,codec_type,profile,width,height,r_frame_rate,avg_frame_rate,sample_rate,channels,duration,bit_rate:stream_tags=title,handler_name:stream_disposition=default:format=duration,bit_rate")
            .arg("-of")
            .arg("json")
            .arg(output_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console_window(&mut command);
        let output = command
            .output()
            .map_err(|error| format!("Could not launch ffprobe stream verification: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                format!("ffprobe stream verification exited with {}.", output.status)
            } else {
                format!("ffprobe could not inspect the final replay: {stderr}")
            });
        }
        serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("Could not parse ffprobe stream verification: {error}"))
    }

    pub fn validate_packet_timeline_if_available(
        &self,
        output_path: &Path,
    ) -> Result<Option<f64>, String> {
        let Some(ffprobe) = self.resolve_ffprobe() else {
            return Ok(None);
        };
        let mut command = Command::new(ffprobe);
        command
            .arg("-v")
            .arg("error")
            .arg("-select_streams")
            .arg("v:0")
            .arg("-show_streams")
            .arg("-show_packets")
            .arg("-show_format")
            .arg("-show_entries")
            .arg("stream=duration:packet=pts_time,dts_time,duration_time:format=duration")
            .arg("-of")
            .arg("json")
            .arg(output_path)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        hide_console_window(&mut command);

        let output = command
            .output()
            .map_err(|error| format!("Could not launch ffprobe timeline validation: {error}"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(if stderr.is_empty() {
                format!("ffprobe timeline validation exited with {}.", output.status)
            } else {
                format!("ffprobe could not validate the assembled replay: {stderr}")
            });
        }

        let report: ProbeReport = serde_json::from_slice(&output.stdout)
            .map_err(|error| format!("Could not parse ffprobe timeline validation: {error}"))?;
        validate_probe_report(&report).map(Some)
    }

    fn resolve_ffprobe(&self) -> Option<OsString> {
        if let Some(configured) = std::env::var_os("JUSTIN_REPLAY_FFPROBE") {
            return probe_ffprobe(&configured).is_ok().then_some(configured);
        }

        let ffmpeg_path = PathBuf::from(&self.program);
        if ffmpeg_path
            .parent()
            .is_some_and(|parent| !parent.as_os_str().is_empty())
        {
            let sibling = ffmpeg_path.with_file_name(if cfg!(windows) {
                "ffprobe.exe"
            } else {
                "ffprobe"
            });
            if sibling.is_file() && probe_ffprobe(sibling.as_os_str()).is_ok() {
                return Some(sibling.into_os_string());
            }
        }

        for candidate in ["ffprobe.exe", "ffprobe"] {
            let candidate = OsString::from(candidate);
            if probe_ffprobe(&candidate).is_ok() {
                return Some(candidate);
            }
        }
        None
    }
}

pub fn build_audio_mux_plan(
    video_path: &Path,
    audio_tracks: &[RenderedAudioTrack],
    duration_100ns: i64,
    output_path: &Path,
) -> Result<AudioMuxCommandPlan, String> {
    if audio_tracks.is_empty() {
        return Err("An audio mux plan requires at least one rendered source.".to_string());
    }
    if duration_100ns <= 0 {
        return Err("An audio mux plan requires a positive video duration.".to_string());
    }
    if audio_tracks
        .windows(2)
        .any(|pair| pair[1].track_role <= pair[0].track_role)
    {
        return Err("Rendered audio tracks must be in deterministic role order.".to_string());
    }

    let duration = format!("{:.7}", duration_100ns as f64 / 10_000_000.0);
    let mut filter_parts = Vec::new();
    for (index, _) in audio_tracks.iter().enumerate() {
        filter_parts.push(format!(
            "[{}:a:0]aresample=48000,aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo,asetpts=PTS-STARTPTS,apad,atrim=duration={duration},asplit=2[mix{index}][individual{index}]",
            index + 1
        ));
    }
    if audio_tracks.len() == 1 {
        filter_parts.push("[mix0]anull[combined]".to_string());
    } else {
        let inputs = (0..audio_tracks.len())
            .map(|index| format!("[mix{index}]"))
            .collect::<String>();
        filter_parts.push(format!(
            "{inputs}amix=inputs={}:duration=longest:dropout_transition=0:normalize=1,atrim=duration={duration},asetpts=PTS-STARTPTS[combined]",
            audio_tracks.len()
        ));
    }
    let filter_graph = filter_parts.join(";");

    let mut arguments = vec![
        OsString::from("-hide_banner"),
        OsString::from("-loglevel"),
        OsString::from("error"),
        OsString::from("-nostdin"),
        OsString::from("-i"),
        video_path.as_os_str().to_os_string(),
    ];
    for track in audio_tracks {
        arguments.push(OsString::from("-i"));
        arguments.push(track.path.as_os_str().to_os_string());
    }
    arguments.extend([
        OsString::from("-filter_complex"),
        OsString::from(&filter_graph),
        OsString::from("-map"),
        OsString::from("0:v:0"),
        OsString::from("-map"),
        OsString::from("[combined]"),
    ]);
    for index in 0..audio_tracks.len() {
        arguments.push(OsString::from("-map"));
        arguments.push(OsString::from(format!("[individual{index}]")));
    }
    arguments.extend([
        OsString::from("-c:v"),
        OsString::from("copy"),
        OsString::from("-c:a"),
        OsString::from("aac"),
        OsString::from("-profile:a"),
        OsString::from("aac_low"),
        OsString::from("-b:a"),
        OsString::from("192k"),
        OsString::from("-ar:a"),
        OsString::from("48000"),
        OsString::from("-ac:a"),
        OsString::from("2"),
    ]);

    let mut audio_titles = vec!["Combined".to_string()];
    audio_titles.extend(
        audio_tracks
            .iter()
            .map(|track| track_title(track.track_role).to_string()),
    );
    for (index, title) in audio_titles.iter().enumerate() {
        arguments.push(OsString::from(format!("-metadata:s:a:{index}")));
        arguments.push(OsString::from(format!("title={title}")));
        arguments.push(OsString::from(format!("-metadata:s:a:{index}")));
        arguments.push(OsString::from(format!("handler_name={title}")));
        arguments.push(OsString::from(format!("-disposition:a:{index}")));
        arguments.push(OsString::from(if index == 0 { "default" } else { "0" }));
    }
    arguments.extend([
        OsString::from("-movflags"),
        OsString::from("+faststart"),
        OsString::from("-n"),
        output_path.as_os_str().to_os_string(),
    ]);

    Ok(AudioMuxCommandPlan {
        arguments,
        filter_graph,
        audio_titles,
    })
}

pub const fn track_title(role: AudioTrackRole) -> &'static str {
    match role {
        AudioTrackRole::Game => "Game",
        AudioTrackRole::VoiceChat => "Voice Chat",
        AudioTrackRole::Microphone => "Microphone",
        AudioTrackRole::Other => "Other",
    }
}

#[derive(Deserialize)]
struct ProbeReport {
    #[serde(default)]
    streams: Vec<ProbeStream>,
    #[serde(default)]
    packets: Vec<ProbePacket>,
    format: Option<ProbeFormat>,
}

#[derive(Deserialize)]
struct ProbeStream {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct ProbeFormat {
    duration: Option<String>,
}

#[derive(Deserialize)]
struct ProbePacket {
    pts_time: Option<String>,
    dts_time: Option<String>,
    duration_time: Option<String>,
}

fn validate_probe_report(report: &ProbeReport) -> Result<f64, String> {
    let Some(stream) = report.streams.first() else {
        return Err("The assembled replay has no video stream.".to_string());
    };
    let duration = stream
        .duration
        .as_deref()
        .or_else(|| report.format.as_ref()?.duration.as_deref())
        .and_then(parse_probe_number)
        .filter(|duration| *duration > 0.0)
        .ok_or_else(|| "The assembled replay has an invalid video duration.".to_string())?;
    if !duration.is_finite() {
        return Err("The assembled replay has a non-finite video duration.".to_string());
    }
    if report.packets.is_empty() {
        return Err("The assembled replay has a video stream but no video packets.".to_string());
    }

    let mut previous_dts = None;
    let mut previous_pts = None;
    let mut previous_duration = None;
    for (index, packet) in report.packets.iter().enumerate() {
        let pts = packet.pts_time.as_deref().and_then(parse_probe_number);
        let dts = packet.dts_time.as_deref().and_then(parse_probe_number);
        if pts.is_none() && dts.is_none() {
            return Err(format!("Video packet {index} has neither a PTS nor a DTS."));
        }
        if let (Some(previous), Some(current)) = (previous_dts, dts) {
            if current <= previous {
                return Err(format!(
                    "Video packet {index} has non-monotonic DTS ({current:.6} after {previous:.6})."
                ));
            }
            let delta = current - previous;
            if let Some(expected) = previous_duration {
                if delta > 0.250 && delta > expected * 5.0 {
                    return Err(format!(
                        "Video packet {index} follows an unexpected {delta:.6}-second timestamp gap."
                    ));
                }
            }
        }
        if let (Some(previous), Some(current)) = (previous_pts, pts) {
            if current == previous {
                return Err(format!("Video packet {index} duplicates PTS {current:.6}."));
            }
        }

        previous_dts = dts.or(previous_dts);
        previous_pts = pts.or(previous_pts);
        previous_duration = match packet.duration_time.as_deref().and_then(parse_probe_number) {
            Some(packet_duration) if packet_duration > 0.0 => Some(packet_duration),
            Some(_) => return Err(format!("Video packet {index} has a non-positive duration.")),
            None => previous_duration,
        };
    }

    Ok(duration)
}

fn parse_probe_number(value: &str) -> Option<f64> {
    value.parse::<f64>().ok().filter(|value| value.is_finite())
}

fn probe(program: &OsStr) -> Result<(), String> {
    let mut command = Command::new(program);
    command
        .arg("-version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    hide_console_window(&mut command);

    let status = command.status().map_err(|error| error.to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("the version probe exited with {status}"))
    }
}

fn probe_ffprobe(program: &OsStr) -> Result<(), String> {
    probe(program)
}

fn hide_console_window(command: &mut Command) {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW);
}

fn lower_cache_priority(command: &mut Command) {
    #[cfg(windows)]
    command.creation_flags(CREATE_NO_WINDOW | BELOW_NORMAL_PRIORITY_CLASS);
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use crate::audio::AudioFormatMetadata;
    use crate::replay::AudioTrackRole;

    use super::super::audio_render::{AudioRenderDiagnostics, RenderedAudioTrack};
    use super::{
        build_audio_mux_plan, validate_probe_report, ProbeFormat, ProbePacket, ProbeReport,
        ProbeStream,
    };

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
                source_frames_available: 48_000,
                frames_trimmed_before: 0,
                frames_trimmed_after: 0,
                leading_silence_frames: 0,
                trailing_silence_frames: 0,
                rendered_frame_count: 48_000,
                rendered_duration_seconds: 1.0,
                rendered_wav_size: 384_056,
                warnings: Vec::new(),
            },
        }
    }

    fn arguments(plan: &super::AudioMuxCommandPlan) -> Vec<String> {
        plan.arguments
            .iter()
            .map(|value| value.to_string_lossy().into_owned())
            .collect()
    }

    fn packet(pts: &str, dts: &str, duration: &str) -> ProbePacket {
        ProbePacket {
            pts_time: Some(pts.to_string()),
            dts_time: Some(dts.to_string()),
            duration_time: Some(duration.to_string()),
        }
    }

    fn report(packets: Vec<ProbePacket>) -> ProbeReport {
        ProbeReport {
            streams: vec![ProbeStream {
                duration: Some("1.000000".to_string()),
            }],
            packets,
            format: Some(ProbeFormat {
                duration: Some("1.000000".to_string()),
            }),
        }
    }

    #[test]
    fn accepts_a_continuous_packet_timeline() {
        let report = report(vec![
            packet("0.000000", "0.000000", "0.016667"),
            packet("0.016667", "0.016667", "0.016667"),
            packet("0.033334", "0.033334", "0.016667"),
        ]);
        assert!(validate_probe_report(&report).is_ok());
    }

    #[test]
    fn rejects_backward_dts() {
        let report = report(vec![
            packet("0.016667", "0.016667", "0.016667"),
            packet("0.000000", "0.000000", "0.016667"),
        ]);
        assert!(validate_probe_report(&report).is_err());
    }

    #[test]
    fn rejects_large_timestamp_gaps() {
        let report = report(vec![
            packet("0.000000", "0.000000", "0.016667"),
            packet("0.500000", "0.500000", "0.016667"),
        ]);
        assert!(validate_probe_report(&report).is_err());
    }

    #[test]
    fn rejects_a_missing_video_stream() {
        let report = ProbeReport {
            streams: Vec::new(),
            packets: Vec::new(),
            format: None,
        };
        assert!(validate_probe_report(&report).is_err());
    }

    #[test]
    fn one_source_plan_stream_copies_video_and_maps_combined_then_individual() {
        let plan = build_audio_mux_plan(
            &PathBuf::from("video.mp4"),
            &[rendered(AudioTrackRole::Microphone)],
            10_000_000,
            &PathBuf::from("partial.mp4"),
        )
        .unwrap();
        let args = arguments(&plan);
        assert!(args.windows(2).any(|pair| pair == ["-c:v", "copy"]));
        assert!(args.windows(2).any(|pair| pair == ["-c:a", "aac"]));
        assert!(args.windows(2).any(|pair| pair == ["-b:a", "192k"]));
        let maps = args
            .windows(2)
            .filter(|pair| pair[0] == "-map")
            .map(|pair| pair[1].clone())
            .collect::<Vec<_>>();
        assert_eq!(maps, ["0:v:0", "[combined]", "[individual0]"]);
        assert_eq!(plan.audio_titles, ["Combined", "Microphone"]);
        assert!(plan.filter_graph.contains("asplit=2[mix0][individual0]"));
        assert!(plan.filter_graph.contains("[mix0]anull[combined]"));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-disposition:a:0", "default"]));
        assert!(args
            .windows(2)
            .any(|pair| pair == ["-disposition:a:1", "0"]));
    }

    #[test]
    fn two_three_and_four_source_filtergraphs_have_deterministic_mix_counts() {
        let roles = [
            AudioTrackRole::Game,
            AudioTrackRole::VoiceChat,
            AudioTrackRole::Microphone,
            AudioTrackRole::Other,
        ];
        for count in 2..=4 {
            let tracks = roles[..count]
                .iter()
                .copied()
                .map(rendered)
                .collect::<Vec<_>>();
            let plan = build_audio_mux_plan(
                &PathBuf::from("video.mp4"),
                &tracks,
                300_000_000,
                &PathBuf::from("partial.mp4"),
            )
            .unwrap();
            assert!(plan
                .filter_graph
                .contains(&format!("amix=inputs={count}:duration=longest")));
            assert_eq!(plan.audio_titles.len(), count + 1);
            assert_eq!(plan.audio_titles[0], "Combined");
        }
    }

    #[test]
    fn disabled_tracks_are_omitted_by_the_rendered_input_list() {
        let tracks = [
            rendered(AudioTrackRole::Game),
            rendered(AudioTrackRole::Microphone),
        ];
        let plan = build_audio_mux_plan(
            &PathBuf::from("video.mp4"),
            &tracks,
            300_000_000,
            &PathBuf::from("partial.mp4"),
        )
        .unwrap();
        assert_eq!(plan.audio_titles, ["Combined", "Game", "Microphone"]);
        assert!(!plan.audio_titles.iter().any(|title| title == "Voice Chat"));
    }

    #[test]
    fn empty_and_out_of_order_audio_inputs_are_rejected() {
        assert!(build_audio_mux_plan(
            &PathBuf::from("video.mp4"),
            &[],
            10_000_000,
            &PathBuf::from("partial.mp4"),
        )
        .is_err());
        assert!(build_audio_mux_plan(
            &PathBuf::from("video.mp4"),
            &[
                rendered(AudioTrackRole::Microphone),
                rendered(AudioTrackRole::Game),
            ],
            10_000_000,
            &PathBuf::from("partial.mp4"),
        )
        .is_err());
    }
}
