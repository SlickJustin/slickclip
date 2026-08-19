use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use windows::core::Interface;
use windows::Win32::Foundation::S_OK;
use windows::Win32::Media::Audio::{
    eRender, AudioSessionStateActive, IAudioSessionControl2, IAudioSessionManager2,
    DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::CLSCTX_ALL;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};

use super::devices::{create_device_enumerator, device_id};
use super::platform::{take_task_mem_string, ComApartment, OwnedHandle};
use super::process_loopback::process_loopback_capability;
use super::types::{
    ApplicationAudioListResult, ApplicationAudioProcess, AudioError, AudioErrorCode,
};

#[derive(Clone, Debug)]
struct SessionCandidate {
    process_id: u32,
    process_name: String,
    executable_path: Option<String>,
    session_display_name: Option<String>,
    endpoint_id: String,
}

pub fn enumerate_application_audio() -> ApplicationAudioListResult {
    let capability = process_loopback_capability();
    match enumerate_active_sessions() {
        Ok(applications) => ApplicationAudioListResult {
            success: true,
            applications,
            capability,
            error: None,
        },
        Err(error) => ApplicationAudioListResult {
            success: false,
            applications: Vec::new(),
            capability,
            error: Some(error),
        },
    }
}

fn enumerate_active_sessions() -> Result<Vec<ApplicationAudioProcess>, AudioError> {
    let _com = ComApartment::initialize_mta("application audio session enumeration")?;
    let enumerator = create_device_enumerator()?;
    let endpoints = unsafe { enumerator.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) }
        .map_err(|error| session_error("enumerate active render endpoints", error))?;
    let endpoint_count = unsafe { endpoints.GetCount() }
        .map_err(|error| session_error("read the render endpoint count", error))?;
    let mut candidates = Vec::new();

    for endpoint_index in 0..endpoint_count {
        let endpoint = unsafe { endpoints.Item(endpoint_index) }
            .map_err(|error| session_error("open a render endpoint", error))?;
        let endpoint_id = device_id(&endpoint)?;
        let manager: IAudioSessionManager2 = unsafe { endpoint.Activate(CLSCTX_ALL, None) }
            .map_err(|error| session_error("activate IAudioSessionManager2", error))?;
        let sessions = unsafe { manager.GetSessionEnumerator() }
            .map_err(|error| session_error("enumerate render audio sessions", error))?;
        let session_count = unsafe { sessions.GetCount() }
            .map_err(|error| session_error("read the render session count", error))?;

        for session_index in 0..session_count {
            let control = match unsafe { sessions.GetSession(session_index) } {
                Ok(control) => control,
                Err(_) => continue,
            };
            if unsafe { control.GetState() }.ok() != Some(AudioSessionStateActive) {
                continue;
            }
            let control2: IAudioSessionControl2 = match control.cast() {
                Ok(control) => control,
                Err(_) => continue,
            };
            if unsafe { control2.IsSystemSoundsSession() } == S_OK {
                continue;
            }
            let process_id = match unsafe { control2.GetProcessId() } {
                Ok(process_id) if process_id != 0 => process_id,
                _ => continue,
            };
            let metadata = resolve_process_metadata(process_id);
            candidates.push(SessionCandidate {
                process_id,
                process_name: metadata
                    .as_ref()
                    .map(|metadata| metadata.process_name.clone())
                    .unwrap_or_else(|| format!("PID {process_id}")),
                executable_path: metadata.and_then(|metadata| metadata.executable_path),
                session_display_name: session_display_name(&control2),
                endpoint_id: endpoint_id.clone(),
            });
        }
    }

    Ok(deduplicate_sessions(candidates))
}

#[derive(Clone, Debug)]
pub struct ProcessMetadata {
    pub process_name: String,
    pub executable_path: Option<String>,
}

pub fn resolve_process_metadata(process_id: u32) -> Option<ProcessMetadata> {
    let handle =
        unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, process_id) }.ok()?;
    let handle = OwnedHandle(handle);
    let mut buffer = vec![0u16; 32_768];
    let mut length = buffer.len() as u32;
    unsafe {
        QueryFullProcessImageNameW(
            handle.0,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buffer.as_mut_ptr()),
            &mut length,
        )
    }
    .ok()?;
    buffer.truncate(length as usize);
    let path = String::from_utf16(&buffer).ok()?;
    let process_name = Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(&path)
        .to_string();
    Some(ProcessMetadata {
        process_name,
        executable_path: Some(path),
    })
}

fn session_display_name(control: &IAudioSessionControl2) -> Option<String> {
    let value = unsafe { control.GetDisplayName() }.ok()?;
    let result = unsafe { take_task_mem_string(value) }.ok()?;
    (!result.trim().is_empty()).then_some(result)
}

fn deduplicate_sessions(candidates: Vec<SessionCandidate>) -> Vec<ApplicationAudioProcess> {
    struct Aggregate {
        process_name: String,
        executable_path: Option<String>,
        display_names: BTreeSet<String>,
        endpoints: BTreeSet<String>,
        session_count: u32,
    }

    let mut by_process: BTreeMap<u32, Aggregate> = BTreeMap::new();
    for candidate in candidates {
        let entry = by_process
            .entry(candidate.process_id)
            .or_insert_with(|| Aggregate {
                process_name: candidate.process_name.clone(),
                executable_path: candidate.executable_path.clone(),
                display_names: BTreeSet::new(),
                endpoints: BTreeSet::new(),
                session_count: 0,
            });
        entry.session_count += 1;
        entry.endpoints.insert(candidate.endpoint_id);
        if entry.executable_path.is_none() {
            entry.executable_path = candidate.executable_path;
        }
        if let Some(name) = candidate.session_display_name {
            entry.display_names.insert(name);
        }
    }

    let mut applications = by_process
        .into_iter()
        .map(|(process_id, aggregate)| {
            let display_name = aggregate
                .display_names
                .iter()
                .find(|name| !name.trim().is_empty())
                .cloned()
                .unwrap_or_else(|| {
                    aggregate
                        .process_name
                        .strip_suffix(".exe")
                        .unwrap_or(&aggregate.process_name)
                        .to_string()
                });
            ApplicationAudioProcess {
                process_id,
                display_name,
                process_name: aggregate.process_name,
                executable_path: aggregate.executable_path,
                session_display_names: aggregate.display_names.into_iter().collect(),
                session_count: aggregate.session_count,
                render_endpoint_count: aggregate.endpoints.len() as u32,
                session_state: "active".to_string(),
            }
        })
        .collect::<Vec<_>>();
    applications.sort_by(|left, right| {
        left.display_name
            .to_ascii_lowercase()
            .cmp(&right.display_name.to_ascii_lowercase())
            .then_with(|| left.process_id.cmp(&right.process_id))
    });
    applications
}

fn session_error(context: &str, error: windows::core::Error) -> AudioError {
    AudioError::new(
        AudioErrorCode::AudioServiceUnavailable,
        format!("Could not {context}: {error}"),
    )
}

#[cfg(test)]
mod tests {
    use super::{deduplicate_sessions, SessionCandidate};

    #[test]
    fn deduplicates_multiple_sessions_without_losing_pid_or_counts() {
        let applications = deduplicate_sessions(vec![
            SessionCandidate {
                process_id: 42,
                process_name: "Discord.exe".to_string(),
                executable_path: Some("C:\\Discord\\Discord.exe".to_string()),
                session_display_name: Some("Discord".to_string()),
                endpoint_id: "speakers".to_string(),
            },
            SessionCandidate {
                process_id: 42,
                process_name: "Discord.exe".to_string(),
                executable_path: Some("C:\\Discord\\Discord.exe".to_string()),
                session_display_name: Some("Voice".to_string()),
                endpoint_id: "headset".to_string(),
            },
        ]);

        assert_eq!(applications.len(), 1);
        assert_eq!(applications[0].process_id, 42);
        assert_eq!(applications[0].session_count, 2);
        assert_eq!(applications[0].render_endpoint_count, 2);
        assert_eq!(applications[0].session_display_names, ["Discord", "Voice"]);
    }
}
