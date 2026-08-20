mod audio;
mod capture;
mod clips;
mod hotkey;
mod library;
mod replay;

use tauri::Manager;

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|app, shortcut, event| {
                    hotkey::handle_global_shortcut(app, shortcut, event.state());
                })
                .build(),
        )
        .setup(|app| {
            let replay_root = app.path().app_local_data_dir()?.join("ReplayBuffer");
            let replay_manager =
                replay::ReplayBufferManager::new(replay_root).map_err(std::io::Error::other)?;
            let clips_directory = app.path().video_dir()?.join("JustIn Replay").join("Clips");
            let library_database = app
                .path()
                .app_local_data_dir()?
                .join("Library")
                .join("clips.db");
            let clip_library =
                library::ClipLibraryManager::initialize(library_database, clips_directory.clone());
            clip_library.start_initial_reconciliation(app.handle().clone());
            app.manage(clips::ClipSaveManager::new(
                replay_manager.clone(),
                clips_directory,
                clip_library.clone(),
                app.handle().clone(),
            ));
            app.manage(clip_library);
            app.manage(replay_manager);
            let audio_tests_directory = app
                .path()
                .video_dir()?
                .join("JustIn Replay")
                .join("DevTests")
                .join("Audio");
            app.manage(audio::AudioCaptureTestManager::new(audio_tests_directory));
            app.manage(hotkey::SaveReplayHotkeyManager::new());
            app.state::<hotkey::SaveReplayHotkeyManager>()
                .register_initial(app.handle());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            capture::capture_test::run_capture_test,
            capture::continuous_baseline::run_continuous_baseline,
            capture::encoder::get_encoder_capabilities,
            capture::targets::list_capture_monitors,
            capture::targets::list_capture_windows,
            replay::start_replay_buffer,
            replay::stop_replay_buffer,
            replay::get_replay_buffer_status,
            clips::save_replay,
            clips::get_save_replay_status,
            library::list_clips,
            library::refresh_clip_library,
            library::set_clip_favorite,
            library::rename_clip_display_name,
            library::open_clip_file,
            library::open_clip_folder,
            library::delete_clip,
            hotkey::get_save_replay_hotkey,
            hotkey::set_save_replay_hotkey,
            hotkey::set_hotkey_recorder_active,
            audio::list_audio_microphones,
            audio::list_application_audio_processes,
            audio::get_process_loopback_capability,
            audio::probe_process_audio_activation,
            audio::start_microphone_audio_test,
            audio::start_process_audio_test,
            audio::get_audio_capture_test_status
        ])
        .build(tauri::generate_context!())
        .expect("error while building Tauri application");

    app.run(|app_handle, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            app_handle
                .state::<audio::AudioCaptureTestManager>()
                .shutdown_and_wait();
            app_handle
                .state::<hotkey::SaveReplayHotkeyManager>()
                .unregister(app_handle);
            app_handle
                .state::<clips::ClipSaveManager>()
                .shutdown_and_wait();
            app_handle
                .state::<replay::ReplayBufferManager>()
                .shutdown_and_cleanup();
        }
    });
}
