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

const UI_PREFERENCES_SCHEMA_VERSION: u32 = 3;

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
    pub game_detection_enabled: bool,
    pub game_auto_arm: bool,
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
            game_detection_enabled: false,
            game_auto_arm: false,
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
    pub game_detection_enabled: Option<bool>,
    pub game_auto_arm: Option<bool>,
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
    if let Some(next) = patch.game_detection_enabled {
        value.game_detection_enabled = next;
    }
    if let Some(next) = patch.game_auto_arm {
        value.game_auto_arm = next;
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
        Ok(bytes) => serde_json::from_slice::<UiPreferences>(&bytes)
            .map(UiPreferences::normalized)
            .map_err(|error| format!("Could not parse '{}': {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(UiPreferences::default()),
        Err(error) => Err(format!("Could not read '{}': {error}", path.display())),
    }
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
        assert_eq!(load_preferences(&path).unwrap(), UiPreferences::default());
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
        assert!(!upgraded.game_detection_enabled);
        assert!(!upgraded.game_auto_arm);
        assert!(upgraded.game_detection_approved_processes.is_empty());
        assert!(upgraded.game_detection_excluded_processes.is_empty());
        fs::remove_dir_all(root).unwrap();
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
            game_detection_enabled: Some(true),
            game_auto_arm: Some(true),
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
        assert!(loaded.game_detection_enabled);
        assert!(loaded.game_auto_arm);
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
}
