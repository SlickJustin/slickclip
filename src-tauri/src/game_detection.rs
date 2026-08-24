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
    #[serde(skip)]
    confidence_score: u8,
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
    let Some(candidate) = single_approved_candidate(&candidates) else {
        return;
    };
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

fn single_approved_candidate(candidates: &[GameCandidate]) -> Option<&GameCandidate> {
    let mut approved = candidates.iter().filter(|candidate| candidate.approved);
    let candidate = approved.next()?;
    approved.next().is_none().then_some(candidate)
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
            .approved
            .cmp(&left.approved)
            .then_with(|| right.confidence_score.cmp(&left.confidence_score))
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
        assert!(single_approved_candidate(&candidates).is_some());
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
        assert!(single_approved_candidate(&first).is_some());
        let disappeared = classify(&[], &preferences);
        assert!(single_approved_candidate(&disappeared).is_none());
        let reappeared = classify(&live, &preferences);
        assert!(single_approved_candidate(&reappeared).is_some());
    }

    #[test]
    fn more_than_one_approved_live_target_never_auto_arms() {
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
        assert!(single_approved_candidate(&candidates).is_none());
    }
}
