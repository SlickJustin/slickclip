mod capture_test;
mod devices;
mod microphone;
mod platform;
mod process_loopback;
mod sessions;
mod types;
mod wav;
mod wave_format;

use tauri::State;

pub use capture_test::AudioCaptureTestManager;
use types::{
    ApplicationAudioListResult, AudioCaptureCommandResult, AudioCaptureStatus, AudioError,
    AudioErrorCode, MicrophoneListResult, ProcessActivationProbeResult, ProcessLoopbackCapability,
    PROCESS_LOOPBACK_MINIMUM_BUILD,
};

#[tauri::command]
pub async fn list_audio_microphones() -> MicrophoneListResult {
    match tauri::async_runtime::spawn_blocking(devices::enumerate_microphones).await {
        Ok(result) => result,
        Err(error) => MicrophoneListResult {
            success: false,
            devices: Vec::new(),
            error: Some(worker_error("microphone enumeration", error)),
        },
    }
}

#[tauri::command]
pub async fn probe_process_audio_activation(process_id: u32) -> ProcessActivationProbeResult {
    if !cfg!(debug_assertions) {
        let error = AudioError::new(
            AudioErrorCode::ProcessLoopbackActivationFailed,
            "The process-audio activation probe is available only in development builds.",
        );
        return ProcessActivationProbeResult {
            success: false,
            process_id,
            message: error.message.clone(),
            error: Some(error),
        };
    }

    match tauri::async_runtime::spawn_blocking(move || {
        let _com = platform::ComApartment::initialize_mta("process-loopback activation probe")?;
        platform::audio_dev_log(format!(
            "PID {process_id}: activation-probe COM MTA initialized"
        ));
        process_loopback::activate_process_loopback(process_id)?;
        platform::audio_dev_log(format!(
            "PID {process_id}: activation probe released IAudioClient normally"
        ));
        Ok::<(), AudioError>(())
    })
    .await
    {
        Ok(Ok(())) => ProcessActivationProbeResult {
            success: true,
            process_id,
            message: format!(
                "Process-loopback activation succeeded for PID {process_id}; no capture was initialized."
            ),
            error: None,
        },
        Ok(Err(error)) => ProcessActivationProbeResult {
            success: false,
            process_id,
            message: error.message.clone(),
            error: Some(error),
        },
        Err(error) => {
            let error = worker_error("process-loopback activation probe", error);
            ProcessActivationProbeResult {
                success: false,
                process_id,
                message: error.message.clone(),
                error: Some(error),
            }
        }
    }
}

#[tauri::command]
pub async fn list_application_audio_processes() -> ApplicationAudioListResult {
    match tauri::async_runtime::spawn_blocking(sessions::enumerate_application_audio).await {
        Ok(result) => result,
        Err(error) => ApplicationAudioListResult {
            success: false,
            applications: Vec::new(),
            capability: ProcessLoopbackCapability {
                available: false,
                windows_build: None,
                minimum_windows_build: PROCESS_LOOPBACK_MINIMUM_BUILD,
                status: "Application audio discovery worker failed.".to_string(),
                error: None,
            },
            error: Some(worker_error("application audio enumeration", error)),
        },
    }
}

#[tauri::command]
pub async fn get_process_loopback_capability() -> ProcessLoopbackCapability {
    match tauri::async_runtime::spawn_blocking(process_loopback::process_loopback_capability).await
    {
        Ok(result) => result,
        Err(error) => {
            let error = worker_error("process-loopback capability detection", error);
            ProcessLoopbackCapability {
                available: false,
                windows_build: None,
                minimum_windows_build: PROCESS_LOOPBACK_MINIMUM_BUILD,
                status: error.message.clone(),
                error: Some(error),
            }
        }
    }
}

#[tauri::command]
pub fn start_microphone_audio_test(
    manager: State<'_, AudioCaptureTestManager>,
    device_id: String,
) -> AudioCaptureCommandResult {
    manager.start_microphone(device_id)
}

#[tauri::command]
pub fn start_process_audio_test(
    manager: State<'_, AudioCaptureTestManager>,
    process_id: u32,
) -> AudioCaptureCommandResult {
    manager.start_process(process_id)
}

#[tauri::command]
pub fn get_audio_capture_test_status(
    manager: State<'_, AudioCaptureTestManager>,
) -> AudioCaptureStatus {
    manager.status()
}

fn worker_error(context: &str, error: impl std::fmt::Display) -> AudioError {
    AudioError::new(
        AudioErrorCode::CaptureFailed,
        format!("The {context} worker failed: {error}"),
    )
}
