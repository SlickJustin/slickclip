use std::collections::BTreeSet;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, MutexGuard,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, State};

use crate::capture::encoder::EncoderChoice;
use crate::capture::targets::{self, CaptureTargetRequest, CaptureTargetType, WindowTarget};
use crate::preferences::{UiPreferences, UiPreferencesManager};
use crate::replay::{
    AudioReplayConfiguration, AudioSourceKind, AudioTrackConfiguration, AudioTrackRole,
    ReplayBufferManager, ReplayBufferStartRequest, ReplayLifecycleState,
};

const DETECTION_INTERVAL: Duration = Duration::from_secs(2);
const FAILED_START_COOLDOWN: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameCandidate {
    target_id: String,
    title: String,
    process_name: String,
    process_id: u32,
    width: u32,
    height: u32,
    approved: bool,
    reason: String,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GameDetectionStatus {
    success: bool,
    enabled: bool,
    auto_arm_enabled: bool,
    candidates: Vec<GameCandidate>,
    auto_armed_target_id: Option<String>,
    last_scan_at_ms: Option<u64>,
    error_message: Option<String>,
}

impl Default for GameDetectionStatus {
    fn default() -> Self {
        Self {
            success: true,
            enabled: false,
            auto_arm_enabled: false,
            candidates: Vec::new(),
            auto_armed_target_id: None,
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
}

impl Default for DetectionRuntime {
    fn default() -> Self {
        Self {
            status: GameDetectionStatus::default(),
            last_failed_target: None,
            ready_notified_target: None,
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
        state.status = GameDetectionStatus::default();
        return;
    }

    let windows = match targets::enumerate_windows() {
        Ok(windows) => windows,
        Err(error) => {
            let mut state = lock_runtime(runtime);
            state.status.success = false;
            state.status.enabled = true;
            state.status.auto_arm_enabled = preferences.game_auto_arm;
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
    let tracked_target = lock_runtime(runtime).status.auto_armed_target_id.clone();
    let replay_status = replay.status();

    if let Some(target_id) = tracked_target.as_ref() {
        let should_notify = replay_status.state == ReplayLifecycleState::Running
            && lock_runtime(runtime).ready_notified_target.as_ref() != Some(target_id)
            && candidates
                .iter()
                .any(|candidate| candidate.approved && &candidate.target_id == target_id);
        if should_notify {
            let target_label = replay_status.target_label.clone().or_else(|| {
                candidates
                    .iter()
                    .find(|candidate| &candidate.target_id == target_id)
                    .map(|candidate| format!("{} — {}", candidate.process_name, candidate.title))
            });
            lock_runtime(runtime).ready_notified_target = Some(target_id.clone());
            let message = target_label
                .as_ref()
                .map(|label| format!("Replay Buffer ready for {label}."))
                .unwrap_or_else(|| "Replay Buffer ready for the approved game.".to_string());
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
                "Replay Buffer Ready",
                target_label
                    .as_deref()
                    .unwrap_or("Approved game capture is active"),
            );
            crate::desktop::refresh_tray_status(app);
        }
    }

    let tracked_still_approved = tracked_target.as_ref().is_none_or(|target| {
        candidates
            .iter()
            .any(|candidate| candidate.approved && &candidate.target_id == target)
    });
    if !tracked_still_approved {
        stop_tracked_buffer_if_needed(
            &replay,
            runtime,
            "The process approval was removed or excluded.",
        );
    } else if tracked_target
        .as_ref()
        .is_some_and(|target| !live_target_ids.contains(target.as_str()))
    {
        stop_tracked_buffer_if_needed(&replay, runtime, "The approved game window closed.");
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
            let message = replay_status
                .error_message
                .clone()
                .unwrap_or_else(|| "The approved game capture stopped with an error.".to_string());
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
    }

    {
        let mut state = lock_runtime(runtime);
        let auto_armed_target_id = state.status.auto_armed_target_id.clone();
        state.status = GameDetectionStatus {
            success: true,
            enabled: true,
            auto_arm_enabled: preferences.game_auto_arm,
            candidates: candidates.clone(),
            auto_armed_target_id,
            last_scan_at_ms: Some(now_ms()),
            error_message: None,
        };
    }

    if !preferences.game_auto_arm
        || !matches!(
            replay.status().state,
            ReplayLifecycleState::Stopped | ReplayLifecycleState::Error
        )
    {
        return;
    }
    let approved = candidates
        .iter()
        .filter(|candidate| candidate.approved)
        .collect::<Vec<_>>();
    if approved.len() != 1 {
        return;
    }
    let candidate = approved[0];
    let retry_blocked = lock_runtime(runtime)
        .last_failed_target
        .as_ref()
        .is_some_and(|(target, attempted)| {
            target == &candidate.target_id && attempted.elapsed() < FAILED_START_COOLDOWN
        });
    if retry_blocked {
        return;
    }

    let request = ReplayBufferStartRequest {
        target: CaptureTargetRequest {
            target_type: CaptureTargetType::Window,
            id: candidate.target_id.clone(),
        },
        encoder: EncoderChoice::Automatic,
        replay_duration_seconds: 120,
        frame_rate: 60,
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
    let result = replay.start(request);
    if result.success {
        {
            let mut state = lock_runtime(runtime);
            state.status.auto_armed_target_id = Some(candidate.target_id.clone());
            state.last_failed_target = None;
            state.ready_notified_target = None;
        }
        crate::desktop::refresh_tray_status(app);
    } else {
        let message = result
            .error_message
            .unwrap_or_else(|| "The approved game could not be auto-armed.".to_string());
        lock_runtime(runtime).last_failed_target =
            Some((candidate.target_id.clone(), Instant::now()));
        let _ = app.emit(
            "game-auto-arm-feedback",
            AutoArmFeedback {
                success: false,
                message,
                target_label: Some(candidate.process_name.clone()),
            },
        );
    }
}

fn stop_tracked_buffer_if_needed(
    replay: &ReplayBufferManager,
    runtime: &Arc<Mutex<DetectionRuntime>>,
    reason: &str,
) {
    let tracked = {
        let mut state = lock_runtime(runtime);
        state.ready_notified_target = None;
        state.status.auto_armed_target_id.take()
    };
    if tracked.is_some() && replay.status().state.is_active() {
        let _ = replay.stop_and_wait();
        eprintln!("SlickClip game auto-arm stopped: {reason}");
    }
}

fn classify_windows(windows: &[WindowTarget], preferences: &UiPreferences) -> Vec<GameCandidate> {
    let approved = normalized_set(&preferences.game_detection_approved_processes);
    let excluded = normalized_set(&preferences.game_detection_excluded_processes);
    let mut candidates = windows
        .iter()
        .filter_map(|window| classify_window(window, &approved, &excluded))
        .collect::<Vec<_>>();
    candidates.sort_by(|left, right| {
        right
            .approved
            .cmp(&left.approved)
            .then_with(|| {
                (u64::from(right.width) * u64::from(right.height))
                    .cmp(&(u64::from(left.width) * u64::from(left.height)))
            })
            .then_with(|| left.process_name.cmp(&right.process_name))
    });
    candidates
}

fn classify_window(
    window: &WindowTarget,
    approved: &BTreeSet<String>,
    excluded: &BTreeSet<String>,
) -> Option<GameCandidate> {
    let process_name = window.process_name.as_ref()?.trim();
    let normalized = normalize_process(process_name);
    if normalized.is_empty() || excluded.contains(&normalized) {
        return None;
    }
    let explicitly_approved = approved.contains(&normalized);
    if !explicitly_approved && default_process_exclusions().contains(normalized.as_str()) {
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
    let aspect = window.width as f64 / window.height as f64;
    if !explicitly_approved && !(1.2..=3.8).contains(&aspect) {
        return None;
    }
    Some(GameCandidate {
        target_id: window.id.clone(),
        title: window.title.clone(),
        process_name: process_name.to_string(),
        process_id: window.process_id,
        width: window.width,
        height: window.height,
        approved: explicitly_approved,
        reason: if explicitly_approved {
            "Explicitly approved for auto-arm".to_string()
        } else {
            "Large dedicated window; approval required before auto-arm".to_string()
        },
    })
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
        "chrome",
        "code",
        "devenv",
        "discord",
        "epicgameslauncher",
        "explorer",
        "firefox",
        "msedge",
        "obs64",
        "powershell",
        "replay-app",
        "riotclientservices",
        "searchhost",
        "shellexperiencehost",
        "startmenuexperiencehost",
        "steam",
        "steamwebhelper",
        "systemsettings",
        "textinputhost",
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
pub fn get_game_detection_status(manager: State<'_, GameDetectionManager>) -> GameDetectionStatus {
    manager.status()
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
        }
    }

    #[test]
    fn suggestions_never_become_approved_without_a_manual_override() {
        let preferences = UiPreferences {
            game_detection_enabled: true,
            ..UiPreferences::default()
        };
        let candidates = classify_windows(
            &[window("GreatGame.exe", "Great Game", 1920, 1080)],
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
        assert!(classify_windows(&running, &preferences)[0].approved);
        preferences.game_detection_excluded_processes = vec!["discord.exe".into()];
        assert!(classify_windows(&running, &preferences).is_empty());
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
        assert!(classify_windows(&windows, &preferences).is_empty());
    }
}
