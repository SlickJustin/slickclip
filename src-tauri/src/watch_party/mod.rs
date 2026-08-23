mod checkpoint;
mod compositor;
mod layout;
mod participants;
mod recording;

use tauri::State;

pub use recording::{
    WatchPartyCommandResult, WatchPartyManager, WatchPartyStartRequest, WatchPartyStatus,
};

#[tauri::command]
pub fn start_watch_party(
    manager: State<'_, WatchPartyManager>,
    replay: State<'_, crate::replay::ReplayBufferManager>,
    request: WatchPartyStartRequest,
) -> WatchPartyCommandResult {
    if replay.status().state.is_active() {
        return WatchPartyCommandResult::rejected(
            manager.status(),
            "Stop the Replay Buffer before starting the separate Watch Party recorder.",
        );
    }
    manager.start(request)
}

#[tauri::command]
pub fn stop_watch_party(manager: State<'_, WatchPartyManager>) -> WatchPartyCommandResult {
    manager.stop()
}

#[tauri::command]
pub fn get_watch_party_status(manager: State<'_, WatchPartyManager>) -> WatchPartyStatus {
    manager.status()
}

#[tauri::command]
pub async fn recover_watch_party(
    manager: State<'_, WatchPartyManager>,
    session_id: String,
) -> Result<WatchPartyCommandResult, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.recover(&session_id))
        .await
        .map_err(|error| format!("Watch Party recovery task failed: {error}"))
}
