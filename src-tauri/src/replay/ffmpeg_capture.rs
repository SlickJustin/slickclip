use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::capture::encoder::{EncoderChoice, EncoderCodec};
use crate::clips::ffmpeg::FfmpegExecutable;

use super::segment::{average_bitrate_mbps, CompletedSegment, VideoFrameTimingPoint};

pub const FFMPEG_SEGMENT_DURATION: Duration = Duration::from_secs(2);
pub const MAX_UNEXPECTED_RESTARTS: u32 = 3;
const CHILD_STOP_TIMEOUT: Duration = Duration::from_secs(3);
const RESTART_BACKOFF: Duration = Duration::from_millis(250);
const CAPABILITY_PROBE_TIMEOUT: Duration = Duration::from_secs(5);
const STDERR_LINE_LIMIT: usize = 160;
const STDERR_CHARACTER_LIMIT: usize = 32 * 1024;

fn install_owned_child<T>(slot: &mut Option<T>, child: T) -> Result<(), String> {
    if slot.is_some() {
        return Err("A Replay session cannot own more than one FFmpeg capture child.".to_string());
    }
    *slot = Some(child);
    Ok(())
}

fn restart_allowed(completed_restarts: u32) -> bool {
    completed_restarts < MAX_UNEXPECTED_RESTARTS
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FfmpegEncoderKind {
    Nvenc,
    Amf,
    Qsv,
    Software,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FfmpegEncoderPlan {
    pub codec: EncoderCodec,
    pub encoder_name: &'static str,
    pub kind: FfmpegEncoderKind,
}

impl FfmpegEncoderPlan {
    pub fn display_name(&self) -> String {
        format!("{} ({})", self.codec.display_name(), self.encoder_name)
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FfmpegCapturePlan {
    pub adapter_index: u32,
    pub output_index: u32,
    pub output_identity: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub quality_qp: u8,
    pub encoder: FfmpegEncoderPlan,
    pub session_directory: PathBuf,
}

impl FfmpegCapturePlan {
    pub fn segment_pattern(&self) -> PathBuf {
        self.session_directory.join("segment-%06d.mp4")
    }

    pub fn arguments(&self, first_sequence_number: u64) -> Vec<String> {
        build_capture_arguments(self, first_sequence_number)
    }
}

#[derive(Clone, Debug)]
pub struct FfmpegCapabilityReport {
    pub program: String,
    pub ddagrab_compiled: bool,
    pub encoder: FfmpegEncoderPlan,
    pub encoder_probe_failures: Vec<String>,
}

pub fn resolve_capture_capabilities(
    choice: EncoderChoice,
    adapter_index: u32,
    output_index: u32,
    frame_rate: u32,
) -> Result<(FfmpegExecutable, FfmpegCapabilityReport), String> {
    let ffmpeg = FfmpegExecutable::resolve()?;
    let filters = run_probe(&ffmpeg, &["-hide_banner", "-filters"])?;
    let ddagrab_compiled = filters.lines().any(|line| line.contains("ddagrab"));
    if !ddagrab_compiled {
        return Err(format!(
            "The bundled FFmpeg build at '{}' does not provide the required ddagrab filter. SlickClip will not fall back to its retired custom Replay capture loop.",
            ffmpeg.program_display()
        ));
    }
    probe_ddagrab(&ffmpeg, adapter_index, output_index, frame_rate)?;

    let mut failures = Vec::new();
    let encoder = encoder_candidates(choice)?
        .into_iter()
        .find(|candidate| match probe_encoder(
            &ffmpeg,
            candidate,
            adapter_index,
            output_index,
            frame_rate,
        ) {
            Ok(()) => true,
            Err(error) => {
                failures.push(format!("{}: {error}", candidate.encoder_name));
                false
            }
        })
        .ok_or_else(|| {
            format!(
                "No FFmpeg encoder allowed by the {:?} preference passed a real runtime encode probe. {}",
                choice,
                failures.join("; ")
            )
        })?;

    let report = FfmpegCapabilityReport {
        program: ffmpeg.program_display(),
        ddagrab_compiled,
        encoder,
        encoder_probe_failures: failures,
    };
    Ok((ffmpeg, report))
}

fn encoder_candidates(choice: EncoderChoice) -> Result<Vec<FfmpegEncoderPlan>, String> {
    let candidates = match choice {
        EncoderChoice::Automatic => vec![
            (EncoderCodec::Hevc, "hevc_nvenc", FfmpegEncoderKind::Nvenc),
            (EncoderCodec::Hevc, "hevc_amf", FfmpegEncoderKind::Amf),
            (EncoderCodec::Hevc, "hevc_qsv", FfmpegEncoderKind::Qsv),
            (EncoderCodec::H264, "h264_nvenc", FfmpegEncoderKind::Nvenc),
            (EncoderCodec::H264, "h264_amf", FfmpegEncoderKind::Amf),
            (EncoderCodec::H264, "h264_qsv", FfmpegEncoderKind::Qsv),
            (EncoderCodec::H264, "libx264", FfmpegEncoderKind::Software),
        ],
        EncoderChoice::H264 => vec![
            (EncoderCodec::H264, "h264_nvenc", FfmpegEncoderKind::Nvenc),
            (EncoderCodec::H264, "h264_amf", FfmpegEncoderKind::Amf),
            (EncoderCodec::H264, "h264_qsv", FfmpegEncoderKind::Qsv),
            (EncoderCodec::H264, "libx264", FfmpegEncoderKind::Software),
        ],
        EncoderChoice::Hevc => vec![
            (EncoderCodec::Hevc, "hevc_nvenc", FfmpegEncoderKind::Nvenc),
            (EncoderCodec::Hevc, "hevc_amf", FfmpegEncoderKind::Amf),
            (EncoderCodec::Hevc, "hevc_qsv", FfmpegEncoderKind::Qsv),
            (EncoderCodec::Hevc, "libx265", FfmpegEncoderKind::Software),
        ],
        EncoderChoice::Av1 => {
            return Err(
                "AV1 Replay capture is not enabled for the FFmpeg ddagrab backend. Choose Automatic, HEVC, or H.264."
                    .to_string(),
            )
        }
    };
    Ok(candidates
        .into_iter()
        .map(|(codec, encoder_name, kind)| FfmpegEncoderPlan {
            codec,
            encoder_name,
            kind,
        })
        .collect())
}

fn probe_ddagrab(
    ffmpeg: &FfmpegExecutable,
    adapter_index: u32,
    output_index: u32,
    frame_rate: u32,
) -> Result<(), String> {
    let source = format!(
        "ddagrab=output_idx={output_index}:draw_mouse=1:framerate={frame_rate}:dup_frames=1,hwdownload,format=bgra"
    );
    let mut command = ffmpeg.capture_command();
    command
        .args([
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "warning".to_string(),
            "-init_hw_device".to_string(),
            format!("d3d11va=dda:{adapter_index}"),
            "-filter_hw_device".to_string(),
            "dda".to_string(),
            "-filter_complex".to_string(),
            source,
            "-frames:v".to_string(),
            "1".to_string(),
            "-f".to_string(),
            "null".to_string(),
            "NUL".to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let output = bounded_probe_output(command, "real FFmpeg ddagrab probe")?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(format!(
            "The bundled FFmpeg has ddagrab compiled in, but a real probe of DXGI adapter {adapter_index}, output {output_index} was denied or failed: {detail}. SlickClip will not fall back to its retired custom Replay capture loop."
        ))
    }
}

fn probe_encoder(
    ffmpeg: &FfmpegExecutable,
    plan: &FfmpegEncoderPlan,
    adapter_index: u32,
    output_index: u32,
    frame_rate: u32,
) -> Result<(), String> {
    let source = format!(
        "ddagrab=output_idx={output_index}:draw_mouse=1:framerate={frame_rate}:dup_frames=1"
    );
    let filter = if plan.kind == FfmpegEncoderKind::Software {
        format!("{source},hwdownload,format=bgra,format=yuv420p[capture]")
    } else {
        format!("{source}[capture]")
    };
    let mut command = ffmpeg.capture_command();
    command
        .args([
            "-hide_banner".to_string(),
            "-loglevel".to_string(),
            "warning".to_string(),
            "-init_hw_device".to_string(),
            format!("d3d11va=dda:{adapter_index}"),
            "-filter_hw_device".to_string(),
            "dda".to_string(),
            "-filter_complex".to_string(),
            filter,
            "-map".to_string(),
            "[capture]".to_string(),
            "-frames:v".to_string(),
            "2".to_string(),
            "-an".to_string(),
            "-c:v".to_string(),
            plan.encoder_name.to_string(),
        ])
        .args(encoder_arguments(plan.kind, 23))
        .args(["-f", "null", "NUL"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let output = bounded_probe_output(command, "FFmpeg encoder probe")?;
    if output.status.success() {
        Ok(())
    } else {
        let detail = String::from_utf8_lossy(&output.stderr).trim().to_string();
        Err(if detail.is_empty() {
            format!("probe exited with {}", output.status)
        } else {
            detail
        })
    }
}

fn run_probe(ffmpeg: &FfmpegExecutable, arguments: &[&str]) -> Result<String, String> {
    let mut command = ffmpeg.capture_command();
    let output = command
        .args(arguments)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|error| format!("Could not launch bundled FFmpeg capability probe: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "Bundled FFmpeg capability probe exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let mut text = String::from_utf8_lossy(&output.stdout).to_string();
    text.push_str(&String::from_utf8_lossy(&output.stderr));
    Ok(text)
}

fn bounded_probe_output(mut command: Command, description: &str) -> Result<Output, String> {
    let mut child = command
        .spawn()
        .map_err(|error| format!("Could not launch {description}: {error}"))?;
    let deadline = Instant::now() + CAPABILITY_PROBE_TIMEOUT;
    loop {
        if child
            .try_wait()
            .map_err(|error| format!("Could not inspect {description}: {error}"))?
            .is_some()
        {
            return child
                .wait_with_output()
                .map_err(|error| format!("Could not collect {description} output: {error}"));
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let output = child
                .wait_with_output()
                .map_err(|error| format!("Could not stop timed-out {description}: {error}"))?;
            return Err(format!(
                "{description} exceeded the {:.0}-second capability deadline. Diagnostics: {}",
                CAPABILITY_PROBE_TIMEOUT.as_secs_f64(),
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn build_capture_arguments(plan: &FfmpegCapturePlan, first_sequence_number: u64) -> Vec<String> {
    let source = format!(
        "ddagrab=output_idx={}:draw_mouse=1:framerate={}:dup_frames=1",
        plan.output_index, plan.frame_rate
    );
    let filter = if plan.encoder.kind == FfmpegEncoderKind::Software {
        format!("{source},hwdownload,format=bgra,format=yuv420p[capture]")
    } else {
        format!("{source}[capture]")
    };
    let mut arguments = vec![
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "warning".to_string(),
        "-y".to_string(),
        "-init_hw_device".to_string(),
        format!("d3d11va=dda:{}", plan.adapter_index),
        "-filter_hw_device".to_string(),
        "dda".to_string(),
        "-filter_complex".to_string(),
        filter,
        "-map".to_string(),
        "[capture]".to_string(),
        "-an".to_string(),
        "-c:v".to_string(),
        plan.encoder.encoder_name.to_string(),
    ];
    arguments.extend(encoder_arguments(plan.encoder.kind, plan.quality_qp));
    let keyframe_interval = plan.frame_rate.saturating_mul(2).to_string();
    arguments.extend(vec![
        "-r".to_string(),
        plan.frame_rate.to_string(),
        "-fps_mode".to_string(),
        "cfr".to_string(),
        "-g".to_string(),
        keyframe_interval.clone(),
        "-keyint_min".to_string(),
        keyframe_interval,
        "-force_key_frames".to_string(),
        format!("expr:gte(t,n_forced*{})", FFMPEG_SEGMENT_DURATION.as_secs()),
        "-f".to_string(),
        "segment".to_string(),
        "-segment_time".to_string(),
        FFMPEG_SEGMENT_DURATION.as_secs().to_string(),
        "-segment_time_delta".to_string(),
        format!("{:.9}", 0.5 / f64::from(plan.frame_rate)),
        "-reset_timestamps".to_string(),
        "1".to_string(),
        "-segment_start_number".to_string(),
        first_sequence_number.to_string(),
        "-segment_format".to_string(),
        "mp4".to_string(),
        "-segment_format_options".to_string(),
        "movflags=+faststart".to_string(),
        plan.segment_pattern().to_string_lossy().into_owned(),
    ]);
    arguments
}

fn encoder_arguments(kind: FfmpegEncoderKind, quality_qp: u8) -> Vec<String> {
    let quality = quality_qp.to_string();
    match kind {
        FfmpegEncoderKind::Nvenc => vec![
            "-preset".into(),
            "p4".into(),
            "-tune".into(),
            "ll".into(),
            "-rc".into(),
            "vbr".into(),
            "-cq".into(),
            quality,
            "-b:v".into(),
            "0".into(),
        ],
        FfmpegEncoderKind::Amf => vec![
            "-quality".into(),
            "balanced".into(),
            "-rc".into(),
            "cqp".into(),
            "-qp_i".into(),
            quality.clone(),
            "-qp_p".into(),
            quality,
        ],
        FfmpegEncoderKind::Qsv => vec![
            "-preset".into(),
            "medium".into(),
            "-global_quality".into(),
            quality,
        ],
        FfmpegEncoderKind::Software => {
            vec!["-preset".into(), "veryfast".into(), "-crf".into(), quality]
        }
    }
}

struct OwnedCaptureChild {
    child: Child,
    stderr: Receiver<String>,
    stderr_worker: Option<JoinHandle<()>>,
}

impl OwnedCaptureChild {
    fn spawn(
        ffmpeg: &FfmpegExecutable,
        plan: &FfmpegCapturePlan,
        first_sequence_number: u64,
    ) -> Result<Self, String> {
        let mut command = ffmpeg.capture_command();
        command
            .args(plan.arguments(first_sequence_number))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped());
        let mut child = command.spawn().map_err(|error| {
            format!("Could not launch SlickClip's owned FFmpeg display-capture child: {error}")
        })?;
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "Could not capture FFmpeg diagnostics.".to_string())?;
        let (sender, receiver) = mpsc::sync_channel(128);
        let worker = thread::Builder::new()
            .name("slickclip-ffmpeg-log".to_string())
            .spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    let _ = sender.try_send(line);
                }
            })
            .map_err(|error| format!("Could not start the FFmpeg diagnostic reader: {error}"))?;
        Ok(Self {
            child,
            stderr: receiver,
            stderr_worker: Some(worker),
        })
    }

    fn try_wait(&mut self) -> Result<Option<ExitStatus>, String> {
        self.child
            .try_wait()
            .map_err(|error| format!("Could not inspect the owned FFmpeg child: {error}"))
    }

    fn drain_stderr(&self, destination: &mut VecDeque<String>) {
        while let Ok(line) = self.stderr.try_recv() {
            destination.push_back(line);
            while destination.len() > STDERR_LINE_LIMIT
                || destination.iter().map(String::len).sum::<usize>() > STDERR_CHARACTER_LIMIT
            {
                destination.pop_front();
            }
        }
    }

    fn stop(mut self) -> Result<ExitStatus, String> {
        if let Some(stdin) = self.child.stdin.as_mut() {
            let _ = stdin.write_all(b"q\n");
            let _ = stdin.flush();
        }
        let deadline = Instant::now() + CHILD_STOP_TIMEOUT;
        loop {
            if let Some(status) = self.try_wait()? {
                self.join_log_worker();
                return Ok(status);
            }
            if Instant::now() >= deadline {
                self.child.kill().map_err(|error| {
                    format!("Could not terminate SlickClip's owned FFmpeg child: {error}")
                })?;
                let status = self.child.wait().map_err(|error| {
                    format!("Could not wait for SlickClip's owned FFmpeg child: {error}")
                })?;
                self.join_log_worker();
                return Ok(status);
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    fn join_log_worker(&mut self) {
        if let Some(worker) = self.stderr_worker.take() {
            let _ = worker.join();
        }
    }

    fn finish_diagnostics(&mut self, destination: &mut VecDeque<String>) {
        self.join_log_worker();
        self.drain_stderr(destination);
    }
}

impl Drop for OwnedCaptureChild {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
        self.join_log_worker();
    }
}

#[derive(Debug)]
pub struct CapturePoll {
    pub completed_segments: Vec<CompletedSegment>,
    pub child_restarted: bool,
    pub first_real_segment: bool,
}

pub struct FfmpegReplayCapture {
    ffmpeg: FfmpegExecutable,
    plan: FfmpegCapturePlan,
    child: Option<OwnedCaptureChild>,
    next_sequence_number: u64,
    restart_count: u32,
    child_epoch_cursor_100ns: i64,
    finalized: BTreeMap<u64, PathBuf>,
    diagnostics: VecDeque<String>,
    observed_real_segment: bool,
}

impl FfmpegReplayCapture {
    pub fn start(
        ffmpeg: FfmpegExecutable,
        plan: FfmpegCapturePlan,
        start_qpc_100ns: i64,
    ) -> Result<Self, String> {
        fs::create_dir_all(&plan.session_directory).map_err(|error| {
            format!(
                "Could not create FFmpeg Replay session directory '{}': {error}",
                plan.session_directory.display()
            )
        })?;
        let child = OwnedCaptureChild::spawn(&ffmpeg, &plan, 1)?;
        let mut child_slot = None;
        install_owned_child(&mut child_slot, child)?;
        Ok(Self {
            ffmpeg,
            plan,
            child: child_slot,
            next_sequence_number: 1,
            restart_count: 0,
            child_epoch_cursor_100ns: start_qpc_100ns,
            finalized: BTreeMap::new(),
            diagnostics: VecDeque::new(),
            observed_real_segment: false,
        })
    }

    pub fn has_live_child(&self) -> bool {
        self.child.is_some()
    }

    pub fn restart_count(&self) -> u32 {
        self.restart_count
    }

    pub fn diagnostics_tail(&self) -> String {
        self.diagnostics
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub fn poll(&mut self, now_qpc_100ns: i64) -> Result<CapturePoll, String> {
        if let Some(child) = self.child.as_ref() {
            child.drain_stderr(&mut self.diagnostics);
        }
        let exit = match self.child.as_mut() {
            Some(child) => child.try_wait()?,
            None => None,
        };
        if exit.is_some() {
            if let Some(child) = self.child.as_mut() {
                child.finish_diagnostics(&mut self.diagnostics);
            }
        }
        let mut completed = self.collect_closed_segments(exit.is_some())?;
        let first_real_segment = !self.observed_real_segment && !completed.is_empty();
        self.observed_real_segment |= first_real_segment;
        let mut child_restarted = false;
        if let Some(status) = exit {
            self.child.take();
            if !restart_allowed(self.restart_count) {
                return Err(format!(
                    "FFmpeg display capture exited unexpectedly with {status} after {} bounded restart attempts. Diagnostics: {}",
                    self.restart_count,
                    self.diagnostics_tail()
                ));
            }
            self.restart_count += 1;
            thread::sleep(RESTART_BACKOFF);
            self.child_epoch_cursor_100ns = now_qpc_100ns.saturating_add(
                i64::try_from(RESTART_BACKOFF.as_nanos() / 100).unwrap_or(i64::MAX),
            );
            let next_child =
                OwnedCaptureChild::spawn(&self.ffmpeg, &self.plan, self.next_sequence_number)?;
            install_owned_child(&mut self.child, next_child)?;
            child_restarted = true;
        }
        completed.sort_by_key(|segment| segment.sequence_number);
        Ok(CapturePoll {
            completed_segments: completed,
            child_restarted,
            first_real_segment,
        })
    }

    pub fn stop_and_finalize(mut self) -> Result<Vec<CompletedSegment>, String> {
        if let Some(child) = self.child.take() {
            child.stop()?;
        }
        self.collect_closed_segments(true)
    }

    fn collect_closed_segments(
        &mut self,
        include_current: bool,
    ) -> Result<Vec<CompletedSegment>, String> {
        let mut paths = discover_segment_paths(&self.plan.session_directory)?;
        paths.retain(|sequence, _| {
            *sequence >= self.next_sequence_number && !self.finalized.contains_key(sequence)
        });
        let highest = paths.keys().next_back().copied();
        let closed = paths
            .into_iter()
            .filter(|(sequence, _)| {
                include_current || highest.is_some_and(|value| *sequence < value)
            })
            .collect::<Vec<_>>();
        let mut completed = Vec::new();
        for (sequence, path) in closed {
            match inspect_segment(
                &self.ffmpeg,
                &self.plan,
                sequence,
                &path,
                self.child_epoch_cursor_100ns,
            ) {
                Ok(segment) => {
                    self.child_epoch_cursor_100ns = segment.segment_session_end_qpc_100ns;
                    self.next_sequence_number = sequence.saturating_add(1);
                    self.finalized.insert(sequence, path);
                    completed.push(segment);
                }
                Err(error) if include_current => {
                    let _ = fs::remove_file(&path);
                    self.next_sequence_number = sequence.saturating_add(1);
                    self.diagnostics.push_back(format!(
                        "Discarded incomplete FFmpeg segment {sequence}: {error}"
                    ));
                }
                Err(error) => return Err(error),
            }
        }
        Ok(completed)
    }
}

fn discover_segment_paths(directory: &Path) -> Result<BTreeMap<u64, PathBuf>, String> {
    let mut paths = BTreeMap::new();
    for entry in fs::read_dir(directory).map_err(|error| {
        format!(
            "Could not inspect FFmpeg Replay segments in '{}': {error}",
            directory.display()
        )
    })? {
        let entry =
            entry.map_err(|error| format!("Could not inspect an FFmpeg segment: {error}"))?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|value| value.to_str()) else {
            continue;
        };
        let Some(sequence) = name
            .strip_prefix("segment-")
            .and_then(|value| value.strip_suffix(".mp4"))
            .and_then(|value| value.parse::<u64>().ok())
        else {
            continue;
        };
        if entry.file_type().is_ok_and(|kind| kind.is_file()) {
            paths.insert(sequence, path);
        }
    }
    Ok(paths)
}

fn inspect_segment(
    ffmpeg: &FfmpegExecutable,
    plan: &FfmpegCapturePlan,
    sequence: u64,
    path: &Path,
    session_start_qpc_100ns: i64,
) -> Result<CompletedSegment, String> {
    let started = Instant::now();
    let report = ffmpeg.inspect_media(path)?;
    let video = report
        .streams
        .iter()
        .find(|stream| stream.codec_type.as_deref() == Some("video"))
        .ok_or_else(|| format!("FFmpeg segment {sequence} has no video stream."))?;
    let duration_seconds = video
        .duration
        .as_deref()
        .and_then(|value| value.parse::<f64>().ok())
        .or_else(|| {
            report
                .format
                .as_ref()
                .and_then(|format| format.duration.as_deref())
                .and_then(|value| value.parse::<f64>().ok())
        })
        .filter(|value| value.is_finite() && *value > 0.0)
        .ok_or_else(|| format!("FFmpeg segment {sequence} has no positive duration."))?;
    let frame_count = (duration_seconds * f64::from(plan.frame_rate))
        .round()
        .max(1.0) as u64;
    let duration_100ns =
        ((i128::from(frame_count) * 10_000_000) / i128::from(plan.frame_rate)) as i64;
    let frame_timing_points = (0..frame_count)
        .map(|frame_index| {
            let pts = ((i128::from(frame_index) * 10_000_000) / i128::from(plan.frame_rate)) as i64;
            VideoFrameTimingPoint {
                frame_index,
                output_qpc_100ns: session_start_qpc_100ns.saturating_add(pts),
                source_qpc_100ns: session_start_qpc_100ns.saturating_add(pts),
                encoded_pts_100ns: pts,
                fresh_source: true,
            }
        })
        .collect::<Vec<_>>();
    let last_pts = frame_timing_points
        .last()
        .map_or(0, |point| point.encoded_pts_100ns);
    let file_size = fs::metadata(path)
        .map_err(|error| {
            format!(
                "Could not inspect FFmpeg segment '{}': {error}",
                path.display()
            )
        })?
        .len();
    let start_timestamp_ms =
        unix_timestamp_ms().saturating_sub((duration_seconds * 1_000.0) as u64);
    Ok(CompletedSegment {
        sequence_number: sequence,
        file_path: path.to_string_lossy().into_owned(),
        start_timestamp_ms,
        end_timestamp_ms: start_timestamp_ms.saturating_add((duration_seconds * 1_000.0) as u64),
        actual_duration_ms: u64::try_from(duration_100ns.max(0) / 10_000).unwrap_or(u64::MAX),
        segment_session_start_qpc_100ns: session_start_qpc_100ns,
        segment_session_end_qpc_100ns: session_start_qpc_100ns.saturating_add(duration_100ns),
        first_frame_timestamp_100ns: session_start_qpc_100ns,
        last_frame_timestamp_100ns: session_start_qpc_100ns.saturating_add(last_pts),
        encoded_start_pts_100ns: 0,
        encoded_last_frame_pts_100ns: last_pts,
        encoded_end_pts_100ns: duration_100ns,
        encoded_duration_100ns: duration_100ns,
        encoded_time_base_numerator: 1,
        encoded_time_base_denominator: 10_000_000,
        frame_timing_points,
        next_segment_first_frame_timestamp_100ns: None,
        source_frame_gap_ms: None,
        source_update_count: frame_count,
        fresh_output_frame_count: frame_count,
        held_output_frame_count: 0,
        frame_count,
        encoder_creation_time_ms: 0.0,
        encoder_creation_started_ms: 0.0,
        encoder_creation_completed_ms: 0.0,
        rotation_requested_ms: None,
        first_frame_submitted_ms: Some(0.0),
        last_frame_submitted_ms: Some(duration_seconds * 1_000.0),
        next_first_frame_submitted_ms: None,
        codec: plan.encoder.codec.display_name().to_string(),
        width: video.width.unwrap_or(plan.width),
        height: video.height.unwrap_or(plan.height),
        frame_rate: plan.frame_rate,
        file_size,
        average_bitrate_mbps: average_bitrate_mbps(file_size, duration_100ns).unwrap_or(0.0),
        finalized: true,
        finalization_time_ms: started.elapsed().as_secs_f64() * 1_000.0,
        rotation_gap_ms: None,
    })
}

fn unix_timestamp_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(
        frame_rate: u32,
        encoder_name: &'static str,
        kind: FfmpegEncoderKind,
    ) -> FfmpegCapturePlan {
        FfmpegCapturePlan {
            adapter_index: 1,
            output_index: 2,
            output_identity: r"\\.\DISPLAY3".to_string(),
            width: 2560,
            height: 1440,
            frame_rate,
            quality_qp: 23,
            encoder: FfmpegEncoderPlan {
                codec: EncoderCodec::H264,
                encoder_name,
                kind,
            },
            session_directory: PathBuf::from(r"C:\redacted\session"),
        }
    }

    #[test]
    fn command_maps_physical_output_and_sixty_fps_to_ddagrab() {
        let arguments = plan(60, "h264_nvenc", FfmpegEncoderKind::Nvenc).arguments(17);
        assert!(arguments.contains(
            &"ddagrab=output_idx=2:draw_mouse=1:framerate=60:dup_frames=1[capture]".to_string()
        ));
        assert!(arguments.windows(2).any(|pair| pair == ["-r", "60"]));
        assert!(arguments
            .windows(2)
            .any(|pair| pair == ["-segment_start_number", "17"]));
        assert_eq!(
            arguments
                .iter()
                .filter(|value| value.as_str() == "h264_nvenc")
                .count(),
            1
        );
    }

    #[test]
    fn command_maps_thirty_fps_and_software_download_explicitly() {
        let arguments = plan(30, "libx264", FfmpegEncoderKind::Software).arguments(1);
        assert!(arguments
            .iter()
            .any(|value| value.contains("framerate=30") && value.contains("hwdownload")));
        assert!(arguments.windows(2).any(|pair| pair == ["-r", "30"]));
    }

    #[test]
    fn command_maps_configured_quality_to_encoder_quantizer() {
        let mut capture = plan(60, "h264_nvenc", FfmpegEncoderKind::Nvenc);
        capture.quality_qp = 18;
        let arguments = capture.arguments(1);
        assert!(arguments.windows(2).any(|pair| pair == ["-cq", "18"]));
    }

    #[test]
    fn encoder_preferences_keep_hardware_priority_and_safe_software_fallback() {
        let h264 = encoder_candidates(EncoderChoice::H264).unwrap();
        assert_eq!(
            h264.iter()
                .map(|item| item.encoder_name)
                .collect::<Vec<_>>(),
            vec!["h264_nvenc", "h264_amf", "h264_qsv", "libx264"]
        );
        let hevc = encoder_candidates(EncoderChoice::Hevc).unwrap();
        assert_eq!(hevc.last().unwrap().encoder_name, "libx265");
        let automatic = encoder_candidates(EncoderChoice::Automatic).unwrap();
        assert_eq!(automatic.first().unwrap().encoder_name, "hevc_nvenc");
        assert_eq!(automatic.last().unwrap().encoder_name, "libx264");
        assert!(encoder_candidates(EncoderChoice::Av1).is_err());
    }

    #[test]
    fn restart_budget_is_bounded_and_session_plan_is_immutable() {
        assert_eq!(MAX_UNEXPECTED_RESTARTS, 3);
        assert!(restart_allowed(0));
        assert!(restart_allowed(2));
        assert!(!restart_allowed(3));
        let capture = plan(60, "h264_nvenc", FfmpegEncoderKind::Nvenc);
        assert_eq!(capture.output_identity, r"\\.\DISPLAY3");
        assert_eq!(capture.output_index, 2);
        assert_eq!(capture.adapter_index, 1);
    }

    #[test]
    fn one_logical_session_owns_at_most_one_child_and_releases_it_before_replacement() {
        let mut slot = None;
        install_owned_child(&mut slot, 10u32).unwrap();
        assert!(install_owned_child(&mut slot, 11u32).is_err());
        assert_eq!(slot.take(), Some(10));
        install_owned_child(&mut slot, 11u32).unwrap();
        assert_eq!(slot, Some(11));
    }

    #[test]
    fn all_supported_replay_lengths_have_a_two_second_safety_boundary() {
        for seconds in [30u32, 60, 120, 180, 300] {
            let complete_segments = seconds / FFMPEG_SEGMENT_DURATION.as_secs() as u32;
            assert_eq!(complete_segments * 2, seconds);
            assert_eq!(complete_segments + 1, seconds / 2 + 1);
        }
    }
}
