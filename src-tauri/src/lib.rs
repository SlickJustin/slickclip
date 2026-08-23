mod audio;
mod capture;
mod clips;
mod desktop;
mod hotkey;
mod library;
mod preferences;
mod replay;

use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::Duration,
};
use tauri::Manager;

#[derive(Clone, Default)]
struct StartupCoordinator {
    revealed: Arc<AtomicBool>,
    background_launch: bool,
}

impl StartupCoordinator {
    fn new(background_launch: bool) -> Self {
        Self {
            revealed: Arc::new(AtomicBool::new(false)),
            background_launch,
        }
    }

    fn reveal(&self, app: &tauri::AppHandle) -> Result<(), String> {
        if self.revealed.swap(true, Ordering::SeqCst) {
            return Ok(());
        }

        let result = (|| {
            if !self.background_launch {
                desktop::show_main_window(app)?;
            }
            if let Some(splash) = app.get_webview_window("splash") {
                splash.close().map_err(|error| error.to_string())?;
            }
            Ok(())
        })();

        if result.is_err() {
            self.revealed.store(false, Ordering::SeqCst);
        }
        result
    }
}

// Learn more about Tauri commands at https://tauri.app/develop/calling-rust/
#[tauri::command]
fn greet(name: &str) -> String {
    format!("Hello, {}! You've been greeted from Rust!", name)
}

#[tauri::command]
fn complete_startup(
    app: tauri::AppHandle,
    startup: tauri::State<'_, StartupCoordinator>,
) -> Result<(), String> {
    startup.reveal(&app)
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
            let background_launch = std::env::args().any(|argument| argument == "--background");
            if background_launch {
                if let Some(splash) = app.get_webview_window("splash") {
                    let _ = splash.hide();
                }
            }
            let app_data = app.path().app_local_data_dir()?;
            let replay_root = app_data.join("ReplayBuffer");
            let replay_manager =
                replay::ReplayBufferManager::new(replay_root).map_err(std::io::Error::other)?;
            let clips_directory = app.path().video_dir()?.join("JustIn Replay").join("Clips");
            let library_database = app_data.join("Library").join("clips.db");
            let clip_library =
                library::ClipLibraryManager::initialize(library_database, clips_directory.clone());
            clip_library.start_initial_reconciliation(app.handle().clone());
            app.manage(library::EditorExportManager::new(
                clip_library.clone(),
                clips_directory.clone(),
                app.handle().clone(),
            ));
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
            app.manage(preferences::UiPreferencesManager::initialize(
                app_data.join("Preferences").join("ui-preferences.json"),
            ));
            app.manage(hotkey::SaveReplayHotkeyManager::new());
            app.state::<hotkey::SaveReplayHotkeyManager>()
                .register_initial(app.handle());
            desktop::setup(app)?;
            let startup = StartupCoordinator::new(background_launch);
            let fallback = startup.clone();
            let fallback_app = app.handle().clone();
            std::thread::Builder::new()
                .name("slickclip-startup-fallback".to_string())
                .spawn(move || {
                    std::thread::sleep(Duration::from_secs(8));
                    let _ = fallback.reveal(&fallback_app);
                })?;
            app.manage(startup);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            greet,
            complete_startup,
            desktop::set_start_with_windows,
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
            library::clipboard::copy_clip_to_clipboard,
            library::get_clip_playback_info,
            library::request_clip_thumbnail,
            library::prepare_clip_preview,
            library::prepare_clip_audio_preview,
            library::prepare_editor_audio_preview,
            library::export::start_editor_export,
            library::export::cancel_editor_export,
            library::export::get_editor_export_status,
            library::list_collections_command,
            library::create_collection_command,
            library::rename_collection_command,
            library::delete_collection_command,
            library::set_clip_collection_membership,
            library::record_clip_watch_command,
            hotkey::get_save_replay_hotkey,
            hotkey::set_save_replay_hotkey,
            hotkey::set_hotkey_recorder_active,
            hotkey::begin_hotkey_test,
            hotkey::cancel_hotkey_test,
            preferences::get_ui_preferences,
            preferences::update_ui_preferences,
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
        if let tauri::RunEvent::WindowEvent { label, event, .. } = &event {
            if label == "main" {
                match event {
                    tauri::WindowEvent::CloseRequested { api, .. } => {
                        if desktop::should_background(app_handle) {
                            api.prevent_close();
                            if let Some(window) = app_handle.get_webview_window("main") {
                                let _ = window.hide();
                            }
                        } else {
                            if let Some(integration) =
                                app_handle.try_state::<desktop::DesktopIntegration>()
                            {
                                integration.begin_exit();
                            }
                            app_handle.exit(0);
                        }
                    }
                    tauri::WindowEvent::Resized(_) if desktop::should_background(app_handle) => {
                        if let Some(window) = app_handle.get_webview_window("main") {
                            if window.is_minimized().unwrap_or(false) {
                                let _ = window.hide();
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if matches!(
            event,
            tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
        ) {
            if let Some(integration) = app_handle.try_state::<desktop::DesktopIntegration>() {
                integration.begin_exit();
            }
            app_handle
                .state::<audio::AudioCaptureTestManager>()
                .shutdown_and_wait();
            app_handle
                .state::<hotkey::SaveReplayHotkeyManager>()
                .unregister(app_handle);
            app_handle
                .state::<library::EditorExportManager>()
                .shutdown_and_wait();
            app_handle
                .state::<clips::ClipSaveManager>()
                .shutdown_and_wait();
            app_handle
                .state::<replay::ReplayBufferManager>()
                .shutdown_and_cleanup();
        }
    });
}
