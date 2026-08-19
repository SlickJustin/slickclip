use std::ffi::c_void;
use std::fmt::Display;
use std::io::{self, Write};

use windows::core::PWSTR;
use windows::Win32::Foundation::{CloseHandle, HANDLE};
use windows::Win32::System::Com::{
    CoInitializeEx, CoTaskMemFree, CoUninitialize, COINIT_MULTITHREADED,
};

use super::types::{AudioError, AudioErrorCode};

pub fn audio_dev_log(message: impl Display) {
    #[cfg(debug_assertions)]
    {
        eprintln!("[JustIn Replay audio] {message}");
        let _ = io::stderr().flush();
    }
    #[cfg(not(debug_assertions))]
    let _ = message;
}

pub struct ComApartment;

impl ComApartment {
    pub fn initialize_mta(context: &str) -> Result<Self, AudioError> {
        let result = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        if result.is_ok() {
            Ok(Self)
        } else {
            Err(AudioError::new(
                AudioErrorCode::CaptureInitializationFailed,
                format!("Could not initialize COM for {context}: {result:?}"),
            ))
        }
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe { CoUninitialize() };
    }
}

pub unsafe fn take_task_mem_string(value: PWSTR) -> Result<String, AudioError> {
    if value.is_null() {
        return Ok(String::new());
    }
    let result = unsafe { value.to_string() }.map_err(|error| {
        AudioError::new(
            AudioErrorCode::CaptureFailed,
            format!("Windows returned invalid UTF-16 text: {error}"),
        )
    });
    unsafe { CoTaskMemFree(Some(value.0.cast::<c_void>())) };
    result
}

pub struct OwnedHandle(pub HANDLE);

impl Drop for OwnedHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

unsafe impl Send for OwnedHandle {}
