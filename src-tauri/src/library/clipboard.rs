use std::mem::size_of;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use tauri::State;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
use windows::Win32::System::Ole::CF_HDROP;
use windows::Win32::UI::Shell::DROPFILES;

use super::{ClipActionResponse, ClipIdRequest, ClipLibraryManager};

struct ClipboardGuard;

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseClipboard();
        }
    }
}

struct OwnedGlobalMemory {
    handle: HGLOBAL,
    transferred: bool,
}

impl Drop for OwnedGlobalMemory {
    fn drop(&mut self) {
        if !self.transferred {
            unsafe {
                let _ = GlobalFree(Some(self.handle));
            }
        }
    }
}

fn dropfiles_payload(path: &Path) -> Result<Vec<u8>, String> {
    let mut wide_path = path.as_os_str().encode_wide().collect::<Vec<_>>();
    if wide_path.contains(&0) {
        return Err("The clip path contains an invalid NUL character.".to_string());
    }
    wide_path.extend([0, 0]);
    let header = DROPFILES {
        pFiles: u32::try_from(size_of::<DROPFILES>())
            .map_err(|_| "The clipboard header is too large.".to_string())?,
        fWide: true.into(),
        ..Default::default()
    };
    let payload_size = size_of::<DROPFILES>()
        .checked_add(
            wide_path
                .len()
                .checked_mul(size_of::<u16>())
                .ok_or_else(|| "The clipboard payload is too large.".to_string())?,
        )
        .ok_or_else(|| "The clipboard payload is too large.".to_string())?;
    let mut payload = vec![0_u8; payload_size];
    unsafe {
        std::ptr::copy_nonoverlapping(
            (&header as *const DROPFILES).cast::<u8>(),
            payload.as_mut_ptr(),
            size_of::<DROPFILES>(),
        );
        std::ptr::copy_nonoverlapping(
            wide_path.as_ptr().cast::<u8>(),
            payload.as_mut_ptr().add(size_of::<DROPFILES>()),
            wide_path.len() * size_of::<u16>(),
        );
    }
    Ok(payload)
}

fn copy_file_to_clipboard(path: &Path) -> Result<(), String> {
    let payload = dropfiles_payload(path)?;
    unsafe {
        OpenClipboard(None).map_err(|error| {
            format!("The Windows clipboard is busy. Close the app using it and try again: {error}")
        })?;
    }
    let _clipboard = ClipboardGuard;
    unsafe {
        EmptyClipboard()
            .map_err(|error| format!("Could not clear the Windows clipboard: {error}"))?;
    }
    let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, payload.len()) }
        .map_err(|error| format!("Could not allocate clipboard memory: {error}"))?;
    let mut memory = OwnedGlobalMemory {
        handle,
        transferred: false,
    };
    let destination = unsafe { GlobalLock(handle) };
    if destination.is_null() {
        return Err("Could not lock the allocated clipboard memory.".to_string());
    }
    unsafe {
        std::ptr::copy_nonoverlapping(payload.as_ptr(), destination.cast::<u8>(), payload.len());
        let _ = GlobalUnlock(handle);
        SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(handle.0)))
            .map_err(|error| format!("Windows rejected the copied clip file: {error}"))?;
    }
    memory.transferred = true;
    Ok(())
}

fn validate_clipboard_source(path: PathBuf) -> Result<PathBuf, String> {
    if !path.is_file() {
        return Err("The source clip is missing or is not a regular file.".to_string());
    }
    if !path
        .extension()
        .and_then(|value| value.to_str())
        .is_some_and(|value| value.eq_ignore_ascii_case("mp4"))
    {
        return Err("Only permanent MP4 clips can be copied.".to_string());
    }
    Ok(path)
}

#[tauri::command]
pub async fn copy_clip_to_clipboard(
    manager: State<'_, ClipLibraryManager>,
    request: ClipIdRequest,
) -> Result<ClipActionResponse, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = manager
            .resolved_clip(&request.clip_id)
            .map(|(_, path)| path)
            .and_then(validate_clipboard_source)
            .and_then(|path| copy_file_to_clipboard(&path));
        match result {
            Ok(()) => ClipActionResponse {
                success: true,
                error_message: None,
            },
            Err(error) => ClipActionResponse {
                success: false,
                error_message: Some(error),
            },
        }
    })
    .await
    .map_err(|error| format!("The clipboard worker failed: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::{ClipLibraryManager, SavedClipMetadata};
    use uuid::Uuid;

    #[test]
    fn dropfiles_payload_is_wide_and_double_nul_terminated() {
        let payload = dropfiles_payload(Path::new(r"C:\Clips\Unicode ☃.mp4")).unwrap();
        assert_eq!(
            u32::from_le_bytes(payload[0..4].try_into().unwrap()) as usize,
            size_of::<DROPFILES>()
        );
        let wide_flag_offset = size_of::<DROPFILES>() - size_of::<i32>();
        assert_eq!(
            i32::from_le_bytes(
                payload[wide_flag_offset..wide_flag_offset + 4]
                    .try_into()
                    .unwrap()
            ),
            1
        );
        assert_eq!(&payload[payload.len() - 4..], &[0, 0, 0, 0]);
        let utf16 = payload[size_of::<DROPFILES>()..]
            .chunks_exact(2)
            .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
            .collect::<Vec<_>>();
        assert!(String::from_utf16_lossy(&utf16).contains("Unicode ☃.mp4"));
    }

    #[test]
    fn clipboard_source_requires_an_existing_mp4() {
        let missing = std::env::temp_dir().join("slickclip-stage18-missing.mp4");
        assert!(validate_clipboard_source(missing).is_err());
    }

    #[test]
    fn clipboard_clip_id_resolution_enforces_owned_existing_source() {
        let root = std::env::temp_dir().join(format!("slickclip-clipboard-{}", Uuid::new_v4()));
        let clips = root.join("Clips");
        std::fs::create_dir_all(&clips).unwrap();
        let source = clips.join("雪 clip.mp4");
        std::fs::write(&source, b"mp4").unwrap();
        let manager =
            ClipLibraryManager::initialize(root.join("Library").join("clips.db"), clips.clone());
        let indexed = manager
            .index_saved_clip(SavedClipMetadata {
                file_path: source.clone(),
                created_at_ms: 1,
                duration_100ns: 10_000_000,
                requested_duration_seconds: 1,
                width: 1920,
                height: 1080,
                fps_numerator: 60,
                fps_denominator: 1,
                video_codec: "h264".into(),
                video_profile: None,
                video_bitrate_bps: None,
                total_bitrate_bps: None,
                capture_target_label: None,
                capture_target_type: None,
                audio_tracks: Vec::new(),
            })
            .unwrap();
        assert_eq!(
            manager.resolved_clip(&indexed.clip_id).unwrap().1,
            source.canonicalize().unwrap()
        );
        assert!(manager.resolved_clip("not-a-clip-id").is_err());
        std::fs::remove_file(&source).unwrap();
        assert!(manager.resolved_clip(&indexed.clip_id).is_err());

        let outside = root.join("outside.mp4");
        std::fs::write(&outside, b"mp4").unwrap();
        let outside_result = manager.index_saved_clip(SavedClipMetadata {
            file_path: outside,
            created_at_ms: 1,
            duration_100ns: 1,
            requested_duration_seconds: 1,
            width: 1,
            height: 1,
            fps_numerator: 1,
            fps_denominator: 1,
            video_codec: "h264".into(),
            video_profile: None,
            video_bitrate_bps: None,
            total_bitrate_bps: None,
            capture_target_label: None,
            capture_target_type: None,
            audio_tracks: Vec::new(),
        });
        assert!(outside_result.is_err());
        std::fs::remove_dir_all(root).unwrap();
    }
}
