use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde::{Deserialize, Serialize};
use tauri::State;
use uuid::Uuid;
use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    MoveFileExW, MOVEFILE_REPLACE_EXISTING, MOVEFILE_WRITE_THROUGH,
};

use crate::capture::compatibility::CaptureMode;

const UI_PREFERENCES_SCHEMA_VERSION: u32 = 9;

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum GameDetectionMode {
    #[default]
    AnyDetectedGame,
    ApprovedGamesOnly,
}

#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiPreferences {
    pub schema_version: u32,
    pub player_volume: f64,
    pub player_muted: bool,
    pub player_last_audible_volume: f64,
    pub clips_sort: String,
    pub clips_favorites_only: bool,
    pub clips_view: String,
    pub clips_grid_size: String,
    pub clips_search_query: String,
    pub selected_collection_id: Option<String>,
    pub start_with_windows: bool,
    pub close_to_tray: bool,
    pub save_overlay_enabled: bool,
    pub save_replay_hotkey: String,
    pub save_and_name_hotkey: Option<String>,
    pub storage_quota_gib: u32,
    pub capture_mode: CaptureMode,
    pub replay_duration_seconds: u32,
    pub replay_frame_rate: u32,
    pub replay_quality: String,
    pub replay_encoder: String,
    pub game_detection_enabled: bool,
    pub game_auto_arm: bool,
    pub game_detection_mode: GameDetectionMode,
    pub game_stop_replay_on_close: bool,
    pub game_ready_notification_enabled: bool,
    pub game_detection_approved_processes: Vec<String>,
    pub game_detection_excluded_processes: Vec<String>,
}

impl Default for UiPreferences {
    fn default() -> Self {
        Self {
            schema_version: UI_PREFERENCES_SCHEMA_VERSION,
            player_volume: 1.0,
            player_muted: false,
            player_last_audible_volume: 1.0,
            clips_sort: "newestFirst".into(),
            clips_favorites_only: false,
            clips_view: "all".into(),
            clips_grid_size: "comfortable".into(),
            clips_search_query: String::new(),
            selected_collection_id: None,
            start_with_windows: false,
            close_to_tray: true,
            save_overlay_enabled: true,
            save_replay_hotkey: crate::hotkey::DEFAULT_SAVE_REPLAY_HOTKEY.to_string(),
            save_and_name_hotkey: None,
            storage_quota_gib: 50,
            capture_mode: CaptureMode::Auto,
            replay_duration_seconds: 120,
            replay_frame_rate: 60,
            replay_quality: "balanced".into(),
            replay_encoder: "automatic".into(),
            game_detection_enabled: true,
            game_auto_arm: true,
            game_detection_mode: GameDetectionMode::AnyDetectedGame,
            game_stop_replay_on_close: true,
            game_ready_notification_enabled: true,
            game_detection_approved_processes: Vec::new(),
            game_detection_excluded_processes: Vec::new(),
        }
    }
}

impl UiPreferences {
    fn normalized(mut self) -> Self {
        self.schema_version = UI_PREFERENCES_SCHEMA_VERSION;
        self.player_volume = clamp_volume(self.player_volume, 1.0);
        self.player_last_audible_volume = clamp_volume(self.player_last_audible_volume, 1.0);
        if self.player_last_audible_volume <= 0.0 {
            self.player_last_audible_volume = 1.0;
        }
        if !matches!(
            self.clips_sort.as_str(),
            "newestFirst"
                | "oldestFirst"
                | "nameAscending"
                | "nameDescending"
                | "longestFirst"
                | "shortestFirst"
                | "largestFirst"
                | "smallestFirst"
                | "mostPlayed"
                | "recentlyWatched"
        ) {
            self.clips_sort = "newestFirst".into();
        }
        if !matches!(
            self.clips_view.as_str(),
            "all" | "favorites" | "recentlyWatched"
        ) {
            self.clips_view = "all".into();
        }
        if !matches!(
            self.clips_grid_size.as_str(),
            "compact" | "comfortable" | "large"
        ) {
            self.clips_grid_size = "comfortable".into();
        }
        self.clips_search_query.truncate(500);
        self.storage_quota_gib = self.storage_quota_gib.clamp(1, 10 * 1024);
        if !matches!(self.replay_duration_seconds, 30 | 60 | 120 | 180 | 300) {
            self.replay_duration_seconds = 120;
        }
        if !matches!(self.replay_frame_rate, 30 | 60) {
            self.replay_frame_rate = 60;
        }
        if !matches!(
            self.replay_quality.as_str(),
            "high" | "balanced" | "smallerFiles"
        ) {
            self.replay_quality = "balanced".into();
        }
        if !matches!(self.replay_encoder.as_str(), "automatic" | "hevc" | "h264") {
            self.replay_encoder = "automatic".into();
        }
        self.save_and_name_hotkey = self
            .save_and_name_hotkey
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.selected_collection_id = self
            .selected_collection_id
            .take()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        self.game_detection_approved_processes =
            normalize_process_list(self.game_detection_approved_processes);
        self.game_detection_excluded_processes =
            normalize_process_list(self.game_detection_excluded_processes);
        let exclusions = self
            .game_detection_excluded_processes
            .iter()
            .map(|value| value.to_lowercase())
            .collect::<std::collections::BTreeSet<_>>();
        self.game_detection_approved_processes
            .retain(|value| !exclusions.contains(&value.to_lowercase()));
        if !self.game_detection_enabled {
            self.game_auto_arm = false;
        }
        self
    }
}

fn normalize_process_list(values: Vec<String>) -> Vec<String> {
    let mut seen = std::collections::BTreeSet::new();
    values
        .into_iter()
        .map(|value| {
            let trimmed = value.trim();
            if trimmed.to_lowercase().ends_with(".exe") {
                trimmed[..trimmed.len() - 4].trim().to_string()
            } else {
                trimmed.to_string()
            }
        })
        .filter(|value| !value.is_empty() && value.len() <= 120)
        .filter(|value| seen.insert(value.to_lowercase()))
        .take(100)
        .collect()
}

fn clamp_volume(value: f64, fallback: f64) -> f64 {
    if value.is_finite() {
        value.clamp(0.0, 1.0)
    } else {
        fallback
    }
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UiPreferencesPatch {
    pub player_volume: Option<f64>,
    pub player_muted: Option<bool>,
    pub player_last_audible_volume: Option<f64>,
    pub clips_sort: Option<String>,
    pub clips_favorites_only: Option<bool>,
    pub clips_view: Option<String>,
    pub clips_grid_size: Option<String>,
    pub clips_search_query: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub selected_collection_id: Option<Option<String>>,
    pub start_with_windows: Option<bool>,
    pub close_to_tray: Option<bool>,
    pub save_overlay_enabled: Option<bool>,
    pub save_replay_hotkey: Option<String>,
    #[serde(default, deserialize_with = "deserialize_nullable_field")]
    pub save_and_name_hotkey: Option<Option<String>>,
    pub storage_quota_gib: Option<u32>,
    pub capture_mode: Option<CaptureMode>,
    pub replay_duration_seconds: Option<u32>,
    pub replay_frame_rate: Option<u32>,
    pub replay_quality: Option<String>,
    pub replay_encoder: Option<String>,
    pub game_detection_enabled: Option<bool>,
    pub game_auto_arm: Option<bool>,
    pub game_detection_mode: Option<GameDetectionMode>,
    pub game_stop_replay_on_close: Option<bool>,
    pub game_ready_notification_enabled: Option<bool>,
    pub game_detection_approved_processes: Option<Vec<String>>,
    pub game_detection_excluded_processes: Option<Vec<String>>,
}

fn deserialize_nullable_field<'de, D>(deserializer: D) -> Result<Option<Option<String>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<String>::deserialize(deserializer).map(Some)
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UiPreferencesResponse {
    pub success: bool,
    pub preferences: UiPreferences,
    pub error_message: Option<String>,
}

pub struct UiPreferencesManager {
    path: PathBuf,
    preferences: Mutex<UiPreferences>,
}

impl UiPreferencesManager {
    pub fn initialize(path: PathBuf) -> Self {
        let preferences = match load_preferences(&path) {
            Ok(value) => value,
            Err(error) => {
                eprintln!("SlickClip UI preferences warning: {error}");
                UiPreferences::default()
            }
        };
        Self {
            path,
            preferences: Mutex::new(preferences),
        }
    }

    fn lock(&self) -> MutexGuard<'_, UiPreferences> {
        self.preferences
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn get(&self) -> UiPreferencesResponse {
        UiPreferencesResponse {
            success: true,
            preferences: self.lock().clone(),
            error_message: None,
        }
    }

    pub(crate) fn with_current<R>(&self, callback: impl FnOnce(&UiPreferences) -> R) -> R {
        let preferences = self.lock();
        callback(&preferences)
    }

    pub fn save_replay_hotkey(&self, combination: String) -> Result<(), String> {
        let response = self.update(UiPreferencesPatch {
            save_replay_hotkey: Some(combination),
            ..Default::default()
        });
        if response.success {
            Ok(())
        } else {
            Err(response.error_message.unwrap_or_else(|| {
                "The Save Replay hotkey preference could not be saved.".to_string()
            }))
        }
    }

    pub fn save_and_name_hotkey(&self, combination: Option<String>) -> Result<(), String> {
        let response = self.update(UiPreferencesPatch {
            save_and_name_hotkey: Some(combination),
            ..Default::default()
        });
        if response.success {
            Ok(())
        } else {
            Err(response.error_message.unwrap_or_else(|| {
                "The Save & Name hotkey preference could not be saved.".to_string()
            }))
        }
    }

    pub fn update(&self, patch: UiPreferencesPatch) -> UiPreferencesResponse {
        let mut preferences = self.lock();
        let current = preferences.clone();
        let next = apply_patch(current.clone(), patch).normalized();
        match save_preferences(&self.path, &next) {
            Ok(()) => {
                *preferences = next.clone();
                UiPreferencesResponse {
                    success: true,
                    preferences: next,
                    error_message: None,
                }
            }
            Err(error) => {
                eprintln!("SlickClip UI preferences warning: {error}");
                UiPreferencesResponse {
                    success: false,
                    preferences: current,
                    error_message: Some(error),
                }
            }
        }
    }
}

fn apply_patch(mut value: UiPreferences, patch: UiPreferencesPatch) -> UiPreferences {
    if let Some(next) = patch.player_volume {
        value.player_volume = next;
    }
    if let Some(next) = patch.player_muted {
        value.player_muted = next;
    }
    if let Some(next) = patch.player_last_audible_volume {
        value.player_last_audible_volume = next;
    }
    if let Some(next) = patch.clips_sort {
        value.clips_sort = next;
    }
    if let Some(next) = patch.clips_favorites_only {
        value.clips_favorites_only = next;
    }
    if let Some(next) = patch.clips_view {
        value.clips_view = next;
    }
    if let Some(next) = patch.clips_grid_size {
        value.clips_grid_size = next;
    }
    if let Some(next) = patch.clips_search_query {
        value.clips_search_query = next;
    }
    if let Some(next) = patch.selected_collection_id {
        value.selected_collection_id = next;
    }
    if let Some(next) = patch.start_with_windows {
        value.start_with_windows = next;
    }
    if let Some(next) = patch.close_to_tray {
        value.close_to_tray = next;
    }
    if let Some(next) = patch.save_overlay_enabled {
        value.save_overlay_enabled = next;
    }
    if let Some(next) = patch.save_replay_hotkey {
        value.save_replay_hotkey = next;
    }
    if let Some(next) = patch.save_and_name_hotkey {
        value.save_and_name_hotkey = next;
    }
    if let Some(next) = patch.storage_quota_gib {
        value.storage_quota_gib = next;
    }
    if let Some(next) = patch.capture_mode {
        value.capture_mode = next;
    }
    if let Some(next) = patch.replay_duration_seconds {
        value.replay_duration_seconds = next;
    }
    if let Some(next) = patch.replay_frame_rate {
        value.replay_frame_rate = next;
    }
    if let Some(next) = patch.replay_quality {
        value.replay_quality = next;
    }
    if let Some(next) = patch.replay_encoder {
        value.replay_encoder = next;
    }
    if let Some(next) = patch.game_detection_enabled {
        value.game_detection_enabled = next;
    }
    if let Some(next) = patch.game_auto_arm {
        value.game_auto_arm = next;
    }
    if let Some(next) = patch.game_detection_mode {
        value.game_detection_mode = next;
    }
    if let Some(next) = patch.game_stop_replay_on_close {
        value.game_stop_replay_on_close = next;
    }
    if let Some(next) = patch.game_ready_notification_enabled {
        value.game_ready_notification_enabled = next;
    }
    if let Some(next) = patch.game_detection_approved_processes {
        value.game_detection_approved_processes = next;
    }
    if let Some(next) = patch.game_detection_excluded_processes {
        value.game_detection_excluded_processes = next;
    }
    value
}

fn load_preferences(path: &Path) -> Result<UiPreferences, String> {
    match fs::read(path) {
        Ok(bytes) => {
            let mut document = serde_json::from_slice::<serde_json::Value>(&bytes)
                .map_err(|error| format!("Could not parse '{}': {error}", path.display()))?;
            preserve_legacy_game_detection_defaults(&mut document);
            serde_json::from_value::<UiPreferences>(document)
                .map(UiPreferences::normalized)
                .map_err(|error| format!("Could not parse '{}': {error}", path.display()))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(UiPreferences::default()),
        Err(error) => Err(format!("Could not read '{}': {error}", path.display())),
    }
}

fn preserve_legacy_game_detection_defaults(document: &mut serde_json::Value) {
    let Some(object) = document.as_object_mut() else {
        return;
    };
    let legacy_detection_enabled = object
        .get("gameDetectionEnabled")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    let legacy_auto_arm = object
        .get("gameAutoArm")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);
    object
        .entry("gameDetectionEnabled")
        .or_insert(serde_json::Value::Bool(false));
    object
        .entry("gameAutoArm")
        .or_insert(serde_json::Value::Bool(false));
    object.entry("gameDetectionMode").or_insert_with(|| {
        serde_json::Value::String(
            if legacy_detection_enabled && legacy_auto_arm {
                "anyDetectedGame"
            } else {
                "approvedGamesOnly"
            }
            .into(),
        )
    });
    object
        .entry("gameStopReplayOnClose")
        .or_insert(serde_json::Value::Bool(true));
    object
        .entry("gameReadyNotificationEnabled")
        .or_insert(serde_json::Value::Bool(true));
}

fn wide_null(value: &OsStr) -> Vec<u16> {
    value.encode_wide().chain(std::iter::once(0)).collect()
}

fn save_preferences(path: &Path, preferences: &UiPreferences) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| "The UI preferences path has no parent directory.".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("Could not create the UI preferences directory: {error}"))?;
    let temporary = parent.join(format!(".ui-preferences-{}.tmp", Uuid::new_v4()));
    let bytes = serde_json::to_vec_pretty(preferences)
        .map_err(|error| format!("Could not serialize UI preferences: {error}"))?;
    let write_result = (|| {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|error| format!("Could not create a temporary preferences file: {error}"))?;
        file.write_all(&bytes)
            .map_err(|error| format!("Could not write UI preferences: {error}"))?;
        file.sync_all()
            .map_err(|error| format!("Could not flush UI preferences: {error}"))?;
        let source = wide_null(temporary.as_os_str());
        let destination = wide_null(path.as_os_str());
        unsafe {
            MoveFileExW(
                PCWSTR(source.as_ptr()),
                PCWSTR(destination.as_ptr()),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            )
        }
        .map_err(|error| format!("Could not atomically replace UI preferences: {error}"))
    })();
    if write_result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    write_result
}

#[tauri::command]
pub fn get_ui_preferences(manager: State<'_, UiPreferencesManager>) -> UiPreferencesResponse {
    manager.get()
}

#[tauri::command]
pub fn update_ui_preferences(
    manager: State<'_, UiPreferencesManager>,
    mut patch: UiPreferencesPatch,
) -> UiPreferencesResponse {
    // The registry entry and this preference are kept together by
    // desktop::set_start_with_windows.
    patch.start_with_windows = None;
    // The hotkey preference changes only after the global registration succeeds.
    patch.save_replay_hotkey = None;
    patch.save_and_name_hotkey = None;
    manager.update(patch)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn directory(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("slickclip-stage18-prefs-{name}-{}", Uuid::new_v4()))
    }

    #[test]
    fn absent_malformed_and_unknown_fields_are_safe() {
        let root = directory("load");
        let path = root.join("ui-preferences.json");
        let fresh = load_preferences(&path).unwrap();
        assert_eq!(fresh, UiPreferences::default());
        assert!(fresh.game_detection_enabled);
        assert!(fresh.game_auto_arm);
        assert_eq!(
            fresh.game_detection_mode,
            GameDetectionMode::AnyDetectedGame
        );
        assert!(fresh.game_stop_replay_on_close);
        assert!(fresh.game_ready_notification_enabled);
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, b"not json").unwrap();
        assert!(load_preferences(&path).is_err());
        fs::write(
            &path,
            br#"{"schemaVersion":1,"playerVolume":0.37,"futureField":true}"#,
        )
        .unwrap();
        let upgraded = load_preferences(&path).unwrap();
        assert_eq!(upgraded.schema_version, UI_PREFERENCES_SCHEMA_VERSION);
        assert_eq!(upgraded.player_volume, 0.37);
        assert!(!upgraded.start_with_windows);
        assert!(upgraded.close_to_tray);
        assert!(upgraded.save_overlay_enabled);
        assert_eq!(
            upgraded.save_replay_hotkey,
            crate::hotkey::DEFAULT_SAVE_REPLAY_HOTKEY
        );
        assert_eq!(upgraded.save_and_name_hotkey, None);
        assert_eq!(upgraded.storage_quota_gib, 50);
        assert_eq!(upgraded.capture_mode, CaptureMode::Auto);
        assert_eq!(upgraded.replay_duration_seconds, 120);
        assert_eq!(upgraded.replay_frame_rate, 60);
        assert_eq!(upgraded.replay_quality, "balanced");
        assert_eq!(upgraded.replay_encoder, "automatic");
        assert!(!upgraded.game_detection_enabled);
        assert!(!upgraded.game_auto_arm);
        assert_eq!(
            upgraded.game_detection_mode,
            GameDetectionMode::ApprovedGamesOnly
        );
        assert!(upgraded.game_stop_replay_on_close);
        assert!(upgraded.game_ready_notification_enabled);
        assert!(upgraded.game_detection_approved_processes.is_empty());
        assert!(upgraded.game_detection_excluded_processes.is_empty());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_enabled_automatic_profile_migrates_to_any_detected_game() {
        let root = directory("game-detection-migration");
        let path = root.join("ui-preferences.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &path,
            br#"{"schemaVersion":5,"gameDetectionEnabled":true,"gameAutoArm":true,"gameDetectionApprovedProcesses":["KeptGame.exe"],"gameDetectionExcludedProcesses":["NeverCapture.exe"]}"#,
        )
        .unwrap();
        let loaded = load_preferences(&path).unwrap();
        assert!(loaded.game_detection_enabled);
        assert!(loaded.game_auto_arm);
        assert_eq!(
            loaded.game_detection_mode,
            GameDetectionMode::AnyDetectedGame
        );
        assert_eq!(loaded.game_detection_approved_processes, vec!["KeptGame"]);
        assert_eq!(
            loaded.game_detection_excluded_processes,
            vec!["NeverCapture"]
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_auto_arm_off_remains_off() {
        let root = directory("game-auto-arm-off-migration");
        let path = root.join("ui-preferences.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &path,
            br#"{"schemaVersion":5,"gameDetectionEnabled":true,"gameAutoArm":false}"#,
        )
        .unwrap();
        let loaded = load_preferences(&path).unwrap();
        assert!(loaded.game_detection_enabled);
        assert!(!loaded.game_auto_arm);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn legacy_detection_off_remains_off() {
        let root = directory("game-detection-off-migration");
        let path = root.join("ui-preferences.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(
            &path,
            br#"{"schemaVersion":5,"gameDetectionEnabled":false,"gameAutoArm":true}"#,
        )
        .unwrap();
        let loaded = load_preferences(&path).unwrap();
        assert!(!loaded.game_detection_enabled);
        assert!(!loaded.game_auto_arm);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn explicit_v6_detection_modes_survive_later_launches() {
        for (name, serialized_mode, expected_mode) in [
            ("any", "anyDetectedGame", GameDetectionMode::AnyDetectedGame),
            (
                "strict",
                "approvedGamesOnly",
                GameDetectionMode::ApprovedGamesOnly,
            ),
        ] {
            let root = directory(&format!("explicit-v6-mode-{name}"));
            let path = root.join("ui-preferences.json");
            fs::create_dir_all(&root).unwrap();
            fs::write(
                &path,
                format!(
                    r#"{{"schemaVersion":6,"gameDetectionEnabled":true,"gameAutoArm":true,"gameDetectionMode":"{serialized_mode}"}}"#
                ),
            )
            .unwrap();
            let first_launch = load_preferences(&path).unwrap();
            assert_eq!(first_launch.game_detection_mode, expected_mode);
            save_preferences(&path, &first_launch).unwrap();
            let later_launch = load_preferences(&path).unwrap();
            assert_eq!(later_launch.game_detection_mode, expected_mode);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn capture_mode_defaults_to_auto_and_explicit_choices_round_trip() {
        for (name, serialized_mode, expected_mode) in [
            ("auto", "auto", CaptureMode::Auto),
            ("game", "gameCapture", CaptureMode::GameCapture),
            ("screen", "screenCapture", CaptureMode::ScreenCapture),
        ] {
            let root = directory(&format!("capture-mode-{name}"));
            let path = root.join("ui-preferences.json");
            fs::create_dir_all(&root).unwrap();
            fs::write(
                &path,
                format!(r#"{{"schemaVersion":7,"captureMode":"{serialized_mode}"}}"#),
            )
            .unwrap();
            let loaded = load_preferences(&path).unwrap();
            assert_eq!(loaded.capture_mode, expected_mode);
            save_preferences(&path, &loaded).unwrap();
            assert_eq!(load_preferences(&path).unwrap().capture_mode, expected_mode);
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn round_trip_clamps_volume_and_preserves_unicode_and_library_choices() {
        let root = directory("roundtrip");
        let path = root.join("ui-preferences.json");
        let manager = UiPreferencesManager::initialize(path.clone());
        let response = manager.update(UiPreferencesPatch {
            player_volume: Some(4.0),
            player_muted: Some(true),
            player_last_audible_volume: Some(0.37),
            clips_sort: Some("mostPlayed".into()),
            clips_favorites_only: Some(true),
            clips_view: Some("favorites".into()),
            clips_grid_size: Some("compact".into()),
            clips_search_query: Some("雪 gta".into()),
            selected_collection_id: Some(Some("collection-1".into())),
            start_with_windows: Some(true),
            close_to_tray: Some(false),
            save_overlay_enabled: Some(false),
            save_replay_hotkey: Some("Shift + Numpad0".into()),
            save_and_name_hotkey: Some(Some("Ctrl + Shift + F11".into())),
            storage_quota_gib: Some(12_000),
            capture_mode: Some(CaptureMode::ScreenCapture),
            replay_duration_seconds: Some(300),
            replay_frame_rate: Some(30),
            replay_quality: Some("high".into()),
            replay_encoder: Some("hevc".into()),
            game_detection_enabled: Some(true),
            game_auto_arm: Some(true),
            game_detection_mode: Some(GameDetectionMode::AnyDetectedGame),
            game_stop_replay_on_close: Some(false),
            game_ready_notification_enabled: Some(false),
            game_detection_approved_processes: Some(vec![
                " Game.exe ".into(),
                "game".into(),
                "Other".into(),
            ]),
            game_detection_excluded_processes: Some(vec!["other.exe".into()]),
        });
        assert!(response.success);
        assert_eq!(response.preferences.player_volume, 1.0);
        let loaded = load_preferences(&path).unwrap();
        assert!(loaded.player_muted);
        assert_eq!(loaded.player_last_audible_volume, 0.37);
        assert_eq!(loaded.clips_sort, "mostPlayed");
        assert!(loaded.clips_favorites_only);
        assert_eq!(loaded.clips_view, "favorites");
        assert_eq!(loaded.clips_grid_size, "compact");
        assert_eq!(loaded.clips_search_query, "雪 gta");
        assert!(loaded.start_with_windows);
        assert!(!loaded.close_to_tray);
        assert!(!loaded.save_overlay_enabled);
        assert_eq!(loaded.save_replay_hotkey, "Shift + Numpad0");
        assert_eq!(
            loaded.save_and_name_hotkey.as_deref(),
            Some("Ctrl + Shift + F11")
        );
        assert_eq!(loaded.storage_quota_gib, 10 * 1024);
        assert_eq!(loaded.capture_mode, CaptureMode::ScreenCapture);
        assert_eq!(loaded.replay_duration_seconds, 300);
        assert_eq!(loaded.replay_frame_rate, 30);
        assert_eq!(loaded.replay_quality, "high");
        assert_eq!(loaded.replay_encoder, "hevc");
        assert!(loaded.game_detection_enabled);
        assert!(loaded.game_auto_arm);
        assert_eq!(
            loaded.game_detection_mode,
            GameDetectionMode::AnyDetectedGame
        );
        assert!(!loaded.game_stop_replay_on_close);
        assert!(!loaded.game_ready_notification_enabled);
        assert_eq!(loaded.game_detection_approved_processes, vec!["Game"]);
        assert_eq!(loaded.game_detection_excluded_processes, vec!["other"]);
        assert_eq!(
            loaded.selected_collection_id.as_deref(),
            Some("collection-1")
        );
        assert_eq!(fs::read_dir(&root).unwrap().count(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn concurrent_partial_updates_preserve_unrelated_fields() {
        let root = directory("concurrent");
        let path = root.join("ui-preferences.json");
        let manager = Arc::new(UiPreferencesManager::initialize(path.clone()));
        let first = Arc::clone(&manager);
        let volume = std::thread::spawn(move || {
            first.update(UiPreferencesPatch {
                player_volume: Some(0.37),
                ..Default::default()
            })
        });
        let second = Arc::clone(&manager);
        let grid = std::thread::spawn(move || {
            second.update(UiPreferencesPatch {
                clips_grid_size: Some("large".into()),
                ..Default::default()
            })
        });
        assert!(volume.join().unwrap().success);
        assert!(grid.join().unwrap().success);
        let saved = load_preferences(&path).unwrap();
        assert_eq!(saved.player_volume, 0.37);
        assert_eq!(saved.clips_grid_size, "large");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn malformed_manager_falls_back_and_nullable_collection_patch_can_clear() {
        let root = directory("fallback");
        let path = root.join("ui-preferences.json");
        fs::create_dir_all(&root).unwrap();
        fs::write(&path, b"{broken").unwrap();
        assert_eq!(
            UiPreferencesManager::initialize(path).get().preferences,
            UiPreferences::default()
        );

        let missing: UiPreferencesPatch = serde_json::from_str("{}").unwrap();
        assert_eq!(missing.selected_collection_id, None);
        let cleared: UiPreferencesPatch =
            serde_json::from_str(r#"{"selectedCollectionId":null}"#).unwrap();
        assert_eq!(cleared.selected_collection_id, Some(None));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn save_replay_hotkey_persists_across_manager_restart() {
        let root = directory("hotkey");
        let path = root.join("ui-preferences.json");
        let manager = UiPreferencesManager::initialize(path.clone());
        manager
            .save_replay_hotkey("F8".to_string())
            .expect("hotkey preference should save");
        drop(manager);

        let restarted = UiPreferencesManager::initialize(path);
        assert_eq!(restarted.get().preferences.save_replay_hotkey, "F8");
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn optional_save_and_name_hotkey_persists_and_can_be_disabled() {
        let root = directory("save-and-name-hotkey");
        let path = root.join("ui-preferences.json");
        let manager = UiPreferencesManager::initialize(path.clone());
        manager
            .save_and_name_hotkey(Some("Ctrl + Shift + F11".to_string()))
            .unwrap();
        drop(manager);

        let restarted = UiPreferencesManager::initialize(path.clone());
        assert_eq!(
            restarted.get().preferences.save_and_name_hotkey.as_deref(),
            Some("Ctrl + Shift + F11")
        );
        restarted.save_and_name_hotkey(None).unwrap();
        drop(restarted);
        assert_eq!(
            UiPreferencesManager::initialize(path)
                .get()
                .preferences
                .save_and_name_hotkey,
            None
        );
        fs::remove_dir_all(root).unwrap();
    }
}
