use std::collections::{BTreeMap, BTreeSet};
use std::mem::size_of;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W, TH32CS_SNAPPROCESS,
};

use crate::audio::OwnedHandle;
use crate::capture::encoder::EncoderChoice;
use crate::capture::targets::{self, CaptureTargetRequest, CaptureTargetType, WindowTarget};
use crate::preferences::{GameDetectionMode, UiPreferences, UiPreferencesManager};
use crate::replay::{
    AudioReplayConfiguration, AudioSourceKind, AudioTrackConfiguration, AudioTrackRole,
    ReplayBufferManager, ReplayBufferStartRequest, ReplayBufferStatus, ReplayLifecycleState,
    ReplayQuality,
};

const DETECTION_INTERVAL: Duration = Duration::from_secs(2);
const FAILED_START_COOLDOWN: Duration = Duration::from_secs(15);
const REQUIRED_STABLE_POLLS: u8 = 2;

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameCandidate {
    target_id: String,
    title: String,
    process_name: String,
    process_id: u32,
    width: u32,
    height: u32,
    foreground: bool,
    approved: bool,
    reason: String,
    #[serde(skip)]
    confidence_score: u8,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameDetectionStatus {
    success: bool,
    enabled: bool,
    auto_arm_enabled: bool,
    detection_mode: GameDetectionMode,
    stop_replay_on_close: bool,
    ready_notification_enabled: bool,
    candidates: Vec<GameCandidate>,
    auto_armed_target_id: Option<String>,
    replay_ready: bool,
    replay_state: DetectedReplayState,
    pending_target_id: Option<String>,
    manual_override_active: bool,
    last_scan_at_ms: Option<u64>,
    error_message: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum DetectedReplayState {
    Detected,
    Starting,
    ReplayReady,
    CaptureFailed,
    ReplayStopped,
}

impl Default for GameDetectionStatus {
    fn default() -> Self {
        Self {
            success: true,
            enabled: false,
            auto_arm_enabled: false,
            detection_mode: GameDetectionMode::AnyDetectedGame,
            stop_replay_on_close: true,
            ready_notification_enabled: true,
            candidates: Vec::new(),
            auto_armed_target_id: None,
            replay_ready: false,
            replay_state: DetectedReplayState::ReplayStopped,
            pending_target_id: None,
            manual_override_active: false,
            last_scan_at_ms: None,
            error_message: None,
        }
    }
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct AutoArmFeedback {
    success: bool,
    message: String,
    target_label: Option<String>,
}

struct DetectionRuntime {
    status: GameDetectionStatus,
    last_failed_target: Option<(String, Instant)>,
    ready_notified_target: Option<String>,
    pending_target: Option<String>,
    pending_polls: u8,
    manual_override_target: Option<String>,
}

#[derive(Clone, Debug)]
struct ProcessSnapshotEntry {
    parent_process_id: u32,
    process_name: String,
}

#[derive(Clone, Debug, Default)]
struct ProcessTree {
    entries: BTreeMap<u32, ProcessSnapshotEntry>,
}

impl ProcessTree {
    fn capture() -> Self {
        let Ok(snapshot) = (unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
            return Self::default();
        };
        let snapshot = OwnedHandle(snapshot);
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        if unsafe { Process32FirstW(snapshot.0, &mut entry) }.is_err() {
            return Self::default();
        }

        let mut entries = BTreeMap::new();
        loop {
            let process_name = String::from_utf16_lossy(
                &entry
                    .szExeFile
                    .iter()
                    .take_while(|value| **value != 0)
                    .copied()
                    .collect::<Vec<_>>(),
            );
            entries.insert(
                entry.th32ProcessID,
                ProcessSnapshotEntry {
                    parent_process_id: entry.th32ParentProcessID,
                    process_name,
                },
            );
            if unsafe { Process32NextW(snapshot.0, &mut entry) }.is_err() {
                break;
            }
        }
        Self { entries }
    }

    fn process_name(&self, process_id: u32) -> Option<&str> {
        self.entries
            .get(&process_id)
            .map(|entry| entry.process_name.as_str())
            .filter(|name| !name.trim().is_empty())
    }

    fn launcher_ancestor(&self, process_id: u32) -> Option<&'static str> {
        let mut current = process_id;
        let mut visited = BTreeSet::new();
        for _ in 0..6 {
            if !visited.insert(current) {
                return None;
            }
            let parent_process_id = self.entries.get(&current)?.parent_process_id;
            if parent_process_id == 0 || parent_process_id == current {
                return None;
            }
            let parent = self.entries.get(&parent_process_id)?;
            if let Some(launcher) = recognized_launcher(&normalize_process(&parent.process_name)) {
                return Some(launcher);
            }
            current = parent_process_id;
        }
        None
    }
}

impl Default for DetectionRuntime {
    fn default() -> Self {
        Self {
            status: GameDetectionStatus::default(),
            last_failed_target: None,
            ready_notified_target: None,
            pending_target: None,
            pending_polls: 0,
            manual_override_target: None,
        }
    }
}

pub struct GameDetectionManager {
    stop: Arc<AtomicBool>,
    runtime: Arc<Mutex<DetectionRuntime>>,
    worker: Mutex<Option<JoinHandle<()>>>,
}

impl GameDetectionManager {
    pub fn new() -> Self {
        Self {
            stop: Arc::new(AtomicBool::new(false)),
            runtime: Arc::new(Mutex::new(DetectionRuntime::default())),
            worker: Mutex::new(None),
        }
    }

    pub fn start(&self, app: AppHandle) -> Result<(), String> {
        let stop = Arc::clone(&self.stop);
        let runtime = Arc::clone(&self.runtime);
        let worker = thread::Builder::new()
            .name("slickclip-game-detection".to_string())
            .spawn(move || detection_loop(app, stop, runtime))
            .map_err(|error| format!("Could not start game detection: {error}"))?;
        *self.lock_worker() = Some(worker);
        Ok(())
    }

    pub fn status(&self) -> GameDetectionStatus {
        self.lock_runtime().status.clone()
    }

    pub fn status_with_replay(&self, replay: &ReplayBufferStatus) -> GameDetectionStatus {
        let state = self.lock_runtime();
        let mut status = state.status.clone();
        status.replay_ready = status
            .auto_armed_target_id
            .as_ref()
            .is_some_and(|_| is_authoritative_replay_ready(replay));
        status.replay_state = detected_replay_state(
            &status,
            replay,
            state
                .last_failed_target
                .as_ref()
                .map(|(target, _)| target.as_str()),
        );
        status
    }

    pub fn set_manual_override(&self, target_id: Option<String>) {
        let mut runtime = self.lock_runtime();
        runtime.manual_override_target = target_id;
        runtime.pending_target = None;
        runtime.pending_polls = 0;
        runtime.status.manual_override_active = runtime.manual_override_target.is_some();
        runtime.status.pending_target_id = None;
    }

    pub fn note_manual_session_stopped(&self) {
        self.set_manual_override(None);
    }

    pub fn shutdown_and_wait(&self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(worker) = self.lock_worker().take() {
            let _ = worker.join();
        }
    }

    fn lock_runtime(&self) -> MutexGuard<'_, DetectionRuntime> {
        self.runtime
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn lock_worker(&self) -> MutexGuard<'_, Option<JoinHandle<()>>> {
        self.worker
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

fn detection_loop(app: AppHandle, stop: Arc<AtomicBool>, runtime: Arc<Mutex<DetectionRuntime>>) {
    while !stop.load(Ordering::SeqCst) {
        scan_and_auto_arm(&app, &runtime);
        let slices = DETECTION_INTERVAL.as_millis() / 100;
        for _ in 0..slices {
            if stop.load(Ordering::SeqCst) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

fn scan_and_auto_arm(app: &AppHandle, runtime: &Arc<Mutex<DetectionRuntime>>) {
    let Some(preference_manager) = app.try_state::<UiPreferencesManager>() else {
        return;
    };
    let preferences = preference_manager.get().preferences;
    let Some(replay) = app.try_state::<ReplayBufferManager>() else {
        return;
    };

    if !preferences.game_detection_enabled {
        stop_tracked_buffer_if_needed(&replay, runtime, "Game detection was disabled.");
        let mut state = lock_runtime(runtime);
        state.pending_target = None;
        state.pending_polls = 0;
        state.status = GameDetectionStatus {
            detection_mode: preferences.game_detection_mode,
            stop_replay_on_close: preferences.game_stop_replay_on_close,
            ready_notification_enabled: preferences.game_ready_notification_enabled,
            manual_override_active: state.manual_override_target.is_some(),
            ..GameDetectionStatus::default()
        };
        return;
    }

    let windows = match targets::enumerate_windows() {
        Ok(windows) => windows,
        Err(error) => {
            let mut state = lock_runtime(runtime);
            state.status.success = false;
            state.status.enabled = true;
            state.status.auto_arm_enabled = preferences.game_auto_arm;
            state.status.detection_mode = preferences.game_detection_mode;
            state.status.stop_replay_on_close = preferences.game_stop_replay_on_close;
            state.status.ready_notification_enabled = preferences.game_ready_notification_enabled;
            state.status.last_scan_at_ms = Some(now_ms());
            state.status.error_message = Some(error);
            return;
        }
    };
    let candidates = classify_windows(&windows, &preferences);
    let live_target_ids = windows
        .iter()
        .map(|window| window.id.as_str())
        .collect::<BTreeSet<_>>();
    let mut replay_status = replay.status();
    let mut tracked_target = lock_runtime(runtime).status.auto_armed_target_id.clone();

    {
        let mut state = lock_runtime(runtime);
        let manual_window_disappeared =
            state
                .manual_override_target
                .as_deref()
                .is_some_and(|target| {
                    target.starts_with("window:") && !live_target_ids.contains(target)
                });
        if manual_window_disappeared && !replay_status.state.is_active() {
            state.manual_override_target = None;
        }
    }

    if !preferences.game_auto_arm && tracked_target.is_some() {
        stop_tracked_buffer_if_needed(&replay, runtime, "Game auto-start was disabled.");
        tracked_target = None;
        replay_status = replay.status();
    }

    if tracked_target.as_ref().is_some_and(|target| {
        live_target_ids.contains(target.as_str())
            && !candidate_is_eligible(&preferences, &candidates, target)
    }) {
        stop_tracked_buffer_if_needed(
            &replay,
            runtime,
            "The game was excluded or is no longer eligible in the selected detection mode.",
        );
        tracked_target = None;
        replay_status = replay.status();
    } else if tracked_target
        .as_ref()
        .is_some_and(|target| closed_target_requires_stop(&preferences, target, &live_target_ids))
        && replay_status.capture_health != "Recovering"
    {
        stop_tracked_buffer_if_needed(&replay, runtime, "The detected game window closed.");
        tracked_target = None;
        replay_status = replay.status();
    } else if tracked_target.is_some()
        && matches!(
            replay_status.state,
            ReplayLifecycleState::Stopped | ReplayLifecycleState::Error
        )
    {
        let mut state = lock_runtime(runtime);
        if replay_status.state == ReplayLifecycleState::Error {
            let target = tracked_target.clone().unwrap_or_default();
            state.last_failed_target = Some((target, Instant::now()));
            let message = replay_status.error_message.clone().unwrap_or_else(|| {
                "The automatically detected game capture stopped with an error.".to_string()
            });
            let _ = app.emit(
                "game-auto-arm-feedback",
                AutoArmFeedback {
                    success: false,
                    message,
                    target_label: replay_status.target_label.clone(),
                },
            );
        }
        state.ready_notified_target = None;
        state.status.auto_armed_target_id = None;
        tracked_target = None;
    }

    if let Some(target_id) = tracked_target.as_ref() {
        let should_mark_ready = mark_ready_transition(
            runtime,
            target_id,
            is_authoritative_replay_ready(&replay_status),
            candidate_is_eligible(&preferences, &candidates, target_id),
        );
        if should_mark_ready {
            if preferences.game_ready_notification_enabled {
                let target_label = candidates
                    .iter()
                    .find(|candidate| &candidate.target_id == target_id)
                    .map(|candidate| candidate.title.clone())
                    .or_else(|| replay_status.target_label.clone());
                let message = target_label
                    .as_ref()
                    .map(|label| format!("Replay Ready for {label}."))
                    .unwrap_or_else(|| "Replay Ready for your game.".to_string());
                let _ = app.emit(
                    "game-auto-arm-feedback",
                    AutoArmFeedback {
                        success: true,
                        message,
                        target_label: target_label.clone(),
                    },
                );
                crate::desktop::show_notification_overlay(
                    app,
                    "Replay Ready",
                    target_label.as_deref().unwrap_or("Game capture is active"),
                    None,
                );
                crate::desktop::refresh_tray_status(app);
            }
        }
    }

    let manual_override_active = lock_runtime(runtime).manual_override_target.is_some();
    let preferred_pending_target = lock_runtime(runtime).pending_target.clone();
    let selected = if tracked_target.is_none()
        && !manual_override_active
        && matches!(
            replay_status.state,
            ReplayLifecycleState::Stopped | ReplayLifecycleState::Error
        ) {
        select_automatic_candidate(
            &preferences,
            &candidates,
            preferred_pending_target.as_deref(),
        )
    } else {
        None
    };
    let stabilized_target = observe_pending_candidate(
        runtime,
        selected.map(|candidate| candidate.target_id.as_str()),
    );

    {
        let mut state = lock_runtime(runtime);
        let auto_armed_target_id = state.status.auto_armed_target_id.clone();
        let replay_ready = auto_armed_target_id
            .as_ref()
            .is_some_and(|_| is_authoritative_replay_ready(&replay_status));
        let mut status = GameDetectionStatus {
            success: true,
            enabled: true,
            auto_arm_enabled: preferences.game_auto_arm,
            detection_mode: preferences.game_detection_mode,
            stop_replay_on_close: preferences.game_stop_replay_on_close,
            ready_notification_enabled: preferences.game_ready_notification_enabled,
            candidates: candidates.clone(),
            auto_armed_target_id,
            replay_ready,
            replay_state: DetectedReplayState::ReplayStopped,
            pending_target_id: state.pending_target.clone(),
            manual_override_active: state.manual_override_target.is_some(),
            last_scan_at_ms: Some(now_ms()),
            error_message: None,
        };
        status.replay_state = detected_replay_state(
            &status,
            &replay_status,
            state
                .last_failed_target
                .as_ref()
                .map(|(target, _)| target.as_str()),
        );
        state.status = status;
    }

    let Some(stabilized_target) = stabilized_target else {
        return;
    };

    // This is the one authoritative automatic-start gate. The preference lock remains held
    // through ReplayBufferManager::start so an update that completes with auto-start OFF can
    // never be followed by a start based on an older detector snapshot.
    let automatic_start = preference_manager.with_current(|current| {
        let candidate = automatic_start_candidate_for_target(
            current,
            replay.status().state,
            &candidates,
            &stabilized_target,
            lock_runtime(runtime).manual_override_target.is_some(),
        )?;
        let retry_blocked = lock_runtime(runtime)
            .last_failed_target
            .as_ref()
            .is_some_and(|(target, attempted)| {
                target == &candidate.target_id && attempted.elapsed() < FAILED_START_COOLDOWN
            });
        if retry_blocked {
            return None;
        }

        let request = ReplayBufferStartRequest {
            target: CaptureTargetRequest {
                target_type: CaptureTargetType::Window,
                id: candidate.target_id.clone(),
            },
            capture_mode: current.capture_mode,
            encoder: match current.replay_encoder.as_str() {
                "hevc" => EncoderChoice::Hevc,
                "h264" => EncoderChoice::H264,
                _ => EncoderChoice::Automatic,
            },
            replay_duration_seconds: current.replay_duration_seconds,
            frame_rate: current.replay_frame_rate,
            quality: match current.replay_quality.as_str() {
                "high" => ReplayQuality::High,
                "smallerFiles" => ReplayQuality::SmallerFiles,
                _ => ReplayQuality::Balanced,
            },
            audio: AudioReplayConfiguration {
                tracks: vec![AudioTrackConfiguration {
                    role: AudioTrackRole::Game,
                    enabled: true,
                    source_kind: AudioSourceKind::Process,
                    process_id: Some(candidate.process_id),
                    endpoint_id: None,
                    source_label: Some(candidate.process_name.clone()),
                }],
            },
        };
        Some((candidate.clone(), replay.start(request)))
    });
    let Some((candidate, result)) = automatic_start else {
        return;
    };
    if result.started_new_session {
        let _ = app.emit("replay-buffer-status-changed", result.status.clone());
        let mut state = lock_runtime(runtime);
        state.status.auto_armed_target_id = Some(candidate.target_id.clone());
        state.status.replay_ready = false;
        state.status.pending_target_id = None;
        state.last_failed_target = None;
        state.ready_notified_target = None;
        state.pending_target = None;
        state.pending_polls = 0;
        drop(state);
        crate::desktop::refresh_tray_status(app);
    } else if !result.success {
        let message = result
            .error_message
            .unwrap_or_else(|| "The detected game could not start Replay.".to_string());
        let mut state = lock_runtime(runtime);
        state.last_failed_target = Some((candidate.target_id.clone(), Instant::now()));
        state.pending_target = None;
        state.pending_polls = 0;
        drop(state);
        let _ = app.emit(
            "game-auto-arm-feedback",
            AutoArmFeedback {
                success: false,
                message,
                target_label: Some(candidate.title.clone()),
            },
        );
    }
}

fn candidate_is_eligible(
    preferences: &UiPreferences,
    candidates: &[GameCandidate],
    target_id: &str,
) -> bool {
    let excluded = normalized_set(&preferences.game_detection_excluded_processes);
    candidates.iter().any(|candidate| {
        candidate.target_id == target_id
            && !excluded.contains(&normalize_process(&candidate.process_name))
            && (preferences.game_detection_mode == GameDetectionMode::AnyDetectedGame
                || candidate.approved)
    })
}

fn closed_target_requires_stop(
    preferences: &UiPreferences,
    target_id: &str,
    live_target_ids: &BTreeSet<&str>,
) -> bool {
    preferences.game_stop_replay_on_close && !live_target_ids.contains(target_id)
}

fn select_automatic_candidate<'a>(
    preferences: &UiPreferences,
    candidates: &'a [GameCandidate],
    stable_target_id: Option<&str>,
) -> Option<&'a GameCandidate> {
    if !preferences.game_detection_enabled || !preferences.game_auto_arm {
        return None;
    }
    let excluded = normalized_set(&preferences.game_detection_excluded_processes);
    candidates
        .iter()
        .filter(|candidate| {
            !excluded.contains(&normalize_process(&candidate.process_name))
                && (preferences.game_detection_mode == GameDetectionMode::AnyDetectedGame
                    || candidate.approved)
        })
        .max_by(|left, right| {
            left.foreground
                .cmp(&right.foreground)
                .then_with(|| {
                    (u64::from(left.width) * u64::from(left.height))
                        .cmp(&(u64::from(right.width) * u64::from(right.height)))
                })
                .then_with(|| left.confidence_score.cmp(&right.confidence_score))
                .then_with(|| {
                    (stable_target_id == Some(left.target_id.as_str()))
                        .cmp(&(stable_target_id == Some(right.target_id.as_str())))
                })
                .then_with(|| right.process_name.cmp(&left.process_name))
                .then_with(|| right.target_id.cmp(&left.target_id))
        })
}

fn observe_pending_candidate(
    runtime: &Arc<Mutex<DetectionRuntime>>,
    target_id: Option<&str>,
) -> Option<String> {
    let mut state = lock_runtime(runtime);
    let Some(target_id) = target_id else {
        state.pending_target = None;
        state.pending_polls = 0;
        return None;
    };
    if state.pending_target.as_deref() == Some(target_id) {
        state.pending_polls = state.pending_polls.saturating_add(1);
    } else {
        state.pending_target = Some(target_id.to_string());
        state.pending_polls = 1;
    }
    (state.pending_polls >= REQUIRED_STABLE_POLLS).then(|| target_id.to_string())
}

fn mark_ready_transition(
    runtime: &Arc<Mutex<DetectionRuntime>>,
    target_id: &str,
    running: bool,
    eligible: bool,
) -> bool {
    if !running || !eligible {
        return false;
    }
    let mut state = lock_runtime(runtime);
    if state.ready_notified_target.as_deref() == Some(target_id) {
        return false;
    }
    state.ready_notified_target = Some(target_id.to_string());
    true
}

fn is_authoritative_replay_ready(status: &ReplayBufferStatus) -> bool {
    replay_ready_from_signals(
        status.state,
        &status.capture_health,
        status.frames_observed,
        status.source_frames_detached,
    )
}

fn replay_ready_from_signals(
    state: ReplayLifecycleState,
    capture_health: &str,
    frames_observed: u64,
    source_frames_detached: u64,
) -> bool {
    state == ReplayLifecycleState::Running
        && capture_health == "Healthy"
        && frames_observed > 0
        && source_frames_detached > 0
}

fn detected_replay_state(
    detection: &GameDetectionStatus,
    replay: &ReplayBufferStatus,
    last_failed_target: Option<&str>,
) -> DetectedReplayState {
    let has_tracked_session = detection.auto_armed_target_id.is_some();
    let current_candidate_failed = last_failed_target.is_some_and(|failed| {
        detection
            .candidates
            .iter()
            .any(|candidate| candidate.target_id == failed)
    });
    detected_replay_state_from_signals(
        has_tracked_session,
        current_candidate_failed,
        !detection.candidates.is_empty(),
        replay.state,
        &replay.capture_health,
        is_authoritative_replay_ready(replay),
    )
}

fn detected_replay_state_from_signals(
    has_tracked_session: bool,
    current_candidate_failed: bool,
    has_candidates: bool,
    replay_state: ReplayLifecycleState,
    capture_health: &str,
    replay_ready: bool,
) -> DetectedReplayState {
    if (has_tracked_session || current_candidate_failed)
        && (replay_state == ReplayLifecycleState::Error || capture_health == "Failed")
    {
        DetectedReplayState::CaptureFailed
    } else if has_tracked_session {
        if replay_ready {
            DetectedReplayState::ReplayReady
        } else {
            match replay_state {
                ReplayLifecycleState::Starting | ReplayLifecycleState::Running => {
                    DetectedReplayState::Starting
                }
                ReplayLifecycleState::Error => DetectedReplayState::CaptureFailed,
                ReplayLifecycleState::Stopped | ReplayLifecycleState::Stopping => {
                    DetectedReplayState::ReplayStopped
                }
            }
        }
    } else if !has_candidates {
        DetectedReplayState::ReplayStopped
    } else {
        DetectedReplayState::Detected
    }
}

fn automatic_start_candidate_for_target<'a>(
    preferences: &UiPreferences,
    replay_state: ReplayLifecycleState,
    candidates: &'a [GameCandidate],
    stabilized_target_id: &str,
    manual_override_active: bool,
) -> Option<&'a GameCandidate> {
    if manual_override_active
        || !matches!(
            replay_state,
            ReplayLifecycleState::Stopped | ReplayLifecycleState::Error
        )
    {
        return None;
    }
    let selected = select_automatic_candidate(preferences, candidates, Some(stabilized_target_id))?;
    (selected.target_id == stabilized_target_id).then_some(selected)
}

fn stop_tracked_buffer_if_needed(
    replay: &ReplayBufferManager,
    runtime: &Arc<Mutex<DetectionRuntime>>,
    reason: &str,
) {
    let tracked = {
        let mut state = lock_runtime(runtime);
        state.ready_notified_target = None;
        state.pending_target = None;
        state.pending_polls = 0;
        state.status.auto_armed_target_id.take()
    };
    if tracked.is_some() && replay.status().state.is_active() {
        let _ = replay.stop_and_wait();
        eprintln!("SlickClip game auto-arm stopped: {reason}");
    }
}

fn classify_windows(windows: &[WindowTarget], preferences: &UiPreferences) -> Vec<GameCandidate> {
    classify_windows_with_process_tree(windows, preferences, &ProcessTree::capture())
}

fn classify_windows_with_process_tree(
    windows: &[WindowTarget],
    preferences: &UiPreferences,
    process_tree: &ProcessTree,
) -> Vec<GameCandidate> {
    let approved = normalized_set(&preferences.game_detection_approved_processes);
    let excluded = normalized_set(&preferences.game_detection_excluded_processes);
    let mut candidates = windows
        .iter()
        .filter_map(|window| classify_window(window, &approved, &excluded, process_tree))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .foreground
            .cmp(&left.foreground)
            .then_with(|| {
                (u64::from(right.width) * u64::from(right.height))
                    .cmp(&(u64::from(left.width) * u64::from(left.height)))
            })
            .then_with(|| right.confidence_score.cmp(&left.confidence_score))
            .then_with(|| left.process_name.cmp(&right.process_name))
            .then_with(|| left.target_id.cmp(&right.target_id))
    });
    candidates
}

fn classify_window(
    window: &WindowTarget,
    approved: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
    process_tree: &ProcessTree,
) -> Option<GameCandidate> {
    let process_name = window
        .process_name
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .or_else(|| {
            window
                .executable_path
                .as_deref()
                .and_then(executable_name_from_path)
        })
        .or_else(|| process_tree.process_name(window.process_id))?
        .trim();
    let normalized = normalize_process(process_name);
    if normalized.is_empty() || excluded.contains(&normalized) {
        return None;
    }
    let explicitly_approved = approved.contains(&normalized);
    if !explicitly_approved
        && (default_process_exclusions().contains(normalized.as_str())
            || looks_like_helper_process(&normalized)
            || window
                .executable_path
                .as_deref()
                .is_some_and(is_infrastructure_path))
    {
        return None;
    }
    if window.width < 800 || window.height < 450 {
        return None;
    }
    let title = window.title.to_lowercase();
    if !explicitly_approved
        && [
            "launcher",
            "updater",
            "update available",
            "settings",
            "sign in",
            "login",
            "library",
        ]
        .iter()
        .any(|term| title.contains(term))
    {
        return None;
    }
    let game_install_source = window
        .executable_path
        .as_deref()
        .and_then(game_install_source);
    let launcher_ancestor = process_tree.launcher_ancestor(window.process_id);
    let near_monitor = is_near_monitor_sized(window);
    let borderless = window.title_bar_height.is_some_and(|height| height <= 8);
    let aspect = window.width as f64 / window.height as f64;
    if !explicitly_approved && game_install_source.is_none() && !(1.2..=3.8).contains(&aspect) {
        return None;
    }
    if !explicitly_approved
        && game_install_source.is_none()
        && launcher_ancestor.is_none()
        && !(near_monitor && borderless)
    {
        return None;
    }
    let confidence_score = if explicitly_approved {
        u8::MAX
    } else {
        u8::from(near_monitor)
            + u8::from(borderless)
            + game_install_source.map_or(0, |_| 5)
            + launcher_ancestor.map_or(0, |_| 4)
    };
    let reason = if explicitly_approved {
        "Approved · explicit process rule".to_string()
    } else if let Some(source) = game_install_source {
        if near_monitor && borderless {
            format!("High confidence · {source} + borderless near-monitor window")
        } else {
            format!("High confidence · {source} + substantial game window")
        }
    } else if let Some(launcher) = launcher_ancestor {
        format!("High confidence · {launcher} launch ancestry + substantial game window")
    } else {
        "Possible game · borderless near-monitor window".to_string()
    };
    Some(GameCandidate {
        target_id: window.id.clone(),
        title: window.title.clone(),
        process_name: process_name.to_string(),
        process_id: window.process_id,
        width: window.width,
        height: window.height,
        foreground: window.foreground,
        approved: explicitly_approved,
        reason,
        confidence_score,
    })
}

fn executable_name_from_path(path: &str) -> Option<&str> {
    path.rsplit(['\\', '/'])
        .find(|component| !component.trim().is_empty())
}

fn path_components(path: &str) -> Vec<String> {
    path.split(['\\', '/'])
        .map(str::trim)
        .filter(|component| !component.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn game_install_source(path: &str) -> Option<&'static str> {
    let components = path_components(path);
    if components
        .windows(2)
        .any(|pair| pair[0] == "steamapps" && pair[1] == "common")
    {
        return Some("Steam game directory");
    }
    if components.iter().any(|component| component == "xboxgames") {
        return Some("Xbox game directory");
    }
    if components
        .iter()
        .any(|component| component == "modifiablewindowsapps")
    {
        return Some("Microsoft game directory");
    }
    if let Some(epic_index) = components
        .iter()
        .position(|component| component == "epic games")
    {
        let tail = &components[epic_index + 1..];
        if !tail.is_empty() && !tail.iter().any(|component| component == "launcher") {
            return Some("Epic Games directory");
        }
    }
    if components
        .windows(2)
        .any(|pair| pair[0] == "gog galaxy" && pair[1] == "games")
    {
        return Some("GOG game directory");
    }
    if components
        .windows(2)
        .any(|pair| pair[0] == "amazon games" && pair[1] == "library")
    {
        return Some("Amazon Games directory");
    }
    None
}

fn is_near_monitor_sized(window: &WindowTarget) -> bool {
    let (Some(monitor_width), Some(monitor_height)) = (window.monitor_width, window.monitor_height)
    else {
        return false;
    };
    let width = u64::from(window.width);
    let height = u64::from(window.height);
    let monitor_width = u64::from(monitor_width);
    let monitor_height = u64::from(monitor_height);
    width * 100 >= monitor_width * 88
        && width * 100 <= monitor_width * 110
        && height * 100 >= monitor_height * 88
        && height * 100 <= monitor_height * 110
}

fn looks_like_helper_process(normalized_process: &str) -> bool {
    [
        "bootstrapper",
        "crashhandler",
        "crashreporter",
        "dlcunlocker",
        "helper",
        "installer",
        "launcher",
        "overlay",
        "patcher",
        "uninstaller",
        "unlocker",
        "updater",
    ]
    .iter()
    .any(|term| normalized_process.contains(term))
}

fn is_infrastructure_path(path: &str) -> bool {
    let normalized = path.replace('/', "\\").to_ascii_lowercase();
    normalized.contains("\\windows\\system32\\")
        || normalized.contains("\\windows\\syswow64\\")
        || normalized.contains("\\epic games\\launcher\\")
        || normalized.contains("\\riot games\\riot client\\")
}

fn recognized_launcher(normalized_process: &str) -> Option<&'static str> {
    match normalized_process {
        "steam" | "steamservice" => Some("Steam"),
        "epicgameslauncher" => Some("Epic Games"),
        "battle.net" | "battlenet" => Some("Battle.net"),
        "eadesktop" | "origin" => Some("EA"),
        "galaxyclient" => Some("GOG Galaxy"),
        "ubisoftconnect" | "upc" => Some("Ubisoft Connect"),
        "gamingservices" | "xboxpcapp" => Some("Xbox"),
        _ => None,
    }
}

fn normalize_process(value: &str) -> String {
    let normalized = value.trim().to_lowercase();
    normalized
        .strip_suffix(".exe")
        .unwrap_or(&normalized)
        .to_string()
}

fn normalized_set(values: &[String]) -> BTreeSet<String> {
    values
        .iter()
        .map(|value| normalize_process(value))
        .collect()
}

fn default_process_exclusions() -> BTreeSet<&'static str> {
    [
        "applicationframehost",
        "battle.net",
        "battlenet",
        "chatgpt",
        "chrome",
        "code",
        "devenv",
        "discord",
        "eadesktop",
        "epicgameslauncher",
        "explorer",
        "firefox",
        "galaxyclient",
        "msedge",
        "ms-teams",
        "obs64",
        "origin",
        "powershell",
        // Keep excluding the legacy executable name during upgrades.
        "replay-app",
        "slickclip",
        "riotclientservices",
        "searchhost",
        "shellexperiencehost",
        "slack",
        "spotify",
        "startmenuexperiencehost",
        "steam",
        "steamwebhelper",
        "systemsettings",
        "teams",
        "textinputhost",
        "ubisoftconnect",
        "upc",
        "whatsapp",
        "windowsterminal",
    ]
    .into_iter()
    .collect()
}

fn lock_runtime(runtime: &Arc<Mutex<DetectionRuntime>>) -> MutexGuard<'_, DetectionRuntime> {
    runtime
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

#[tauri::command]
pub fn get_game_detection_status(
    manager: State<'_, GameDetectionManager>,
    replay: State<'_, ReplayBufferManager>,
) -> GameDetectionStatus {
    manager.status_with_replay(&replay.status())
}

#[tauri::command]
pub fn set_game_detection_manual_override(
    manager: State<'_, GameDetectionManager>,
    target_id: Option<String>,
) -> Result<GameDetectionStatus, String> {
    let target_id = target_id
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    if target_id.as_ref().is_some_and(|value| {
        value.len() > 256 || !(value.starts_with("window:") || value.starts_with("monitor:"))
    }) {
        return Err("The manual capture target identifier is invalid.".to_string());
    }
    manager.set_manual_override(target_id);
    Ok(manager.status())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(process: &str, title: &str, width: u32, height: u32) -> WindowTarget {
        WindowTarget {
            id: format!("window:{process}"),
            title: title.to_string(),
            process_name: Some(process.to_string()),
            process_id: 42,
            width,
            height,
            foreground: false,
            executable_path: None,
            monitor_width: Some(1920),
            monitor_height: Some(1080),
            title_bar_height: Some(32),
        }
    }

    fn with_path(mut window: WindowTarget, executable_path: &str) -> WindowTarget {
        window.executable_path = Some(executable_path.to_string());
        window
    }

    fn borderless(mut window: WindowTarget) -> WindowTarget {
        window.title_bar_height = Some(0);
        window
    }

    fn foreground(mut window: WindowTarget) -> WindowTarget {
        window.foreground = true;
        window
    }

    fn process_tree(entries: &[(u32, u32, &str)]) -> ProcessTree {
        ProcessTree {
            entries: entries
                .iter()
                .map(|(process_id, parent_process_id, process_name)| {
                    (
                        *process_id,
                        ProcessSnapshotEntry {
                            parent_process_id: *parent_process_id,
                            process_name: (*process_name).to_string(),
                        },
                    )
                })
                .collect(),
        }
    }

    fn classify(windows: &[WindowTarget], preferences: &UiPreferences) -> Vec<GameCandidate> {
        classify_windows_with_process_tree(windows, preferences, &ProcessTree::default())
    }

    #[test]
    fn suggestions_never_become_approved_without_a_manual_override() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            ..UiPreferences::default()
        };
        let candidates = classify(
            &[with_path(
                window("GreatGame.exe", "Great Game", 1920, 1080),
                r"D:\SteamLibrary\steamapps\common\Great Game\GreatGame.exe",
            )],
            &preferences,
        );
        assert_eq!(candidates.len(), 1);
        assert!(!candidates[0].approved);
    }

    #[test]
    fn explicit_approval_wins_over_default_heuristics_but_exclusion_wins_over_approval() {
        let mut preferences = UiPreferences {
            game_detection_enabled: true,
            game_detection_approved_processes: vec!["Discord".into()],
            ..UiPreferences::default()
        };
        let running = [window("Discord.exe", "Approved stream", 1280, 720)];
        assert!(classify(&running, &preferences)[0].approved);
        preferences.game_detection_excluded_processes = vec!["discord.exe".into()];
        assert!(classify(&running, &preferences).is_empty());
    }

    #[test]
    fn small_launcher_and_productivity_windows_are_not_suggested() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            ..UiPreferences::default()
        };
        let windows = [
            window("NewGame.exe", "New Game Launcher", 1280, 720),
            window("chrome.exe", "A large video", 1920, 1080),
            window("TinyGame.exe", "Tiny Game", 640, 360),
        ];
        assert!(classify(&windows, &preferences).is_empty());
    }

    #[test]
    fn real_mw4_fixture_qualifies_from_general_steam_path_and_window_evidence() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            ..UiPreferences::default()
        };
        let mut mw4 = with_path(
            borderless(window(
                "cod26-cod.exe",
                "Call of Duty®: Modern Warfare® 4",
                1920,
                1080,
            )),
            r"G:\SteamLibrary\steamapps\common\Modern Warfare 4 - Beta\cod26-cod.exe",
        );
        // Reproduce the access pattern that caused the old classifier to drop the window.
        mw4.process_name = None;
        mw4.process_id = 23_740;

        let tree = process_tree(&[(23_740, 8_788, "cod26-cod.exe"), (8_788, 1, "steam.exe")]);
        let candidates = classify_windows_with_process_tree(&[mw4], &preferences, &tree);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].process_name, "cod26-cod.exe");
        assert!(!candidates[0].approved);
        assert!(candidates[0].reason.contains("Steam game directory"));
    }

    #[test]
    fn unknown_executable_in_steam_common_is_a_likely_game() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            ..UiPreferences::default()
        };
        let candidate = with_path(
            window("x7_qz.exe", "Unknown Game", 1280, 720),
            r"E:\Games\steamapps\common\Unknown Game\x7_qz.exe",
        );
        let candidates = classify(&[candidate], &preferences);
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].reason.starts_with("High confidence"));
    }

    #[test]
    fn known_library_paths_are_generic_and_launcher_directories_are_not_games() {
        assert_eq!(
            game_install_source(r"D:\Epic Games\Unknown Title\game.exe"),
            Some("Epic Games directory")
        );
        assert_eq!(
            game_install_source(r"C:\XboxGames\Unknown Title\Content\game.exe"),
            Some("Xbox game directory")
        );
        assert_eq!(
            game_install_source(r"C:\Program Files\ModifiableWindowsApps\Unknown\game.exe"),
            Some("Microsoft game directory")
        );
        assert_eq!(
            game_install_source(r"C:\Program Files\Epic Games\Launcher\Portal\launcher.exe"),
            None
        );
    }

    #[test]
    fn large_normal_unknown_desktop_window_is_not_enough_by_itself() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            ..UiPreferences::default()
        };
        let candidate = window("UnknownDesktop.exe", "Work dashboard", 1920, 1040);
        assert!(classify(&[candidate], &preferences).is_empty());
    }

    #[test]
    fn maximized_desktop_apps_and_helper_processes_are_not_likely_games() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            ..UiPreferences::default()
        };
        let windows = [
            borderless(window("ChatGPT.exe", "ChatGPT", 1920, 1080)),
            borderless(window("Spotify.exe", "Spotify Premium", 1920, 1080)),
            borderless(window("chrome.exe", "YouTube", 1920, 1080)),
            borderless(window("Discord.exe", "Friends", 1920, 1080)),
            borderless(window("steamwebhelper.exe", "Steam", 1920, 1080)),
            borderless(window("VyxxnnDLCUnlocker.exe", "DLC Unlocker", 1920, 1080)),
        ];
        assert!(classify(&windows, &preferences).is_empty());
    }

    #[test]
    fn launcher_is_not_a_target_but_its_substantial_child_can_be_detected() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            ..UiPreferences::default()
        };
        let mut launcher = borderless(window("steam.exe", "Steam", 1920, 1080));
        launcher.process_id = 100;
        let mut game = window("opaque.exe", "Opaque Game", 1600, 900);
        game.process_id = 200;
        game.process_name = None;
        let tree = process_tree(&[(100, 1, "steam.exe"), (200, 100, "opaque.exe")]);
        let candidates = classify_windows_with_process_tree(&[launcher, game], &preferences, &tree);
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].process_name, "opaque.exe");
        assert!(candidates[0].reason.contains("Steam launch ancestry"));
    }

    #[test]
    fn explicit_approval_keeps_unusual_process_auto_arm_eligible() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            game_auto_arm: true,
            game_detection_approved_processes: vec!["odd_name.exe".into()],
            ..UiPreferences::default()
        };
        let candidates = classify(
            &[window(
                "ODD_NAME.EXE",
                "Unrecognized application",
                1280,
                720,
            )],
            &preferences,
        );
        assert_eq!(candidates.len(), 1);
        assert!(candidates[0].approved);
        assert!(select_automatic_candidate(&preferences, &candidates, None).is_some());
    }

    #[test]
    fn auto_arm_off_detects_approved_game_but_keeps_replay_stopped() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            game_auto_arm: false,
            game_detection_approved_processes: vec!["approved_game".into()],
            ..UiPreferences::default()
        };
        let candidates = classify(
            &[window("approved_game.exe", "Approved Game", 1920, 1080)],
            &preferences,
        );

        assert_eq!(candidates.len(), 1, "detection may still observe the game");
        assert!(candidates[0].approved);
        assert!(select_automatic_candidate(&preferences, &candidates, None).is_none());
    }

    #[test]
    fn auto_arm_on_starts_exactly_once_across_repeated_detection_polls() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            game_auto_arm: true,
            game_detection_approved_processes: vec!["approved_game".into()],
            ..UiPreferences::default()
        };
        let candidates = classify(
            &[window("approved_game.exe", "Approved Game", 1920, 1080)],
            &preferences,
        );
        let mut replay_state = ReplayLifecycleState::Stopped;
        let mut start_count = 0;
        let target_id = candidates[0].target_id.clone();

        for _ in 0..5 {
            if automatic_start_candidate_for_target(
                &preferences,
                replay_state,
                &candidates,
                &target_id,
                false,
            )
            .is_some()
            {
                start_count += 1;
                replay_state = ReplayLifecycleState::Running;
            }
        }

        assert_eq!(start_count, 1);
    }

    #[test]
    fn explicit_exclusion_still_overrides_approval() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            game_auto_arm: true,
            game_detection_approved_processes: vec!["odd_name".into()],
            game_detection_excluded_processes: vec!["ODD_NAME.exe".into()],
            ..UiPreferences::default()
        };
        assert!(classify(
            &[window("odd_name.exe", "Odd Game", 1280, 720)],
            &preferences,
        )
        .is_empty());
    }

    #[test]
    fn approved_game_disappearance_and_reappearance_stays_unambiguous() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            game_auto_arm: true,
            game_detection_approved_processes: vec!["reappearing_game".into()],
            ..UiPreferences::default()
        };
        let live = [window(
            "Reappearing_Game.exe",
            "Reappearing Game",
            1920,
            1080,
        )];
        let first = classify(&live, &preferences);
        assert!(select_automatic_candidate(&preferences, &first, None).is_some());
        let disappeared = classify(&[], &preferences);
        assert!(select_automatic_candidate(&preferences, &disappeared, None).is_none());
        let reappeared = classify(&live, &preferences);
        assert!(select_automatic_candidate(&preferences, &reappeared, None).is_some());
    }

    #[test]
    fn multiple_approved_live_targets_choose_one_deterministically() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            game_auto_arm: true,
            game_detection_approved_processes: vec!["game_one".into(), "game_two".into()],
            ..UiPreferences::default()
        };
        let candidates = classify(
            &[
                window("game_one.exe", "Game One", 1280, 720),
                window("game_two.exe", "Game Two", 1280, 720),
            ],
            &preferences,
        );
        assert_eq!(candidates.len(), 2);
        let first = select_automatic_candidate(&preferences, &candidates, None)
            .unwrap()
            .target_id
            .clone();
        let second = select_automatic_candidate(&preferences, &candidates, None)
            .unwrap()
            .target_id
            .clone();
        assert_eq!(first, second);
    }

    #[test]
    fn any_detected_mode_selects_a_confident_unapproved_game() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            game_auto_arm: true,
            game_detection_mode: GameDetectionMode::AnyDetectedGame,
            ..UiPreferences::default()
        };
        let candidates = classify(
            &[with_path(
                window("new_game.exe", "New Game", 1920, 1080),
                r"D:\SteamLibrary\steamapps\common\New Game\new_game.exe",
            )],
            &preferences,
        );
        let selected = select_automatic_candidate(&preferences, &candidates, None).unwrap();
        assert_eq!(selected.process_name, "new_game.exe");
        assert!(!selected.approved);
    }

    #[test]
    fn approved_only_mode_ignores_unapproved_games_and_accepts_an_approval() {
        let mut preferences = UiPreferences {
            game_detection_mode: GameDetectionMode::ApprovedGamesOnly,
            ..UiPreferences::default()
        };
        let running = [with_path(
            window("strict_game.exe", "Strict Game", 1920, 1080),
            r"D:\SteamLibrary\steamapps\common\Strict Game\strict_game.exe",
        )];
        let candidates = classify(&running, &preferences);
        assert!(select_automatic_candidate(&preferences, &candidates, None).is_none());

        preferences.game_detection_approved_processes = vec!["strict_game".into()];
        let approved = classify(&running, &preferences);
        assert_eq!(
            select_automatic_candidate(&preferences, &approved, None)
                .unwrap()
                .process_name,
            "strict_game.exe"
        );
    }

    #[test]
    fn foreground_game_wins_deterministically_over_a_larger_candidate() {
        let preferences = UiPreferences::default();
        let candidates = classify(
            &[
                with_path(
                    foreground(window("foreground.exe", "Foreground Game", 1280, 720)),
                    r"D:\SteamLibrary\steamapps\common\Foreground\foreground.exe",
                ),
                with_path(
                    window("larger.exe", "Larger Game", 2560, 1440),
                    r"D:\SteamLibrary\steamapps\common\Larger\larger.exe",
                ),
            ],
            &preferences,
        );
        assert_eq!(
            select_automatic_candidate(&preferences, &candidates, None)
                .unwrap()
                .process_name,
            "foreground.exe"
        );
    }

    #[test]
    fn candidate_must_survive_two_polls_and_repeated_polls_start_only_once() {
        let preferences = UiPreferences::default();
        let candidates = classify(
            &[with_path(
                window("stable.exe", "Stable Game", 1920, 1080),
                r"D:\SteamLibrary\steamapps\common\Stable\stable.exe",
            )],
            &preferences,
        );
        let runtime = Arc::new(Mutex::new(DetectionRuntime::default()));
        let target = candidates[0].target_id.as_str();
        let mut replay_state = ReplayLifecycleState::Stopped;
        let mut starts = 0;
        for _ in 0..5 {
            if let Some(stable) = observe_pending_candidate(&runtime, Some(target)) {
                if automatic_start_candidate_for_target(
                    &preferences,
                    replay_state,
                    &candidates,
                    &stable,
                    false,
                )
                .is_some()
                {
                    starts += 1;
                    replay_state = ReplayLifecycleState::Running;
                }
            }
        }
        assert_eq!(starts, 1);
    }

    #[test]
    fn changing_candidates_resets_stabilization() {
        let runtime = Arc::new(Mutex::new(DetectionRuntime::default()));
        assert!(observe_pending_candidate(&runtime, Some("game-a")).is_none());
        assert!(observe_pending_candidate(&runtime, Some("game-b")).is_none());
        assert_eq!(
            observe_pending_candidate(&runtime, Some("game-b")),
            Some("game-b".into())
        );
    }

    #[test]
    fn manual_override_blocks_the_automatic_start_gate_until_cleared() {
        let manager = GameDetectionManager::new();
        let preferences = UiPreferences::default();
        let candidates = classify(
            &[with_path(
                window("manual_game.exe", "Manual Game", 1920, 1080),
                r"D:\SteamLibrary\steamapps\common\Manual Game\manual_game.exe",
            )],
            &preferences,
        );
        manager.set_manual_override(Some("window:manual".into()));
        assert!(manager.status().manual_override_active);
        assert!(manager.lock_runtime().manual_override_target.is_some());
        assert!(automatic_start_candidate_for_target(
            &preferences,
            ReplayLifecycleState::Stopped,
            &candidates,
            &candidates[0].target_id,
            true,
        )
        .is_none());
        manager.note_manual_session_stopped();
        assert!(!manager.status().manual_override_active);
        assert!(manager.lock_runtime().manual_override_target.is_none());
    }

    #[test]
    fn replay_ready_transition_is_emitted_once_per_session() {
        let runtime = Arc::new(Mutex::new(DetectionRuntime::default()));
        assert!(mark_ready_transition(&runtime, "game", true, true));
        assert!(!mark_ready_transition(&runtime, "game", true, true));
        lock_runtime(&runtime).ready_notified_target = None;
        assert!(mark_ready_transition(&runtime, "game", true, true));
        assert!(!mark_ready_transition(&runtime, "other", false, true));
    }

    #[test]
    fn detected_game_does_not_report_ready_while_starting_or_after_capture_failure() {
        assert!(!replay_ready_from_signals(
            ReplayLifecycleState::Starting,
            "Probing",
            0,
            0,
        ));
        assert_eq!(
            detected_replay_state_from_signals(
                true,
                false,
                true,
                ReplayLifecycleState::Starting,
                "Probing",
                false,
            ),
            DetectedReplayState::Starting,
        );
        assert_eq!(
            detected_replay_state_from_signals(
                false,
                true,
                true,
                ReplayLifecycleState::Error,
                "Failed",
                false,
            ),
            DetectedReplayState::CaptureFailed,
        );
    }

    #[test]
    fn detected_game_reports_ready_only_after_healthy_production_frames() {
        assert!(!replay_ready_from_signals(
            ReplayLifecycleState::Running,
            "Healthy",
            1,
            0,
        ));
        assert!(replay_ready_from_signals(
            ReplayLifecycleState::Running,
            "Healthy",
            1,
            1,
        ));
        assert_eq!(
            detected_replay_state_from_signals(
                true,
                false,
                true,
                ReplayLifecycleState::Running,
                "Healthy",
                true,
            ),
            DetectedReplayState::ReplayReady,
        );
    }

    #[test]
    fn active_target_stays_stable_when_another_game_becomes_foreground() {
        let preferences = UiPreferences::default();
        let initial = classify(
            &[with_path(
                window("game_one.exe", "Game One", 1920, 1080),
                r"D:\SteamLibrary\steamapps\common\Game One\game_one.exe",
            )],
            &preferences,
        );
        let active_target = initial[0].target_id.clone();
        let later = classify(
            &[
                with_path(
                    window("game_one.exe", "Game One", 1920, 1080),
                    r"D:\SteamLibrary\steamapps\common\Game One\game_one.exe",
                ),
                with_path(
                    foreground(window("game_two.exe", "Game Two", 1920, 1080)),
                    r"D:\SteamLibrary\steamapps\common\Game Two\game_two.exe",
                ),
            ],
            &preferences,
        );
        assert!(candidate_is_eligible(&preferences, &later, &active_target));
        assert_ne!(
            select_automatic_candidate(&preferences, &later, None)
                .unwrap()
                .target_id,
            active_target
        );
        // The runtime does not call selection while auto_armed_target_id is populated.
        assert_eq!(active_target, initial[0].target_id);
    }

    #[test]
    fn closed_game_stops_once_and_a_later_game_can_stabilize() {
        let preferences = UiPreferences::default();
        let first = classify(
            &[with_path(
                window("first.exe", "First", 1920, 1080),
                r"D:\SteamLibrary\steamapps\common\First\first.exe",
            )],
            &preferences,
        );
        let mut tracked = Some(first[0].target_id.clone());
        let mut stop_count = 0;
        let no_live_targets = BTreeSet::new();
        for _ in 0..3 {
            if tracked.as_ref().is_some_and(|target| {
                closed_target_requires_stop(&preferences, target, &no_live_targets)
            }) {
                stop_count += 1;
                tracked = None;
            }
        }
        assert_eq!(stop_count, 1);

        let next = classify(
            &[with_path(
                window("next.exe", "Next", 1920, 1080),
                r"D:\SteamLibrary\steamapps\common\Next\next.exe",
            )],
            &preferences,
        );
        let runtime = Arc::new(Mutex::new(DetectionRuntime::default()));
        let selected = select_automatic_candidate(&preferences, &next, None).unwrap();
        assert!(observe_pending_candidate(&runtime, Some(&selected.target_id)).is_none());
        assert_eq!(
            observe_pending_candidate(&runtime, Some(&selected.target_id)),
            Some(selected.target_id.clone())
        );
    }

    #[test]
    fn close_stop_preference_can_leave_detection_from_proactively_stopping() {
        let preferences = UiPreferences {
            game_stop_replay_on_close: false,
            ..UiPreferences::default()
        };
        assert!(!closed_target_requires_stop(
            &preferences,
            "closed-game",
            &BTreeSet::new()
        ));
    }
}
