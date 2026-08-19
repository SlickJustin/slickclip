mod audio;
mod buffer;
pub(crate) mod segment;
mod state;
mod timeline;

use tauri::State;

pub use audio::{AudioSaveBarrierTelemetry, AudioSnapshotPlan};
pub use buffer::{ReplayBufferManager, ReplayBufferStartRequest, ReplaySaveSnapshot};
pub use segment::CompletedSegment;
pub use state::{ReplayBufferStatus, ReplayCommandResult, ReplayLifecycleState};
pub use timeline::SavedReplayTimeline;

#[tauri::command]
pub async fn start_replay_buffer(
    manager: State<'_, ReplayBufferManager>,
    request: ReplayBufferStartRequest,
) -> Result<ReplayCommandResult, String> {
    Ok(manager.start(request))
}

#[tauri::command]
pub async fn stop_replay_buffer(
    manager: State<'_, ReplayBufferManager>,
) -> Result<ReplayCommandResult, String> {
    let manager = manager.inner().clone();
    tauri::async_runtime::spawn_blocking(move || manager.stop_and_wait())
        .await
        .map_err(|error| format!("Replay buffer stop task failed: {error}"))
}

#[tauri::command]
pub fn get_replay_buffer_status(manager: State<'_, ReplayBufferManager>) -> ReplayBufferStatus {
    manager.status()
}
