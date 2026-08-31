mod audio;
mod buffer;
mod ffmpeg_capture;
pub(crate) mod segment;
mod state;
mod timeline;

use tauri::{AppHandle, Emitter, State};

#[cfg(test)]
pub use audio::CompletedAudioSegment;
pub use audio::{
    AudioReplayConfiguration, AudioSaveBarrierTelemetry, AudioSnapshotPinGuard, AudioSnapshotPlan,
    AudioSnapshotTrack, AudioSourceKind, AudioTrackConfiguration, AudioTrackRole, AudioTrackState,
};
pub(crate) use audio::{AudioReplaySession, AudioReplayShared, ReplaySessionClock};
pub use buffer::{
    ReplayBufferManager, ReplayBufferStartRequest, ReplayQuality, ReplaySaveSnapshot,
};
pub use segment::CompletedSegment;
pub use state::{ReplayBufferStatus, ReplayCommandResult, ReplayLifecycleState};
pub use timeline::SavedReplayTimeline;

#[tauri::command]
pub async fn start_replay_buffer(
    app: AppHandle,
    manager: State<'_, ReplayBufferManager>,
    watch_party: State<'_, crate::watch_party::WatchPartyManager>,
    game_detection: State<'_, crate::game_detection::GameDetectionManager>,
    request: ReplayBufferStartRequest,
) -> Result<ReplayCommandResult, String> {
    if watch_party.status().state.active() {
        return Ok(ReplayCommandResult::failure(
            manager.status(),
            "Stop Watch Party before starting the separate Replay Buffer.",
        ));
    }
    let manual_target_id = request.target.id.clone();
    let result = manager.start(request);
    if result.started_new_session {
        game_detection.set_manual_override(Some(manual_target_id));
    }
    let _ = app.emit("replay-buffer-status-changed", result.status.clone());
    Ok(result)
}

#[tauri::command]
pub async fn stop_replay_buffer(
    app: AppHandle,
    manager: State<'_, ReplayBufferManager>,
    game_detection: State<'_, crate::game_detection::GameDetectionManager>,
) -> Result<ReplayCommandResult, String> {
    let manager = manager.inner().clone();
    let result = tauri::async_runtime::spawn_blocking(move || manager.stop_and_wait())
        .await
        .map_err(|error| format!("Replay buffer stop task failed: {error}"))?;
    if result.success {
        game_detection.note_manual_session_stopped();
    }
    let _ = app.emit("replay-buffer-status-changed", result.status.clone());
    Ok(result)
}

#[tauri::command]
pub fn get_replay_buffer_status(manager: State<'_, ReplayBufferManager>) -> ReplayBufferStatus {
    manager.status()
}
