mod capture;
mod clips;
mod hotkey;
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
            app.manage(clips::ClipSaveManager::new(
                replay_manager.clone(),
                clips_directory,
            ));
            app.manage(replay_manager);
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
            hotkey::get_save_replay_hotkey,
            hotkey::set_save_replay_hotkey,
            hotkey::set_hotkey_recorder_active
        ])
        .build(tauri::generate_context!())
        .expect("error while building Tauri application");

    app.run(|app_handle, event| {
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
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
