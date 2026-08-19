use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Media::Audio::{IAudioClient, DEVICE_STATE_ACTIVE};
use windows::Win32::System::Com::CLSCTX_ALL;

use super::devices::get_microphone_device;
use super::types::{AudioError, AudioErrorCode};

pub fn activate_microphone(device_id: &str) -> Result<IAudioClient, AudioError> {
    let device = get_microphone_device(device_id)?;
    let state = unsafe { device.GetState() }.map_err(|error| {
        AudioError::new(
            AudioErrorCode::MicrophoneUnavailable,
            format!("Could not read the selected microphone state: {error}"),
        )
    })?;
    if state != DEVICE_STATE_ACTIVE {
        return Err(AudioError::new(
            AudioErrorCode::MicrophoneUnavailable,
            "The selected microphone is no longer active.",
        ));
    }

    unsafe { device.Activate::<IAudioClient>(CLSCTX_ALL, None) }.map_err(|error| {
        let code = if error.code() == E_ACCESSDENIED {
            AudioErrorCode::MicrophoneAccessDenied
        } else {
            AudioErrorCode::CaptureInitializationFailed
        };
        AudioError::new(
            code,
            format!("Could not activate shared-mode microphone capture: {error}"),
        )
    })
}
