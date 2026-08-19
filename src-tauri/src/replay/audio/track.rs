use std::fs::OpenOptions;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY, AUDCLNT_BUFFERFLAGS_SILENT,
    AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
};
use windows::Win32::System::Performance::QueryPerformanceCounter;
use windows::Win32::System::Threading::{
    CreateEventW, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE,
};

use crate::audio::{
    activate_microphone, initialize_capture_client, initialize_process_loopback_client,
    resolve_process_metadata, AudioError, AudioErrorCode, CaptureWaveFormat, ComApartment,
    InitializedAudioClient, OwnedHandle, WavWriter,
};

use super::buffer::{TrackShared, AUDIO_PACKET_QUEUE_CAPACITY};
use super::clock::ReplaySessionClock;
use super::segment::CompletedAudioSegment;
use super::{AudioSourceKind, AudioTrackState};

const EVENT_POLL_MS: u32 = 100;
const AUDIO_SEGMENT_SECONDS: u64 = 2;

enum GateState {
    Waiting,
    Start,
    Cancel,
}
pub struct StartGate(Mutex<GateState>, Condvar);

impl StartGate {
    fn new() -> Self {
        Self(Mutex::new(GateState::Waiting), Condvar::new())
    }
    fn open(&self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = GateState::Start;
        self.1.notify_all();
    }
    fn cancel(&self) {
        *self.0.lock().unwrap_or_else(|p| p.into_inner()) = GateState::Cancel;
        self.1.notify_all();
    }
    fn wait(&self) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|p| p.into_inner());
        while matches!(*state, GateState::Waiting) {
            state = self.1.wait(state).unwrap_or_else(|p| p.into_inner());
        }
        matches!(*state, GateState::Start)
    }
}

enum StartupEvent {
    Prepared,
    Started,
    Failed(String),
}

pub struct AudioReplaySession {
    gate: Arc<StartGate>,
    stop: Arc<AtomicBool>,
    startup: Receiver<(super::AudioTrackRole, StartupEvent)>,
    workers: Vec<JoinHandle<()>>,
    track_count: usize,
}

impl AudioReplaySession {
    pub fn prepare(
        shared_tracks: Vec<Arc<TrackShared>>,
        clock: ReplaySessionClock,
    ) -> Result<Self, String> {
        let gate = Arc::new(StartGate::new());
        let stop = Arc::new(AtomicBool::new(false));
        let (startup_sender, startup) = mpsc::channel();
        let track_count = shared_tracks.len();
        let mut workers = Vec::new();
        for track in shared_tracks {
            let worker_track = Arc::clone(&track);
            let worker_gate = Arc::clone(&gate);
            let worker_stop = Arc::clone(&stop);
            let worker_clock = clock.clone();
            let worker_startup = startup_sender.clone();
            let name = format!("replay-audio-{}", track.configuration.role.directory_name());
            match thread::Builder::new().name(name).spawn(move || {
                let role = worker_track.configuration.role;
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_track(
                        worker_track.clone(),
                        worker_clock,
                        worker_gate,
                        worker_stop,
                        worker_startup.clone(),
                    )
                }));
                match result {
                    Ok(Ok(())) => {}
                    Ok(Err(error)) => {
                        worker_track.set_terminal(AudioTrackState::Error, Some(error.clone()));
                        let _ = worker_startup.send((role, StartupEvent::Failed(error)));
                    }
                    Err(_) => {
                        let message = format!("{:?} audio worker panicked.", role);
                        worker_track.set_terminal(AudioTrackState::Error, Some(message.clone()));
                        let _ = worker_startup.send((role, StartupEvent::Failed(message)));
                    }
                }
            }) {
                Ok(worker) => workers.push(worker),
                Err(error) => {
                    gate.cancel();
                    stop.store(true, Ordering::Release);
                    for worker in workers {
                        let _ = worker.join();
                    }
                    return Err(format!(
                        "Could not create {:?} audio worker: {error}",
                        track.configuration.role
                    ));
                }
            }
        }
        drop(startup_sender);
        let mut session = Self {
            gate,
            stop,
            startup,
            workers,
            track_count,
        };
        if let Err(error) = session.wait_for_phase(false) {
            session.stop_and_wait();
            return Err(error);
        }
        Ok(session)
    }

    pub fn start(&mut self) -> Result<(), String> {
        self.gate.open();
        if let Err(error) = self.wait_for_phase(true) {
            self.stop_and_wait();
            return Err(error);
        }
        Ok(())
    }

    fn wait_for_phase(&self, started: bool) -> Result<(), String> {
        for _ in 0..self.track_count {
            match self.startup.recv_timeout(Duration::from_secs(15)) {
                Ok((_role, StartupEvent::Prepared)) if !started => {}
                Ok((_role, StartupEvent::Started)) if started => {}
                Ok((role, StartupEvent::Failed(error))) => {
                    return Err(format!(
                        "Required {:?} audio source failed to initialize: {error}",
                        role
                    ))
                }
                Ok((role, _)) => {
                    return Err(format!(
                        "Required {:?} audio source reported an invalid startup phase.",
                        role
                    ))
                }
                Err(error) => {
                    return Err(format!(
                        "Timed out coordinating Replay audio startup: {error}"
                    ))
                }
            }
        }
        Ok(())
    }

    pub fn stop_and_wait(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.gate.cancel();
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

impl Drop for AudioReplaySession {
    fn drop(&mut self) {
        self.stop_and_wait();
    }
}

struct PcmPacket {
    bytes: Vec<u8>,
    frames: u32,
    qpc_position_100ns: Option<i64>,
    device_position: Option<u64>,
    silent: bool,
    discontinuity: bool,
    timestamp_error: bool,
}

fn run_track(
    track: Arc<TrackShared>,
    clock: ReplaySessionClock,
    gate: Arc<StartGate>,
    stop: Arc<AtomicBool>,
    startup: mpsc::Sender<(super::AudioTrackRole, StartupEvent)>,
) -> Result<(), String> {
    let _com = ComApartment::initialize_mta("Replay audio track").map_err(audio_message)?;
    let config = track.configuration.clone();
    let (initialized, label, process_handle) = match config.source_kind {
        AudioSourceKind::Microphone => {
            let endpoint = config
                .endpoint_id
                .as_deref()
                .ok_or_else(|| "Microphone endpoint ID is missing.".to_string())?;
            let client = activate_microphone(endpoint).map_err(audio_message)?;
            let format = CaptureWaveFormat::endpoint_mix_format(&client).map_err(audio_message)?;
            initialize_capture_client(&client, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, &format)
                .map_err(|error| {
                    format!("Could not initialize microphone WASAPI client: {error}")
                })?;
            (
                InitializedAudioClient {
                    audio_client: client,
                    format,
                    format_diagnostics: None,
                },
                config
                    .source_label
                    .clone()
                    .unwrap_or_else(|| "Microphone".into()),
                None,
            )
        }
        AudioSourceKind::Process => {
            let pid = config
                .process_id
                .ok_or_else(|| "Application PID is missing.".to_string())?;
            let metadata = resolve_process_metadata(pid)
                .ok_or_else(|| format!("Selected application PID {pid} is no longer available."))?;
            let handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    false,
                    pid,
                )
            }
            .map(OwnedHandle)
            .map_err(|error| format!("Could not monitor application PID {pid}: {error}"))?;
            (
                initialize_process_loopback_client(pid).map_err(audio_message)?,
                config.source_label.clone().unwrap_or(metadata.process_name),
                Some(handle),
            )
        }
    };
    let InitializedAudioClient {
        audio_client,
        format,
        ..
    } = initialized;
    let event = unsafe { CreateEventW(None, false, false, None) }
        .map(OwnedHandle)
        .map_err(|error| format!("Could not create WASAPI event: {error}"))?;
    unsafe { audio_client.SetEventHandle(event.0) }
        .map_err(|error| format!("Could not configure WASAPI event: {error}"))?;
    let capture_client: IAudioCaptureClient = unsafe { audio_client.GetService() }
        .map_err(|error| format!("Could not obtain IAudioCaptureClient: {error}"))?;
    let (packet_sender, packet_receiver) = mpsc::sync_channel(AUDIO_PACKET_QUEUE_CAPACITY);
    let queue_depth = Arc::new(AtomicUsize::new(0));
    let writer_track = Arc::clone(&track);
    let writer_format = format.clone();
    let writer_clock = clock.clone();
    let writer_depth = Arc::clone(&queue_depth);
    let writer = thread::Builder::new()
        .name(format!("replay-audio-wav-{}", config.role.directory_name()))
        .spawn(move || {
            writer_loop(
                writer_track,
                writer_format,
                writer_clock,
                packet_receiver,
                writer_depth,
            )
        })
        .map_err(|error| format!("Could not start audio writer: {error}"))?;
    track.set_prepared(label, format.metadata.clone());
    startup
        .send((config.role, StartupEvent::Prepared))
        .map_err(|_| "Replay audio startup coordinator stopped.".to_string())?;
    if !gate.wait() {
        drop(packet_sender);
        let _ = writer.join();
        return Ok(());
    }
    unsafe { audio_client.Start() }
        .map_err(|error| format!("Could not start WASAPI client: {error}"))?;
    let mut start_qpc = 0i64;
    unsafe { QueryPerformanceCounter(&mut start_qpc) }
        .map_err(|error| format!("Could not timestamp audio start: {error}"))?;
    track.set_running(clock.raw_qpc_to_session_100ns(start_qpc) as f64 / 10_000.0);
    startup
        .send((config.role, StartupEvent::Started))
        .map_err(|_| "Replay audio startup coordinator stopped.".to_string())?;

    let capture_result = capture_loop(
        &capture_client,
        &event,
        process_handle.as_ref(),
        &packet_sender,
        &queue_depth,
        &track,
        &clock,
        &stop,
    );
    let _ = unsafe { audio_client.Stop() };
    drop(packet_sender);
    writer
        .join()
        .map_err(|_| "Audio WAV writer panicked.".to_string())??;
    match capture_result {
        Ok(TrackExit::Stopped) => track.set_terminal(AudioTrackState::Stopped, None),
        Ok(TrackExit::SourceEnded(reason)) => {
            track.set_terminal(AudioTrackState::Ended, Some(reason))
        }
        Err(error) => track.set_terminal(AudioTrackState::Error, Some(error)),
    }
    Ok(())
}

enum TrackExit {
    Stopped,
    SourceEnded(String),
}

fn capture_loop(
    capture: &IAudioCaptureClient,
    event: &OwnedHandle,
    process: Option<&OwnedHandle>,
    sender: &SyncSender<PcmPacket>,
    queue_depth: &AtomicUsize,
    track: &TrackShared,
    clock: &ReplaySessionClock,
    stop: &AtomicBool,
) -> Result<TrackExit, String> {
    loop {
        if stop.load(Ordering::Acquire) {
            return Ok(TrackExit::Stopped);
        }
        if let Some(process) = process {
            if unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 {
                return Ok(TrackExit::SourceEnded(
                    "The selected application exited; usable audio was finalized.".into(),
                ));
            }
        }
        let wait = unsafe { WaitForSingleObject(event.0, EVENT_POLL_MS) };
        if wait == WAIT_TIMEOUT {
            continue;
        }
        if wait != WAIT_OBJECT_0 {
            return Err(format!(
                "Waiting for WASAPI event failed with status {wait:?}."
            ));
        }
        loop {
            let available = unsafe { capture.GetNextPacketSize() }
                .map_err(|error| format!("Could not read WASAPI packet size: {error}"))?;
            if available == 0 {
                break;
            }
            capture_one(capture, sender, queue_depth, track, clock)?;
        }
    }
}

fn capture_one(
    capture: &IAudioCaptureClient,
    sender: &SyncSender<PcmPacket>,
    queue_depth: &AtomicUsize,
    track: &TrackShared,
    clock: &ReplaySessionClock,
) -> Result<(), String> {
    let block_align = track
        .lock()
        .status
        .format
        .as_ref()
        .map(|f| f.block_align)
        .ok_or_else(|| "Audio format is unavailable.".to_string())?;
    let mut data = std::ptr::null_mut();
    let mut frames = 0;
    let mut flags = 0;
    let mut device_position = 0;
    let mut qpc_position = 0;
    unsafe {
        capture.GetBuffer(
            &mut data,
            &mut frames,
            &mut flags,
            Some(&mut device_position),
            Some(&mut qpc_position),
        )
    }
    .map_err(|error| format!("Could not read WASAPI packet: {error}"))?;
    let release = PacketRelease {
        capture,
        frames,
        released: false,
    };
    let byte_count = frames as usize * block_align as usize;
    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
    let bytes = if silent {
        vec![0; byte_count]
    } else if data.is_null() {
        return Err("WASAPI returned null data for a non-silent packet.".into());
    } else {
        unsafe { slice::from_raw_parts(data, byte_count) }.to_vec()
    };
    release
        .release()
        .map_err(|error| format!("Could not release WASAPI packet: {error}"))?;
    let discontinuity = flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0;
    let timestamp_error = flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0;
    {
        let mut inner = track.lock();
        let first_qpc = if timestamp_error {
            None
        } else {
            let qpc = qpc_position as i64;
            Some(*inner.first_packet_qpc_100ns.get_or_insert(qpc))
        };
        let status = &mut inner.status;
        status.packet_count += 1;
        status.captured_sample_frames += u64::from(frames);
        status.silent_packet_count += u64::from(silent);
        status.discontinuity_count += u64::from(discontinuity);
        status.timestamp_error_count += u64::from(timestamp_error);
        status.expected_duration_from_samples_seconds = status
            .format
            .as_ref()
            .map(|f| f.duration_ms_for_frames(status.captured_sample_frames) / 1_000.0)
            .unwrap_or(0.0);
        if !timestamp_error {
            let qpc = qpc_position as i64;
            let first_qpc = first_qpc.expect("valid WASAPI timestamp has a first position");
            let packet_duration_100ns = (i128::from(frames) * 10_000_000
                / i128::from(status.format.as_ref().map(|f| f.sample_rate).unwrap_or(1)))
                as i64;
            let audio_end_qpc = qpc.saturating_add(packet_duration_100ns);
            status.newest_captured_audio_qpc_100ns = Some(
                status
                    .newest_captured_audio_qpc_100ns
                    .unwrap_or(i64::MIN)
                    .max(audio_end_qpc),
            );
            let session_end_100ns = clock.normalized_qpc_to_session_100ns(audio_end_qpc);
            status.latest_audio_position_ms = Some(session_end_100ns as f64 / 10_000.0);
            status.first_device_position.get_or_insert(device_position);
            status.latest_device_position = Some(device_position.saturating_add(u64::from(frames)));
            let qpc_elapsed = audio_end_qpc.saturating_sub(first_qpc).max(0) as f64 / 10_000.0;
            status.qpc_elapsed_duration_seconds = Some(qpc_elapsed / 1_000.0);
            let sample_ms = status.expected_duration_from_samples_seconds * 1_000.0;
            status.sample_qpc_difference_ms = Some(sample_ms - qpc_elapsed);
            status.estimated_clock_drift_ppm =
                (qpc_elapsed > 1.0).then(|| (sample_ms - qpc_elapsed) / qpc_elapsed * 1_000_000.0);
        }
    }
    let depth = queue_depth.fetch_add(1, Ordering::Relaxed) + 1;
    let packet = PcmPacket {
        bytes,
        frames,
        qpc_position_100ns: (!timestamp_error).then_some(qpc_position as i64),
        device_position: (!timestamp_error).then_some(device_position),
        silent,
        discontinuity,
        timestamp_error,
    };
    match sender.try_send(packet) {
        Ok(()) => {
            let mut inner = track.lock();
            inner.status.maximum_queue_depth = inner
                .status
                .maximum_queue_depth
                .max(depth.min(AUDIO_PACKET_QUEUE_CAPACITY));
            inner.status.current_queue_depth = depth;
            Ok(())
        }
        Err(TrySendError::Full(packet)) => {
            queue_depth.fetch_sub(1, Ordering::Relaxed);
            let mut inner = track.lock();
            inner.status.queue_full_events += 1;
            inner.status.dropped_packets += 1;
            inner.status.dropped_sample_frames += u64::from(packet.frames);
            Ok(())
        }
        Err(TrySendError::Disconnected(_)) => {
            Err("Audio WAV writer stopped while capture remained active.".into())
        }
    }
}

struct PacketRelease<'a> {
    capture: &'a IAudioCaptureClient,
    frames: u32,
    released: bool,
}
impl PacketRelease<'_> {
    fn release(mut self) -> windows::core::Result<()> {
        unsafe { self.capture.ReleaseBuffer(self.frames)? };
        self.released = true;
        Ok(())
    }
}
impl Drop for PacketRelease<'_> {
    fn drop(&mut self) {
        if !self.released {
            let _ = unsafe { self.capture.ReleaseBuffer(self.frames) };
        }
    }
}

struct ActiveWav {
    sequence: u64,
    writer: WavWriter,
    path: std::path::PathBuf,
    frames: u64,
    packets: u64,
    silent: u64,
    discontinuities: u64,
    timestamp_errors: u64,
    first_qpc: Option<i64>,
    last_qpc: Option<i64>,
    first_device_position: Option<u64>,
    last_device_position: Option<u64>,
}

fn writer_loop(
    track: Arc<TrackShared>,
    format: CaptureWaveFormat,
    clock: ReplaySessionClock,
    receiver: Receiver<PcmPacket>,
    depth: Arc<AtomicUsize>,
) -> Result<(), String> {
    let target_frames = u64::from(format.metadata.sample_rate) * AUDIO_SEGMENT_SECONDS;
    let block = format.metadata.block_align as usize;
    let mut sequence = 1u64;
    let mut active: Option<ActiveWav> = None;
    let mut expected_next_qpc: Option<i64> = None;
    while let Ok(packet) = receiver.recv() {
        depth.fetch_sub(1, Ordering::Relaxed);
        track.lock().status.current_queue_depth = depth.load(Ordering::Relaxed);
        let mut offset_frames = 0u64;
        while offset_frames < u64::from(packet.frames) {
            if active.is_none() {
                active = Some(create_wav(&track, &format, sequence)?);
                sequence += 1;
            }
            let wav = active.as_mut().unwrap();
            let remaining = target_frames.saturating_sub(wav.frames);
            let take = remaining.min(u64::from(packet.frames) - offset_frames);
            let start = offset_frames as usize * block;
            let end = (offset_frames + take) as usize * block;
            let write_started = Instant::now();
            wav.writer
                .write_packet(&packet.bytes[start..end])
                .map_err(audio_message)?;
            track.lock().status.writer_write_time_ms +=
                write_started.elapsed().as_secs_f64() * 1_000.0;
            let part_qpc = packet
                .qpc_position_100ns
                .map(|qpc| {
                    qpc.saturating_add(
                        (offset_frames as i128 * 10_000_000
                            / i128::from(format.metadata.sample_rate))
                            as i64,
                    )
                })
                .or(expected_next_qpc)
                .or(Some(clock.session_start_qpc_100ns));
            wav.first_qpc = wav.first_qpc.or(part_qpc);
            if let Some(qpc) = part_qpc {
                wav.last_qpc = Some(qpc.saturating_add(
                    (take as i128 * 10_000_000 / i128::from(format.metadata.sample_rate)) as i64,
                ));
            }
            expected_next_qpc = wav.last_qpc;
            let part_device_position = packet
                .device_position
                .map(|position| position.saturating_add(offset_frames));
            wav.first_device_position = wav.first_device_position.or(part_device_position);
            wav.last_device_position = part_device_position
                .map(|position| position.saturating_add(take))
                .or(wav.last_device_position);
            wav.frames += take;
            wav.packets += 1;
            wav.silent += u64::from(packet.silent);
            wav.discontinuities += u64::from(packet.discontinuity);
            wav.timestamp_errors += u64::from(packet.timestamp_error);
            offset_frames += take;
            if let Some(written_through) = wav.last_qpc {
                track.record_written_through(written_through);
            }
            if wav.frames >= target_frames {
                finalize_wav(&track, &format, &clock, active.take().unwrap())?;
            }
        }
        if active
            .as_ref()
            .and_then(|wav| wav.last_qpc)
            .is_some_and(|end| track.should_cut_after_packet(end))
        {
            finalize_wav(&track, &format, &clock, active.take().unwrap())?;
        }
    }
    if let Some(wav) = active {
        if wav.frames > 0 {
            finalize_wav(&track, &format, &clock, wav)?;
        }
    }
    Ok(())
}

fn create_wav(
    track: &TrackShared,
    format: &CaptureWaveFormat,
    sequence: u64,
) -> Result<ActiveWav, String> {
    let path = track.directory.join(format!("segment-{sequence:06}.wav"));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|error| {
            format!(
                "Could not create audio segment '{}': {error}",
                path.display()
            )
        })?;
    Ok(ActiveWav {
        sequence,
        writer: WavWriter::create(
            file,
            &format.bytes,
            format.metadata.block_align,
            !format.is_pcm,
        )
        .map_err(audio_message)?,
        path,
        frames: 0,
        packets: 0,
        silent: 0,
        discontinuities: 0,
        timestamp_errors: 0,
        first_qpc: None,
        last_qpc: None,
        first_device_position: None,
        last_device_position: None,
    })
}

fn finalize_wav(
    track: &TrackShared,
    format: &CaptureWaveFormat,
    clock: &ReplaySessionClock,
    wav: ActiveWav,
) -> Result<(), String> {
    let finalize_started = Instant::now();
    let data_bytes = wav.writer.finalize().map_err(audio_message)?;
    track.lock().status.writer_finalize_time_ms +=
        finalize_started.elapsed().as_secs_f64() * 1_000.0;
    let file_size = std::fs::metadata(&wav.path)
        .map_err(|error| format!("Could not inspect finalized audio segment: {error}"))?
        .len();
    if data_bytes == 0 || file_size == 0 {
        return Err("Finalized audio segment was empty.".into());
    }
    let duration_100ns =
        (wav.frames as i128 * 10_000_000 / i128::from(format.metadata.sample_rate)) as i64;
    let start_qpc = wav.first_qpc.unwrap_or(clock.session_start_qpc_100ns);
    let end_qpc = wav
        .last_qpc
        .unwrap_or(start_qpc.saturating_add(duration_100ns));
    let config = &track.configuration;
    let segment = CompletedAudioSegment {
        track_role: config.role,
        source_identifier: config.source_identifier().unwrap_or_default(),
        process_id: config.process_id,
        endpoint_id: config.endpoint_id.clone(),
        sequence_number: wav.sequence,
        file_path: wav.path.to_string_lossy().into_owned(),
        format: format.metadata.clone(),
        start_qpc_100ns: start_qpc,
        end_qpc_100ns: end_qpc,
        start_session_100ns: clock.normalized_qpc_to_session_100ns(start_qpc),
        end_session_100ns: clock.normalized_qpc_to_session_100ns(end_qpc),
        first_device_position: wav.first_device_position,
        last_device_position: wav.last_device_position,
        captured_sample_frames: wav.frames,
        written_sample_frames: wav.frames,
        actual_duration_ms: format.metadata.duration_ms_for_frames(wav.frames),
        packet_count: wav.packets,
        silent_packet_count: wav.silent,
        discontinuity_count: wav.discontinuities,
        timestamp_error_count: wav.timestamp_errors,
        dropped_packet_count: 0,
        dropped_frame_count: 0,
        finalized: true,
        file_size,
    };
    track.complete_segment(segment)
}

fn audio_message(error: AudioError) -> String {
    format!("{:?}: {}", error.code, error.message)
}

#[allow(dead_code)]
fn _structured_error(message: impl Into<String>) -> AudioError {
    AudioError::new(AudioErrorCode::CaptureFailed, message)
}
