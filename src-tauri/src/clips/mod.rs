mod assembler;
mod ffmpeg;
mod save;

use tauri::State;

pub use save::{ClipSaveManager, SaveJobState, SaveReplayCommandResult, SaveReplayStatus};

#[tauri::command]
pub fn save_replay(manager: State<'_, ClipSaveManager>) -> SaveReplayCommandResult {
    manager.start()
}

#[tauri::command]
pub fn get_save_replay_status(manager: State<'_, ClipSaveManager>) -> SaveReplayStatus {
    manager.status()
}
