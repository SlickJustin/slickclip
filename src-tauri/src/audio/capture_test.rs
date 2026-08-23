use std::path::PathBuf;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{E_ACCESSDENIED, WAIT_OBJECT_0, WAIT_TIMEOUT};
use windows::Win32::Media::Audio::{
    IAudioCaptureClient, IAudioClient, AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY,
    AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR, AUDCLNT_E_DEVICE_INVALIDATED,
    AUDCLNT_E_RESOURCES_INVALIDATED, AUDCLNT_E_SERVICE_NOT_RUNNING, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
    AUDCLNT_STREAMFLAGS_LOOPBACK,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};
use windows::Win32::System::Threading::{
    CreateEventW, OpenProcess, WaitForSingleObject, PROCESS_QUERY_LIMITED_INFORMATION,
    PROCESS_SYNCHRONIZE,
};

use super::microphone::activate_microphone;
use super::platform::{audio_dev_log, ComApartment, OwnedHandle};
use super::process_loopback::activate_process_loopback;
use super::sessions::resolve_process_metadata;
use super::types::{
    AudioCaptureCommandResult, AudioCaptureKind, AudioCaptureState, AudioCaptureStatus, AudioError,
    AudioErrorCode, AudioFormatDiagnostics, AudioFormatMetadata, AudioTimingTelemetry,
    AUDIO_TEST_DURATION_SECONDS,
};
use super::wav::{reserve_wav_path, utc_file_timestamp, WavWriter};
use super::wave_format::{CaptureWaveFormat, PROCESS_CLIENT_FORMAT_CANDIDATES};

const AUDIO_QUEUE_CAPACITY_PACKETS: usize = 64;
const PROCESS_EVENT_POLL_MS: u32 = 250;
const PROCESS_LOOPBACK_STREAM_FLAGS: u32 = AUDCLNT_STREAMFLAGS_LOOPBACK
    | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM;

#[derive(Clone)]
enum CaptureTarget {
    Microphone { device_id: String },
    Process { process_id: u32 },
}

impl CaptureTarget {
    fn kind(&self) -> AudioCaptureKind {
        match self {
            Self::Microphone { .. } => AudioCaptureKind::Microphone,
            Self::Process { .. } => AudioCaptureKind::ProcessLoopback,
        }
    }

    fn initial_label(&self) -> String {
        match self {
            Self::Microphone { .. } => "Selected microphone".to_string(),
            Self::Process { process_id } => format!("PID {process_id}"),
        }
    }
}

struct SharedCaptureStatus {
    status: Mutex<AudioCaptureStatus>,
}

impl SharedCaptureStatus {
    fn new() -> Self {
        Self {
            status: Mutex::new(AudioCaptureStatus::default()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, AudioCaptureStatus> {
        self.status
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn snapshot(&self) -> AudioCaptureStatus {
        self.lock().clone()
    }

    fn begin(&self, target: &CaptureTarget) {
        *self.lock() = AudioCaptureStatus {
            state: AudioCaptureState::Preparing,
            kind: Some(target.kind()),
            target_label: Some(target.initial_label()),
            ..Default::default()
        };
    }

    fn recording(
        &self,
        target_label: String,
        output_path: String,
        format: AudioFormatMetadata,
        format_diagnostics: Option<AudioFormatDiagnostics>,
    ) {
        let mut status = self.lock();
        status.state = AudioCaptureState::Recording;
        status.target_label = Some(target_label);
        status.output_path = Some(output_path);
        status.format = Some(format);
        status.format_diagnostics = format_diagnostics;
    }

    fn finalizing(&self) {
        self.lock().state = AudioCaptureState::Finalizing;
    }

    fn finish(&self, execution: CaptureExecution) {
        let mut status = self.lock();
        status.state = if execution.error.is_some() {
            AudioCaptureState::Error
        } else {
            AudioCaptureState::Completed
        };
        if let Some(path) = execution.output_path {
            status.output_path = Some(path);
        }
        if let Some(format) = execution.format {
            status.format = Some(format);
        }
        if let Some(format_diagnostics) = execution.format_diagnostics {
            status.format_diagnostics = Some(format_diagnostics);
        }
        status.timing = execution.timing;
        status.error = execution.error;
    }

    fn fail(&self, error: AudioError) {
        let mut status = self.lock();
        status.state = AudioCaptureState::Error;
        status.error = Some(error);
    }
}

pub struct AudioCaptureTestManager {
    output_directory: Arc<PathBuf>,
    shared: Arc<SharedCaptureStatus>,
    worker: Mutex<Option<JoinHandle<()>>>,
    cancel_requested: Arc<AtomicBool>,
}

impl AudioCaptureTestManager {
    pub fn new(output_directory: PathBuf) -> Self {
        Self {
            output_directory: Arc::new(output_directory),
            shared: Arc::new(SharedCaptureStatus::new()),
            worker: Mutex::new(None),
            cancel_requested: Arc::new(AtomicBool::new(false)),
        }
    }

    pub fn status(&self) -> AudioCaptureStatus {
        self.shared.snapshot()
    }

    pub fn start_microphone(&self, device_id: String) -> AudioCaptureCommandResult {
        self.start(CaptureTarget::Microphone { device_id })
    }

    pub fn start_process(&self, process_id: u32) -> AudioCaptureCommandResult {
        self.start(CaptureTarget::Process { process_id })
    }

    fn start(&self, target: CaptureTarget) -> AudioCaptureCommandResult {
        let mut worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if worker.as_ref().is_some_and(JoinHandle::is_finished) {
            if let Some(finished) = worker.take() {
                let _ = finished.join();
            }
        }
        if worker.is_some() || self.status().state.is_active() {
            let error = AudioError::new(
                AudioErrorCode::CaptureAlreadyRunning,
                "An audio capture test is already in progress.",
            );
            return AudioCaptureCommandResult {
                success: false,
                status: self.status(),
                error: Some(error),
            };
        }

        self.shared.begin(&target);
        self.cancel_requested.store(false, Ordering::Release);
        let shared = Arc::clone(&self.shared);
        let output_directory = Arc::clone(&self.output_directory);
        let cancel_requested = Arc::clone(&self.cancel_requested);
        let thread = thread::Builder::new()
            .name("slickclip-audio-test".to_string())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_capture_test(
                        target,
                        output_directory.as_ref(),
                        &shared,
                        &cancel_requested,
                    )
                }));
                match outcome {
                    Ok(execution) => shared.finish(execution),
                    Err(_) => shared.fail(AudioError::new(
                        AudioErrorCode::CaptureFailed,
                        "The audio capture worker panicked.",
                    )),
                }
            });
        match thread {
            Ok(thread) => {
                *worker = Some(thread);
                AudioCaptureCommandResult {
                    success: true,
                    status: self.status(),
                    error: None,
                }
            }
            Err(error) => {
                let error = AudioError::new(
                    AudioErrorCode::CaptureInitializationFailed,
                    format!("Could not start the audio capture worker: {error}"),
                );
                self.shared.fail(error.clone());
                AudioCaptureCommandResult {
                    success: false,
                    status: self.status(),
                    error: Some(error),
                }
            }
        }
    }

    pub fn shutdown_and_wait(&self) {
        self.cancel_requested.store(true, Ordering::Release);
        let worker = self
            .worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take();
        if let Some(worker) = worker {
            let _ = worker.join();
        }
    }
}

struct CaptureExecution {
    output_path: Option<String>,
    format: Option<AudioFormatMetadata>,
    format_diagnostics: Option<AudioFormatDiagnostics>,
    timing: Option<AudioTimingTelemetry>,
    error: Option<AudioError>,
}

impl CaptureExecution {
    fn failed(error: AudioError) -> Self {
        Self {
            output_path: None,
            format: None,
            format_diagnostics: None,
            timing: None,
            error: Some(error),
        }
    }
}

fn run_capture_test(
    target: CaptureTarget,
    output_directory: &PathBuf,
    shared: &SharedCaptureStatus,
    cancel_requested: &AtomicBool,
) -> CaptureExecution {
    match run_capture_test_inner(target, output_directory, shared, cancel_requested) {
        Ok(execution) => execution,
        Err(error) => CaptureExecution::failed(error),
    }
}

fn run_capture_test_inner(
    target: CaptureTarget,
    output_directory: &PathBuf,
    shared: &SharedCaptureStatus,
    cancel_requested: &AtomicBool,
) -> Result<CaptureExecution, AudioError> {
    let _com = ComApartment::initialize_mta("audio capture")?;
    audio_dev_log("audio capture worker COM MTA initialized");
    let (initialized, target_label, filename_prefix, process_handle) = match target {
        CaptureTarget::Microphone { device_id } => {
            let audio_client = activate_microphone(&device_id)?;
            let format = CaptureWaveFormat::endpoint_mix_format(&audio_client)?;
            initialize_capture_client(&audio_client, AUDCLNT_STREAMFLAGS_EVENTCALLBACK, &format)
                .map_err(|error| {
                    map_wasapi_error("initialize shared-mode microphone capture", error)
                })?;
            (
                InitializedAudioClient {
                    audio_client,
                    format,
                    format_diagnostics: None,
                },
                "Microphone".to_string(),
                "mic".to_string(),
                None,
            )
        }
        CaptureTarget::Process { process_id } => {
            let metadata = resolve_process_metadata(process_id).ok_or_else(|| {
                AudioError::new(
                    AudioErrorCode::ProcessUnavailable,
                    format!("The selected process PID {process_id} is no longer available."),
                )
            })?;
            let process_handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    false,
                    process_id,
                )
            }
            .map(OwnedHandle)
            .map_err(|error| {
                AudioError::new(
                    AudioErrorCode::ProcessUnavailable,
                    format!("Could not monitor selected process PID {process_id}: {error}"),
                )
            })?;
            let display_name = metadata
                .process_name
                .strip_suffix(".exe")
                .unwrap_or(&metadata.process_name)
                .to_string();
            (
                initialize_process_loopback_client(process_id)?,
                format!("{} (PID {process_id})", metadata.process_name),
                format!("process-{display_name}"),
                Some(process_handle),
            )
        }
    };
    let InitializedAudioClient {
        audio_client,
        format: capture_format,
        format_diagnostics,
    } = initialized;
    audio_dev_log("audio capture initialized");

    let event = unsafe { CreateEventW(None, false, false, None) }
        .map(OwnedHandle)
        .map_err(|error| {
            AudioError::new(
                AudioErrorCode::CaptureInitializationFailed,
                format!("Could not create the WASAPI sample-ready event: {error}"),
            )
        })?;
    unsafe { audio_client.SetEventHandle(event.0) }
        .map_err(|error| map_wasapi_error("configure event-driven audio capture", error))?;
    audio_dev_log("audio capture event configured");
    let capture_client: IAudioCaptureClient = unsafe { audio_client.GetService() }
        .map_err(|error| map_wasapi_error("obtain IAudioCaptureClient", error))?;

    let timestamp = utc_file_timestamp()?;
    let (output_path, output_file) =
        reserve_wav_path(output_directory, &filename_prefix, &timestamp)?;
    let wav_writer = WavWriter::create(
        output_file,
        &capture_format.bytes,
        capture_format.metadata.block_align,
        !capture_format.is_pcm,
    )?;
    let (sender, receiver) = mpsc::sync_channel::<PcmPacket>(AUDIO_QUEUE_CAPACITY_PACKETS);
    let queued_packets = Arc::new(AtomicUsize::new(0));
    let writer_queue = Arc::clone(&queued_packets);
    let writer_thread = thread::Builder::new()
        .name("slickclip-audio-wav".to_string())
        .spawn(move || -> Result<u64, AudioError> {
            let mut writer = wav_writer;
            let mut written_frames = 0u64;
            while let Ok(packet) = receiver.recv() {
                writer_queue.fetch_sub(1, Ordering::Relaxed);
                writer.write_packet(&packet.bytes)?;
                written_frames = written_frames.saturating_add(packet.frames as u64);
            }
            writer.finalize()?;
            Ok(written_frames)
        })
        .map_err(|error| {
            AudioError::new(
                AudioErrorCode::WavOutputFailed,
                format!("Could not start the WAV writer worker: {error}"),
            )
        })?;

    let output_path_string = output_path.to_string_lossy().into_owned();
    shared.recording(
        target_label,
        output_path_string.clone(),
        capture_format.metadata.clone(),
        format_diagnostics.clone(),
    );

    let mut start_qpc = 0i64;
    let mut qpc_frequency = 0i64;
    unsafe {
        QueryPerformanceFrequency(&mut qpc_frequency).map_err(|error| {
            AudioError::new(
                AudioErrorCode::CaptureInitializationFailed,
                format!("Could not read the performance-counter frequency: {error}"),
            )
        })?;
        QueryPerformanceCounter(&mut start_qpc).map_err(|error| {
            AudioError::new(
                AudioErrorCode::CaptureInitializationFailed,
                format!("Could not read the capture start performance counter: {error}"),
            )
        })?;
        audio_client
            .Start()
            .map_err(|error| map_wasapi_error("start audio capture", error))?;
    }
    audio_dev_log("audio client started");
    let wall_start = Instant::now();
    let mut statistics = CaptureStatistics::default();
    let mut capture_error = capture_packets(
        &capture_client,
        &event,
        process_handle.as_ref(),
        capture_format.metadata.block_align,
        &sender,
        &queued_packets,
        &mut statistics,
        wall_start,
        cancel_requested,
    );

    shared.finalizing();
    if let Err(error) = unsafe { audio_client.Stop() } {
        capture_error.get_or_insert_with(|| map_wasapi_error("stop audio capture", error));
    }
    let actual_wall_clock_duration_ms = wall_start.elapsed().as_secs_f64() * 1_000.0;
    let mut end_qpc = start_qpc;
    if let Err(error) = unsafe { QueryPerformanceCounter(&mut end_qpc) } {
        capture_error.get_or_insert_with(|| {
            AudioError::new(
                AudioErrorCode::CaptureFailed,
                format!("Could not read the capture end performance counter: {error}"),
            )
        });
    }

    drop(sender);
    let written_frames = match writer_thread.join() {
        Ok(Ok(written_frames)) => written_frames,
        Ok(Err(error)) => {
            capture_error.get_or_insert(error);
            0
        }
        Err(_) => {
            capture_error.get_or_insert_with(|| {
                AudioError::new(
                    AudioErrorCode::WavOutputFailed,
                    "The WAV writer worker panicked.",
                )
            });
            0
        }
    };

    let timing = AudioTimingTelemetry {
        monotonic_capture_start_qpc: start_qpc,
        monotonic_capture_end_qpc: end_qpc,
        qpc_frequency,
        actual_wall_clock_duration_ms,
        expected_duration_from_captured_frames_ms: capture_format
            .metadata
            .duration_ms_for_frames(statistics.captured_frames),
        expected_duration_in_wav_ms: capture_format
            .metadata
            .duration_ms_for_frames(written_frames),
        captured_sample_frames: statistics.captured_frames,
        written_sample_frames: written_frames,
        audio_packet_count: statistics.packet_count,
        silent_packet_count: statistics.silent_packet_count,
        discontinuity_count: statistics.discontinuity_count,
        timestamp_error_count: statistics.timestamp_error_count,
        first_device_position: statistics.first_device_position,
        last_device_position: statistics.last_device_position,
        first_qpc_position_100ns: statistics.first_qpc_position_100ns,
        last_qpc_position_100ns: statistics.last_qpc_position_100ns,
        queue_capacity_packets: AUDIO_QUEUE_CAPACITY_PACKETS,
        maximum_queue_depth: statistics.maximum_queue_depth,
        queue_full_events: statistics.queue_full_events,
        deliberately_dropped_packets: statistics.dropped_packets,
        deliberately_dropped_frames: statistics.dropped_frames,
    };

    Ok(CaptureExecution {
        output_path: Some(output_path_string),
        format: Some(capture_format.metadata),
        format_diagnostics,
        timing: Some(timing),
        error: capture_error,
    })
}

pub(crate) struct InitializedAudioClient {
    pub(crate) audio_client: IAudioClient,
    pub(crate) format: CaptureWaveFormat,
    pub(crate) format_diagnostics: Option<AudioFormatDiagnostics>,
}

pub(crate) fn initialize_capture_client(
    audio_client: &IAudioClient,
    stream_flags: u32,
    format: &CaptureWaveFormat,
) -> windows::core::Result<()> {
    // Shared mode follows Microsoft's ApplicationLoopback contract: zero lets
    // the engine select buffer duration, periodicity must be zero, and no
    // session GUID is needed. `format` owns the bytes for the entire call.
    unsafe {
        audio_client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            stream_flags,
            0,
            0,
            format.as_wave_format_ptr(),
            None,
        )
    }
}

pub(crate) fn initialize_process_loopback_client(
    process_id: u32,
) -> Result<InitializedAudioClient, AudioError> {
    let first_client = activate_process_loopback(process_id)?;
    let get_mix_format_status = CaptureWaveFormat::process_mix_format_diagnostic(&first_client)?;
    audio_dev_log(format!(
        "PID {process_id}: GetMixFormat: {get_mix_format_status}"
    ));

    let mut first_client = Some(first_client);
    let mut initialization_failures = Vec::new();
    for candidate in PROCESS_CLIENT_FORMAT_CANDIDATES {
        let audio_client = match first_client.take() {
            Some(client) => client,
            None => activate_process_loopback(process_id)?,
        };
        let format = candidate.build();
        audio_dev_log(format!(
            "PID {process_id}: trying process-loopback client format {}",
            candidate.label()
        ));
        match initialize_capture_client(&audio_client, PROCESS_LOOPBACK_STREAM_FLAGS, &format) {
            Ok(()) => {
                audio_dev_log(format!(
                    "PID {process_id}: selected process-loopback client format {}",
                    candidate.label()
                ));
                return Ok(InitializedAudioClient {
                    audio_client,
                    format,
                    format_diagnostics: Some(AudioFormatDiagnostics {
                        get_mix_format_status,
                        format_role: "Requested process-loopback client capture format; Windows may convert the isolated process audio with AUTOCONVERTPCM".to_string(),
                    }),
                });
            }
            Err(error) => {
                audio_dev_log(format!(
                    "PID {process_id}: process-loopback format {} failed: {error}",
                    candidate.label()
                ));
                initialization_failures.push(format!("{}: {error}", candidate.label()));
            }
        }
    }

    Err(AudioError::new(
        AudioErrorCode::CaptureInitializationFailed,
        format!(
            "Could not initialize process-loopback capture for PID {process_id} with any supported client format. GetMixFormat: {get_mix_format_status}. Attempts: {}",
            initialization_failures.join("; ")
        ),
    ))
}

fn capture_packets(
    capture_client: &IAudioCaptureClient,
    event: &OwnedHandle,
    process_handle: Option<&OwnedHandle>,
    block_align: u16,
    sender: &SyncSender<PcmPacket>,
    queued_packets: &AtomicUsize,
    statistics: &mut CaptureStatistics,
    wall_start: Instant,
    cancel_requested: &AtomicBool,
) -> Option<AudioError> {
    let duration = Duration::from_secs(AUDIO_TEST_DURATION_SECONDS);
    loop {
        if wall_start.elapsed() >= duration {
            return None;
        }
        if cancel_requested.load(Ordering::Acquire) {
            return None;
        }
        if let Some(process) = process_handle {
            if unsafe { WaitForSingleObject(process.0, 0) } == WAIT_OBJECT_0 {
                return Some(AudioError::new(
                    AudioErrorCode::ProcessExited,
                    "The selected process exited before the audio test completed; the partial WAV was finalized.",
                ));
            }
        }

        let remaining_ms = duration
            .saturating_sub(wall_start.elapsed())
            .as_millis()
            .clamp(1, PROCESS_EVENT_POLL_MS as u128) as u32;
        let wait = unsafe { WaitForSingleObject(event.0, remaining_ms) };
        if wait == WAIT_TIMEOUT {
            continue;
        }
        if wait != WAIT_OBJECT_0 {
            return Some(AudioError::new(
                AudioErrorCode::CaptureFailed,
                format!("Waiting for the WASAPI audio event failed with status {wait:?}."),
            ));
        }

        loop {
            let available_frames = match unsafe { capture_client.GetNextPacketSize() } {
                Ok(frames) => frames,
                Err(error) => {
                    return Some(map_wasapi_error("read the next audio packet size", error));
                }
            };
            if available_frames == 0 {
                break;
            }
            if let Some(error) = capture_one_packet(
                capture_client,
                block_align,
                sender,
                queued_packets,
                statistics,
            ) {
                return Some(error);
            }
        }
    }
}

fn capture_one_packet(
    capture_client: &IAudioCaptureClient,
    block_align: u16,
    sender: &SyncSender<PcmPacket>,
    queued_packets: &AtomicUsize,
    statistics: &mut CaptureStatistics,
) -> Option<AudioError> {
    let mut data = std::ptr::null_mut();
    let mut frames = 0u32;
    let mut flags = 0u32;
    let mut device_position = 0u64;
    let mut qpc_position = 0u64;
    if let Err(error) = unsafe {
        capture_client.GetBuffer(
            &mut data,
            &mut frames,
            &mut flags,
            Some(&mut device_position),
            Some(&mut qpc_position),
        )
    } {
        return Some(map_wasapi_error("read an audio packet", error));
    }
    let packet = CapturePacketRelease::new(capture_client, frames);

    let byte_count = match (frames as usize).checked_mul(block_align as usize) {
        Some(byte_count) => byte_count,
        None => {
            return Some(AudioError::new(
                AudioErrorCode::CaptureFailed,
                "The WASAPI packet size overflowed addressable memory.",
            ));
        }
    };
    let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
    let packet_bytes = if silent {
        vec![0; byte_count]
    } else if data.is_null() {
        return Some(AudioError::new(
            AudioErrorCode::CaptureFailed,
            "WASAPI returned a non-silent packet without sample data.",
        ));
    } else {
        unsafe { slice::from_raw_parts(data, byte_count) }.to_vec()
    };
    if let Err(error) = packet.release() {
        return Some(map_wasapi_error("release an audio packet", error));
    }

    statistics.packet_count += 1;
    statistics.captured_frames = statistics.captured_frames.saturating_add(frames as u64);
    if silent {
        statistics.silent_packet_count += 1;
    }
    if flags & AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY.0 as u32 != 0 {
        statistics.discontinuity_count += 1;
    }
    let timestamp_error = flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0;
    if timestamp_error {
        statistics.timestamp_error_count += 1;
    } else {
        statistics
            .first_device_position
            .get_or_insert(device_position);
        statistics.last_device_position = Some(device_position);
        statistics
            .first_qpc_position_100ns
            .get_or_insert(qpc_position);
        statistics.last_qpc_position_100ns = Some(qpc_position);
    }

    let depth = queued_packets.fetch_add(1, Ordering::Relaxed) + 1;
    match sender.try_send(PcmPacket {
        bytes: packet_bytes,
        frames,
    }) {
        Ok(()) => {
            statistics.maximum_queue_depth = statistics
                .maximum_queue_depth
                .max(depth.min(AUDIO_QUEUE_CAPACITY_PACKETS));
            None
        }
        Err(TrySendError::Full(_)) => {
            queued_packets.fetch_sub(1, Ordering::Relaxed);
            statistics.queue_full_events += 1;
            statistics.dropped_packets += 1;
            statistics.dropped_frames = statistics.dropped_frames.saturating_add(frames as u64);
            None
        }
        Err(TrySendError::Disconnected(_)) => {
            queued_packets.fetch_sub(1, Ordering::Relaxed);
            Some(AudioError::new(
                AudioErrorCode::WavOutputFailed,
                "The WAV writer stopped while audio capture was active.",
            ))
        }
    }
}

struct CapturePacketRelease<'a> {
    capture_client: &'a IAudioCaptureClient,
    frames: u32,
    released: bool,
}

impl<'a> CapturePacketRelease<'a> {
    fn new(capture_client: &'a IAudioCaptureClient, frames: u32) -> Self {
        Self {
            capture_client,
            frames,
            released: false,
        }
    }

    fn release(mut self) -> windows::core::Result<()> {
        let result = unsafe { self.capture_client.ReleaseBuffer(self.frames) };
        // ReleaseBuffer was called; never call it a second time, even if it failed.
        self.released = true;
        result
    }
}

impl Drop for CapturePacketRelease<'_> {
    fn drop(&mut self) {
        if !self.released {
            let _ = unsafe { self.capture_client.ReleaseBuffer(self.frames) };
            self.released = true;
        }
    }
}

struct PcmPacket {
    bytes: Vec<u8>,
    frames: u32,
}

#[derive(Default)]
struct CaptureStatistics {
    captured_frames: u64,
    packet_count: u64,
    silent_packet_count: u64,
    discontinuity_count: u64,
    timestamp_error_count: u64,
    first_device_position: Option<u64>,
    last_device_position: Option<u64>,
    first_qpc_position_100ns: Option<u64>,
    last_qpc_position_100ns: Option<u64>,
    maximum_queue_depth: usize,
    queue_full_events: u64,
    dropped_packets: u64,
    dropped_frames: u64,
}

fn map_wasapi_error(context: &str, error: windows::core::Error) -> AudioError {
    let code = if error.code() == AUDCLNT_E_DEVICE_INVALIDATED
        || error.code() == AUDCLNT_E_RESOURCES_INVALIDATED
    {
        AudioErrorCode::DeviceInvalidated
    } else if error.code() == AUDCLNT_E_SERVICE_NOT_RUNNING {
        AudioErrorCode::AudioServiceUnavailable
    } else if error.code() == E_ACCESSDENIED {
        AudioErrorCode::MicrophoneAccessDenied
    } else {
        AudioErrorCode::CaptureFailed
    };
    AudioError::new(code, format!("Could not {context}: {error}"))
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::{
        CaptureExecution, CaptureTarget, SharedCaptureStatus, PROCESS_LOOPBACK_STREAM_FLAGS,
    };
    use crate::audio::types::{AudioCaptureState, AudioError, AudioErrorCode};
    use windows::Win32::Media::Audio::{
        AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_EVENTCALLBACK,
        AUDCLNT_STREAMFLAGS_LOOPBACK, AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
    };

    #[test]
    fn process_loopback_uses_the_minimal_official_conversion_flags() {
        assert_eq!(
            PROCESS_LOOPBACK_STREAM_FLAGS,
            AUDCLNT_STREAMFLAGS_LOOPBACK
                | AUDCLNT_STREAMFLAGS_EVENTCALLBACK
                | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
        );
        assert_eq!(
            PROCESS_LOOPBACK_STREAM_FLAGS & AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
            0
        );
    }
    #[test]
    fn shared_capture_status_transitions_without_audio_hardware() {
        let shared = SharedCaptureStatus::new();
        shared.begin(&CaptureTarget::Process { process_id: 123 });
        assert_eq!(shared.snapshot().state, AudioCaptureState::Preparing);

        shared.finish(CaptureExecution {
            output_path: Some(PathBuf::from("test.wav").to_string_lossy().into_owned()),
            format: None,
            format_diagnostics: None,
            timing: None,
            error: None,
        });
        assert_eq!(shared.snapshot().state, AudioCaptureState::Completed);

        shared.fail(AudioError::new(
            AudioErrorCode::CaptureFailed,
            "test failure",
        ));
        assert_eq!(shared.snapshot().state, AudioCaptureState::Error);
    }
}
