use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};

use serde::Deserialize;

#[cfg(windows)]
use std::os::windows::process::CommandExt;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

pub struct FfmpegExecutable {
    program: OsString,
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

    pub fn validate_packet_timeline_if_available(&self, output_path: &Path) -> Result<(), String> {
        let Some(ffprobe) = self.resolve_ffprobe() else {
            return Ok(());
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
        validate_probe_report(&report)
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

fn validate_probe_report(report: &ProbeReport) -> Result<(), String> {
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

    Ok(())
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

#[cfg(test)]
mod tests {
    use super::{validate_probe_report, ProbeFormat, ProbePacket, ProbeReport, ProbeStream};

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
}
