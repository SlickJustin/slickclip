use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eMultimedia, IMMDevice, IMMDeviceEnumerator, MMDeviceEnumerator,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_ALL, STGM_READ};

use super::platform::{take_task_mem_string, ComApartment};
use super::types::{AudioError, AudioErrorCode, MicrophoneEndpoint, MicrophoneListResult};

pub fn enumerate_microphones() -> MicrophoneListResult {
    match enumerate_microphones_inner() {
        Ok(devices) => MicrophoneListResult {
            success: true,
            devices,
            error: None,
        },
        Err(error) => MicrophoneListResult {
            success: false,
            devices: Vec::new(),
            error: Some(error),
        },
    }
}

fn enumerate_microphones_inner() -> Result<Vec<MicrophoneEndpoint>, AudioError> {
    let _com = ComApartment::initialize_mta("microphone enumeration")?;
    let enumerator = create_device_enumerator()?;
    let multimedia_default = default_device_id(&enumerator, eMultimedia);
    let communications_default = default_device_id(&enumerator, eCommunications);
    let collection = unsafe { enumerator.EnumAudioEndpoints(eCapture, DEVICE_STATE_ACTIVE) }
        .map_err(|error| endpoint_error("enumerate active microphone endpoints", error))?;
    let count = unsafe { collection.GetCount() }
        .map_err(|error| endpoint_error("read the microphone endpoint count", error))?;
    let mut devices = Vec::with_capacity(count as usize);

    for index in 0..count {
        let device = unsafe { collection.Item(index) }
            .map_err(|error| endpoint_error("open a microphone endpoint", error))?;
        let id = device_id(&device)?;
        let friendly_name =
            device_friendly_name(&device).unwrap_or_else(|_| format!("Microphone {}", index + 1));
        devices.push(MicrophoneEndpoint {
            is_default_multimedia: multimedia_default.as_deref() == Some(id.as_str()),
            is_default_communications: communications_default.as_deref() == Some(id.as_str()),
            id,
            friendly_name,
            state: "active".to_string(),
        });
    }

    devices.sort_by(|left, right| {
        right
            .is_default_communications
            .cmp(&left.is_default_communications)
            .then_with(|| right.is_default_multimedia.cmp(&left.is_default_multimedia))
            .then_with(|| left.friendly_name.cmp(&right.friendly_name))
    });
    Ok(devices)
}

pub fn create_device_enumerator() -> Result<IMMDeviceEnumerator, AudioError> {
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
        .map_err(|error| endpoint_error("create the Windows audio device enumerator", error))
}

pub fn get_microphone_device(device_id_value: &str) -> Result<IMMDevice, AudioError> {
    let enumerator = create_device_enumerator()?;
    unsafe { enumerator.GetDevice(&windows::core::HSTRING::from(device_id_value)) }.map_err(
        |error| {
            AudioError::new(
                AudioErrorCode::MicrophoneUnavailable,
                format!("The selected microphone endpoint is unavailable: {error}"),
            )
        },
    )
}

pub fn device_id(device: &IMMDevice) -> Result<String, AudioError> {
    let value = unsafe { device.GetId() }
        .map_err(|error| endpoint_error("read the Windows endpoint identifier", error))?;
    unsafe { take_task_mem_string(value) }
}

fn default_device_id(
    enumerator: &IMMDeviceEnumerator,
    role: windows::Win32::Media::Audio::ERole,
) -> Option<String> {
    let device = unsafe { enumerator.GetDefaultAudioEndpoint(eCapture, role) }.ok()?;
    device_id(&device).ok()
}

fn device_friendly_name(device: &IMMDevice) -> Result<String, AudioError> {
    let store = unsafe { device.OpenPropertyStore(STGM_READ) }
        .map_err(|error| endpoint_error("open microphone endpoint properties", error))?;
    let mut value = unsafe { store.GetValue(&PKEY_Device_FriendlyName) }
        .map_err(|error| endpoint_error("read the microphone friendly name", error))?;
    let name_pointer = unsafe { PropVariantToStringAlloc(&value) }
        .map_err(|error| endpoint_error("convert the microphone friendly name", error));
    let _ = unsafe { PropVariantClear(&mut value) };
    let name_pointer = name_pointer?;
    unsafe { take_task_mem_string(name_pointer) }
}

fn endpoint_error(context: &str, error: windows::core::Error) -> AudioError {
    AudioError::new(
        AudioErrorCode::EndpointUnavailable,
        format!("Could not {context}: {error}"),
    )
}
