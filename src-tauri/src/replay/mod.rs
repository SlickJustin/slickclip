mod audio;
mod buffer;
pub(crate) mod segment;
mod state;
mod timeline;

use tauri::State;

#[cfg(test)]
pub use audio::CompletedAudioSegment;
pub use audio::{
    AudioReplayConfiguration, AudioSaveBarrierTelemetry, AudioSnapshotPinGuard, AudioSnapshotPlan,
    AudioSnapshotTrack, AudioSourceKind, AudioTrackConfiguration, AudioTrackRole, AudioTrackState,
};
pub(crate) use audio::{AudioReplaySession, AudioReplayShared, ReplaySessionClock};
pub use buffer::{ReplayBufferManager, ReplayBufferStartRequest, ReplaySaveSnapshot};
pub use segment::CompletedSegment;
pub use state::{ReplayBufferStatus, ReplayCommandResult, ReplayLifecycleState};
pub use timeline::SavedReplayTimeline;

#[tauri::command]
pub async fn start_replay_buffer(
    manager: State<'_, ReplayBufferManager>,
    watch_party: State<'_, crate::watch_party::WatchPartyManager>,
    request: ReplayBufferStartRequest,
) -> Result<ReplayCommandResult, String> {
    if watch_party.status().state.active() {
        return Ok(ReplayCommandResult::failure(
            manager.status(),
            "Stop Watch Party before starting the separate Replay Buffer.",
        ));
    }
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
