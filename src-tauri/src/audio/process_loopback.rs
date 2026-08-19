use std::mem::{size_of, ManuallyDrop};
use std::sync::mpsc::{self, Receiver, Sender};

use windows::core::{implement, Error as WindowsError, IUnknown, Interface, Ref, HRESULT};
use windows::Wdk::System::SystemServices::RtlGetVersion;
use windows::Win32::Media::Audio::{
    ActivateAudioInterfaceAsync, IActivateAudioInterfaceAsyncOperation,
    IActivateAudioInterfaceCompletionHandler, IActivateAudioInterfaceCompletionHandler_Impl,
    IAudioClient, AUDIOCLIENT_ACTIVATION_PARAMS, AUDIOCLIENT_ACTIVATION_PARAMS_0,
    AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK, AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE, VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
};
use windows::Win32::System::Com::StructuredStorage::{
    PROPVARIANT, PROPVARIANT_0, PROPVARIANT_0_0, PROPVARIANT_0_0_0,
};
use windows::Win32::System::Com::BLOB;
use windows::Win32::System::SystemInformation::OSVERSIONINFOW;
use windows::Win32::System::Variant::VT_BLOB;

use super::platform::audio_dev_log;
use super::types::{
    AudioError, AudioErrorCode, ProcessLoopbackCapability, PROCESS_LOOPBACK_MINIMUM_BUILD,
};

pub fn process_loopback_capability() -> ProcessLoopbackCapability {
    match windows_build_number() {
        Ok(build) if build >= PROCESS_LOOPBACK_MINIMUM_BUILD => ProcessLoopbackCapability {
            available: true,
            windows_build: Some(build),
            minimum_windows_build: PROCESS_LOOPBACK_MINIMUM_BUILD,
            status: format!("Available on Windows build {build}."),
            error: None,
        },
        Ok(build) => {
            let message = format!(
                "Process loopback requires Windows build {PROCESS_LOOPBACK_MINIMUM_BUILD} or later; this system reports build {build}."
            );
            ProcessLoopbackCapability {
                available: false,
                windows_build: Some(build),
                minimum_windows_build: PROCESS_LOOPBACK_MINIMUM_BUILD,
                status: message.clone(),
                error: Some(AudioError::new(
                    AudioErrorCode::ProcessLoopbackUnsupported,
                    message,
                )),
            }
        }
        Err(error) => ProcessLoopbackCapability {
            available: false,
            windows_build: None,
            minimum_windows_build: PROCESS_LOOPBACK_MINIMUM_BUILD,
            status: error.message.clone(),
            error: Some(error),
        },
    }
}

pub fn activate_process_loopback(process_id: u32) -> Result<IAudioClient, AudioError> {
    let capability = process_loopback_capability();
    if !capability.available {
        return Err(capability.error.unwrap_or_else(|| {
            AudioError::new(
                AudioErrorCode::ProcessLoopbackUnsupported,
                capability.status,
            )
        }));
    }

    let mut context = ProcessLoopbackActivationContext::new(process_id);
    audio_dev_log(format!(
        "PID {process_id}: process-loopback activation parameters created"
    ));
    context.begin(process_id)?;
    context.wait_for_audio_client(process_id)
}

/// Owns every Rust and COM value used by one asynchronous activation attempt.
///
/// The PROPVARIANT's VT_BLOB is a non-owning view into `parameters`. The Box keeps
/// that allocation stable even if this context moves. We intentionally do not call
/// PropVariantClear: the blob points at Rust-owned memory, just like the local
/// AUDIOCLIENT_ACTIVATION_PARAMS in Microsoft's ApplicationLoopback sample.
struct ProcessLoopbackActivationContext {
    parameters: Box<AUDIOCLIENT_ACTIVATION_PARAMS>,
    variant: ManuallyDrop<PROPVARIANT>,
    handler: IActivateAudioInterfaceCompletionHandler,
    operation: Option<IActivateAudioInterfaceAsyncOperation>,
    receiver: Receiver<windows::core::Result<IUnknown>>,
}

impl ProcessLoopbackActivationContext {
    fn new(process_id: u32) -> Self {
        let mut parameters = Box::new(AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: process_id,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        });
        let blob = BLOB {
            cbSize: size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            pBlobData: parameters.as_mut() as *mut AUDIOCLIENT_ACTIVATION_PARAMS as *mut u8,
        };
        let variant = ManuallyDrop::new(PROPVARIANT {
            Anonymous: PROPVARIANT_0 {
                Anonymous: ManuallyDrop::new(PROPVARIANT_0_0 {
                    vt: VT_BLOB,
                    wReserved1: 0,
                    wReserved2: 0,
                    wReserved3: 0,
                    Anonymous: PROPVARIANT_0_0_0 { blob },
                }),
            },
        });
        let (sender, receiver) = mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler = ActivationHandler(sender).into();

        Self {
            parameters,
            variant,
            handler,
            operation: None,
            receiver,
        }
    }

    fn begin(&mut self, process_id: u32) -> Result<(), AudioError> {
        debug_assert_eq!(
            self.parameters.as_ref() as *const AUDIOCLIENT_ACTIVATION_PARAMS as *mut u8,
            unsafe {
                (&*self.variant.Anonymous.Anonymous)
                    .Anonymous
                    .blob
                    .pBlobData
            }
        );
        let operation = unsafe {
            ActivateAudioInterfaceAsync(
                VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
                &IAudioClient::IID,
                Some(&*self.variant),
                &self.handler,
            )
        }
        .map_err(|error| activation_error(process_id, error))?;
        self.operation = Some(operation);
        audio_dev_log(format!(
            "PID {process_id}: ActivateAudioInterfaceAsync returned successfully"
        ));
        Ok(())
    }

    fn wait_for_audio_client(self, process_id: u32) -> Result<IAudioClient, AudioError> {
        // Do not time out: Windows may still be reading `variant`/`parameters` and
        // retaining `handler` while the asynchronous operation is outstanding.
        let interface = self
            .receiver
            .recv()
            .map_err(|error| {
                AudioError::new(
                    AudioErrorCode::ProcessLoopbackActivationFailed,
                    format!(
                        "Process-loopback completion channel closed for PID {process_id}: {error}"
                    ),
                )
            })?
            .map_err(|error| activation_error(process_id, error))?;
        let audio_client = interface
            .cast::<IAudioClient>()
            .map_err(|error| activation_error(process_id, error))?;
        audio_dev_log(format!("PID {process_id}: IAudioClient obtained"));
        Ok(audio_client)
    }
}

fn windows_build_number() -> Result<u32, AudioError> {
    let mut version = OSVERSIONINFOW {
        dwOSVersionInfoSize: size_of::<OSVERSIONINFOW>() as u32,
        ..Default::default()
    };
    let status = unsafe { RtlGetVersion(&mut version) };
    if status.is_ok() {
        Ok(version.dwBuildNumber)
    } else {
        Err(AudioError::new(
            AudioErrorCode::ProcessLoopbackUnsupported,
            format!(
                "Could not determine the Windows build for process-loopback support: {status:?}"
            ),
        ))
    }
}

// windows-rs generates the COM vtable, QueryInterface/AddRef/Release, and (by
// default in 0.62) IAgileObject plus free-threaded IMarshal support.
#[implement(IActivateAudioInterfaceCompletionHandler)]
struct ActivationHandler(Sender<windows::core::Result<IUnknown>>);

impl IActivateAudioInterfaceCompletionHandler_Impl for ActivationHandler_Impl {
    fn ActivateCompleted(
        &self,
        operation: Ref<'_, IActivateAudioInterfaceAsyncOperation>,
    ) -> windows::core::Result<()> {
        audio_dev_log("process-loopback completion callback entered");
        let result = operation.ok().and_then(retrieve_activation_result);
        audio_dev_log(match &result {
            Ok(_) => "process-loopback GetActivateResult returned successfully".to_string(),
            Err(error) => format!("process-loopback GetActivateResult failed: {error}"),
        });
        let _ = self.0.send(result);
        Ok(())
    }
}

fn retrieve_activation_result(
    operation: &IActivateAudioInterfaceAsyncOperation,
) -> windows::core::Result<IUnknown> {
    let mut activation_result = HRESULT::default();
    let mut interface = None;
    unsafe { operation.GetActivateResult(&mut activation_result, &mut interface) }?;
    activation_result.ok()?;
    interface.ok_or_else(|| {
        WindowsError::new(
            windows::Win32::Foundation::E_FAIL,
            "Process-loopback activation returned no audio interface",
        )
    })
}

fn activation_error(process_id: u32, error: WindowsError) -> AudioError {
    AudioError::new(
        AudioErrorCode::ProcessLoopbackActivationFailed,
        format!("Could not activate process-loopback capture for PID {process_id}: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use std::mem::{align_of, offset_of, size_of};
    use std::sync::mpsc::TryRecvError;

    use windows::core::Interface;
    use windows::Win32::Media::Audio::{
        IActivateAudioInterfaceCompletionHandler, AUDIOCLIENT_ACTIVATION_PARAMS,
        AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS,
    };
    use windows::Win32::System::Com::StructuredStorage::PROPVARIANT;
    use windows::Win32::System::Com::{IAgileObject, BLOB};
    use windows::Win32::System::Variant::VT_BLOB;

    use super::{
        process_loopback_capability, ActivationHandler, ProcessLoopbackActivationContext,
        PROCESS_LOOPBACK_MINIMUM_BUILD,
    };

    #[test]
    fn capability_matches_the_runtime_windows_build() {
        let capability = process_loopback_capability();
        assert_eq!(
            capability.minimum_windows_build,
            PROCESS_LOOPBACK_MINIMUM_BUILD
        );
        if let Some(build) = capability.windows_build {
            assert_eq!(
                capability.available,
                build >= PROCESS_LOOPBACK_MINIMUM_BUILD
            );
        }
    }

    #[test]
    fn windows_activation_types_match_the_x64_abi() {
        assert_eq!(size_of::<AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS>(), 8);
        assert_eq!(align_of::<AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS>(), 4);
        assert_eq!(
            offset_of!(AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, TargetProcessId),
            0
        );
        assert_eq!(
            offset_of!(AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS, ProcessLoopbackMode),
            4
        );
        assert_eq!(size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>(), 12);
        assert_eq!(align_of::<AUDIOCLIENT_ACTIVATION_PARAMS>(), 4);
        assert_eq!(offset_of!(AUDIOCLIENT_ACTIVATION_PARAMS, ActivationType), 0);
        assert_eq!(offset_of!(AUDIOCLIENT_ACTIVATION_PARAMS, Anonymous), 4);
        assert_eq!(size_of::<BLOB>(), 16);
        assert_eq!(align_of::<BLOB>(), 8);
        assert_eq!(offset_of!(BLOB, cbSize), 0);
        assert_eq!(offset_of!(BLOB, pBlobData), 8);
        assert_eq!(size_of::<PROPVARIANT>(), 24);
        assert_eq!(align_of::<PROPVARIANT>(), 8);
    }

    #[test]
    fn activation_variant_points_to_stable_owned_parameters() {
        let context = ProcessLoopbackActivationContext::new(42);
        let expected =
            context.parameters.as_ref() as *const AUDIOCLIENT_ACTIVATION_PARAMS as *mut u8;
        let inner = unsafe { &*context.variant.Anonymous.Anonymous };
        let blob = unsafe { inner.Anonymous.blob };
        assert_eq!(inner.vt, VT_BLOB);
        assert_eq!(
            blob.cbSize as usize,
            size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>()
        );
        assert_eq!(blob.pBlobData, expected);
        let process = unsafe { context.parameters.Anonymous.ProcessLoopbackParams };
        assert_eq!(process.TargetProcessId, 42);
    }

    #[test]
    fn generated_handler_is_agile_and_com_references_retain_state() {
        let (sender, receiver) = std::sync::mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler = ActivationHandler(sender).into();
        assert!(handler.cast::<IAgileObject>().is_ok());

        let retained = handler.clone();
        drop(handler);
        assert!(matches!(receiver.try_recv(), Err(TryRecvError::Empty)));
        drop(retained);
        assert!(matches!(
            receiver.try_recv(),
            Err(TryRecvError::Disconnected)
        ));
    }
}
