use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::clips::{ClipSaveManager, SaveJobState};
use crate::preferences::UiPreferencesManager;

pub const DEFAULT_SAVE_REPLAY_HOTKEY: &str = "Ctrl + Shift + F10";
const HOTKEY_TEST_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyState {
    pub registered: bool,
    pub current_combination: String,
    pub last_registration_error: Option<String>,
    pub testing: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyCommandResult {
    pub success: bool,
    pub state: HotkeyState,
    pub error_message: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HotkeySaveFeedback {
    success: bool,
    message: String,
    save_state: SaveJobState,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct HotkeyTestFeedback {
    success: bool,
    message: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ShortcutAction {
    Ignore,
    SaveReplay,
    TestSucceeded,
}

struct ActiveTest {
    generation: u64,
    deadline: Instant,
}

struct ParsedHotkey {
    normalized: String,
    shortcut: Shortcut,
}

struct HotkeyInner {
    state: HotkeyState,
    shortcut: Shortcut,
    recorder_active: bool,
    active_test: Option<ActiveTest>,
    next_test_generation: u64,
}

pub struct SaveReplayHotkeyManager {
    inner: Mutex<HotkeyInner>,
    repair_persisted_combination: bool,
}

impl SaveReplayHotkeyManager {
    pub fn new(initial_combination: &str) -> Self {
        let (parsed, initial_error, repair_persisted_combination) =
            match parse_hotkey(initial_combination) {
                Ok(parsed) => (parsed, None, false),
                Err(error) => (
                    parse_hotkey(DEFAULT_SAVE_REPLAY_HOTKEY)
                        .expect("the built-in Save Replay hotkey must remain valid"),
                    Some(format!(
                        "The saved Save Replay hotkey was invalid ({error}). SlickClip restored the default."
                    )),
                    true,
                ),
            };
        Self {
            inner: Mutex::new(HotkeyInner {
                state: HotkeyState {
                    registered: false,
                    current_combination: parsed.normalized,
                    last_registration_error: initial_error,
                    testing: false,
                },
                shortcut: parsed.shortcut,
                recorder_active: false,
                active_test: None,
                next_test_generation: 1,
            }),
            repair_persisted_combination,
        }
    }

    fn lock(&self) -> MutexGuard<'_, HotkeyInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn register_initial(&self, app: &AppHandle, preferences: &UiPreferencesManager) {
        let mut inner = self.lock();
        match app.global_shortcut().register(inner.shortcut) {
            Ok(()) => {
                inner.state.registered = true;
                if self.repair_persisted_combination {
                    let combination = inner.state.current_combination.clone();
                    if let Err(error) = preferences.save_replay_hotkey(combination) {
                        inner.state.last_registration_error = Some(format!(
                            "The default Save Replay hotkey is registered, but the repaired preference could not be saved: {error}"
                        ));
                    }
                } else {
                    inner.state.last_registration_error = None;
                }
            }
            Err(error) => {
                let unavailable_combination = inner.state.current_combination.clone();
                if unavailable_combination != DEFAULT_SAVE_REPLAY_HOTKEY {
                    let fallback = parse_hotkey(DEFAULT_SAVE_REPLAY_HOTKEY)
                        .expect("the built-in Save Replay hotkey must remain valid");
                    if app.global_shortcut().register(fallback.shortcut).is_ok() {
                        inner.shortcut = fallback.shortcut;
                        inner.state.registered = true;
                        inner.state.current_combination = fallback.normalized;
                        let mut message = format!(
                            "Could not register the saved hotkey {unavailable_combination}: {error}. SlickClip restored {DEFAULT_SAVE_REPLAY_HOTKEY}."
                        );
                        if let Err(persist_error) =
                            preferences.save_replay_hotkey(inner.state.current_combination.clone())
                        {
                            message.push_str(&format!(
                                " The fallback preference could not be saved: {persist_error}"
                            ));
                        }
                        inner.state.last_registration_error = Some(message);
                        return;
                    }
                }
                inner.state.registered = false;
                inner.state.last_registration_error =
                    Some(format_registration_error(&unavailable_combination, error));
            }
        }
    }

    pub fn state(&self) -> HotkeyState {
        self.lock().state.clone()
    }

    pub fn set_recorder_active(&self, active: bool) -> HotkeyState {
        let mut inner = self.lock();
        inner.recorder_active = active;
        if active {
            clear_test(&mut inner);
        }
        inner.state.clone()
    }

    pub fn rebind(
        &self,
        app: &AppHandle,
        preferences: &UiPreferencesManager,
        combination: &str,
    ) -> HotkeyCommandResult {
        self.rebind_with_operations(
            combination,
            |shortcut| {
                app.global_shortcut()
                    .register(shortcut)
                    .map_err(|error| error.to_string())
            },
            |shortcut| {
                app.global_shortcut()
                    .unregister(shortcut)
                    .map_err(|error| error.to_string())
            },
            |combination| preferences.save_replay_hotkey(combination.to_string()),
        )
    }

    fn rebind_with_operations<Register, Unregister, Persist>(
        &self,
        combination: &str,
        mut register: Register,
        mut unregister: Unregister,
        mut persist: Persist,
    ) -> HotkeyCommandResult
    where
        Register: FnMut(Shortcut) -> Result<(), String>,
        Unregister: FnMut(Shortcut) -> Result<(), String>,
        Persist: FnMut(&str) -> Result<(), String>,
    {
        let parsed = match parse_hotkey(combination) {
            Ok(parsed) => parsed,
            Err(error) => {
                let mut inner = self.lock();
                inner.recorder_active = false;
                clear_test(&mut inner);
                return HotkeyCommandResult {
                    success: false,
                    state: inner.state.clone(),
                    error_message: Some(error),
                };
            }
        };

        let mut inner = self.lock();
        clear_test(&mut inner);
        if parsed.shortcut == inner.shortcut && inner.state.registered {
            inner.recorder_active = false;
            inner.state.last_registration_error = None;
            if let Err(error) = persist(&inner.state.current_combination) {
                let message = format!(
                    "The hotkey is registered, but its preference could not be saved: {error}"
                );
                inner.state.last_registration_error = Some(message.clone());
                return HotkeyCommandResult {
                    success: false,
                    state: inner.state.clone(),
                    error_message: Some(message),
                };
            }
            return HotkeyCommandResult {
                success: true,
                state: inner.state.clone(),
                error_message: None,
            };
        }

        if let Err(error) = register(parsed.shortcut) {
            let message = format_registration_error(&parsed.normalized, error);
            inner.recorder_active = false;
            inner.state.last_registration_error = Some(message.clone());
            return HotkeyCommandResult {
                success: false,
                state: inner.state.clone(),
                error_message: Some(message),
            };
        }

        if let Err(error) = persist(&parsed.normalized) {
            let _ = unregister(parsed.shortcut);
            let message = format!(
                "{0} was available, but SlickClip could not save the preference. The previous hotkey remains active: {error}",
                parsed.normalized
            );
            inner.recorder_active = false;
            inner.state.last_registration_error = Some(message.clone());
            return HotkeyCommandResult {
                success: false,
                state: inner.state.clone(),
                error_message: Some(message),
            };
        }

        if inner.state.registered {
            if let Err(error) = unregister(inner.shortcut) {
                let rollback_error = persist(&inner.state.current_combination).err();
                let _ = unregister(parsed.shortcut);
                let mut message = format!(
                    "The new hotkey was available, but the previous hotkey could not be unregistered: {error}. The previous hotkey remains active."
                );
                if let Some(rollback_error) = rollback_error {
                    message.push_str(&format!(
                        " The saved preference also could not be restored: {rollback_error}"
                    ));
                }
                inner.recorder_active = false;
                inner.state.last_registration_error = Some(message.clone());
                return HotkeyCommandResult {
                    success: false,
                    state: inner.state.clone(),
                    error_message: Some(message),
                };
            }
        }

        inner.shortcut = parsed.shortcut;
        inner.recorder_active = false;
        inner.state = HotkeyState {
            registered: true,
            current_combination: parsed.normalized,
            last_registration_error: None,
            testing: false,
        };
        HotkeyCommandResult {
            success: true,
            state: inner.state.clone(),
            error_message: None,
        }
    }

    fn shortcut_action(&self, shortcut: &Shortcut, now: Instant) -> ShortcutAction {
        let mut inner = self.lock();
        if !inner.state.registered || inner.recorder_active || shortcut != &inner.shortcut {
            return ShortcutAction::Ignore;
        }
        if let Some(test) = inner.active_test.take() {
            inner.state.testing = false;
            if now <= test.deadline {
                return ShortcutAction::TestSucceeded;
            }
        }
        ShortcutAction::SaveReplay
    }

    fn begin_test(&self, timeout: Duration) -> (HotkeyCommandResult, Option<u64>) {
        let mut inner = self.lock();
        if !inner.state.registered {
            let message = inner
                .state
                .last_registration_error
                .clone()
                .unwrap_or_else(|| "The Save Replay hotkey is not registered.".to_string());
            return (
                HotkeyCommandResult {
                    success: false,
                    state: inner.state.clone(),
                    error_message: Some(message),
                },
                None,
            );
        }
        if inner.recorder_active {
            return (
                HotkeyCommandResult {
                    success: false,
                    state: inner.state.clone(),
                    error_message: Some(
                        "Finish recording the new shortcut before testing it.".to_string(),
                    ),
                },
                None,
            );
        }
        let generation = inner.next_test_generation;
        inner.next_test_generation = inner.next_test_generation.wrapping_add(1).max(1);
        inner.active_test = Some(ActiveTest {
            generation,
            deadline: Instant::now() + timeout,
        });
        inner.state.testing = true;
        (
            HotkeyCommandResult {
                success: true,
                state: inner.state.clone(),
                error_message: None,
            },
            Some(generation),
        )
    }

    fn expire_test(&self, generation: u64) -> bool {
        let mut inner = self.lock();
        let matches = inner
            .active_test
            .as_ref()
            .is_some_and(|test| test.generation == generation && Instant::now() >= test.deadline);
        if matches {
            clear_test(&mut inner);
        }
        matches
    }

    pub fn cancel_test(&self) -> HotkeyState {
        let mut inner = self.lock();
        clear_test(&mut inner);
        inner.state.clone()
    }

    pub fn unregister(&self, app: &AppHandle) {
        let mut inner = self.lock();
        clear_test(&mut inner);
        if inner.state.registered {
            if let Err(error) = app.global_shortcut().unregister(inner.shortcut) {
                inner.state.last_registration_error = Some(format!(
                    "The Save Replay hotkey could not be unregistered during shutdown: {error}"
                ));
                return;
            }
            inner.state.registered = false;
        }
    }
}

fn clear_test(inner: &mut HotkeyInner) {
    inner.active_test = None;
    inner.state.testing = false;
}

pub fn handle_global_shortcut(app: &AppHandle, shortcut: &Shortcut, state: ShortcutState) {
    if state != ShortcutState::Pressed {
        return;
    }
    let Some(hotkey) = app.try_state::<SaveReplayHotkeyManager>() else {
        return;
    };
    match hotkey.shortcut_action(shortcut, Instant::now()) {
        ShortcutAction::Ignore => return,
        ShortcutAction::TestSucceeded => {
            let _ = app.emit(
                "save-replay-hotkey-test-result",
                HotkeyTestFeedback {
                    success: true,
                    message: "Hotkey detected ✓".to_string(),
                },
            );
            return;
        }
        ShortcutAction::SaveReplay => {}
    }
    request_save_with_feedback(app);
}

pub fn request_save_with_feedback(app: &AppHandle) {
    let Some(save_manager) = app.try_state::<ClipSaveManager>() else {
        return;
    };
    let result = save_manager.start();
    let message = if result.success {
        "Save Replay started.".to_string()
    } else {
        result
            .error_message
            .clone()
            .unwrap_or_else(|| "Save Replay could not start.".to_string())
    };
    let _ = app.emit(
        "save-replay-hotkey-feedback",
        HotkeySaveFeedback {
            success: result.success,
            message,
            save_state: result.status.state,
        },
    );
    crate::desktop::refresh_tray_status(app);
}

#[tauri::command]
pub fn get_save_replay_hotkey(manager: tauri::State<'_, SaveReplayHotkeyManager>) -> HotkeyState {
    manager.state()
}

#[tauri::command]
pub fn set_save_replay_hotkey(
    app: AppHandle,
    manager: tauri::State<'_, SaveReplayHotkeyManager>,
    preferences: tauri::State<'_, UiPreferencesManager>,
    combination: String,
) -> HotkeyCommandResult {
    manager.rebind(&app, &preferences, &combination)
}

#[tauri::command]
pub fn set_hotkey_recorder_active(
    manager: tauri::State<'_, SaveReplayHotkeyManager>,
    active: bool,
) -> HotkeyState {
    manager.set_recorder_active(active)
}

#[tauri::command]
pub fn begin_hotkey_test(
    app: AppHandle,
    manager: tauri::State<'_, SaveReplayHotkeyManager>,
) -> HotkeyCommandResult {
    let (result, generation) = manager.begin_test(HOTKEY_TEST_TIMEOUT);
    if let Some(generation) = generation {
        std::thread::spawn(move || {
            std::thread::sleep(HOTKEY_TEST_TIMEOUT);
            let Some(manager) = app.try_state::<SaveReplayHotkeyManager>() else {
                return;
            };
            if manager.expire_test(generation) {
                let _ = app.emit(
                    "save-replay-hotkey-test-result",
                    HotkeyTestFeedback {
                        success: false,
                        message: "Hotkey not detected. Check the binding or possible conflicts."
                            .to_string(),
                    },
                );
            }
        });
    }
    result
}

#[tauri::command]
pub fn cancel_hotkey_test(manager: tauri::State<'_, SaveReplayHotkeyManager>) -> HotkeyState {
    manager.cancel_test()
}

fn format_registration_error(combination: &str, error: impl std::fmt::Display) -> String {
    format!(
        "Could not register {combination}. Another application may already own this global hotkey: {error}"
    )
}

fn parse_hotkey(input: &str) -> Result<ParsedHotkey, String> {
    let mut modifiers = Modifiers::empty();
    let mut seen_modifiers = HashSet::new();
    let mut key = None;

    for raw_part in input.split('+') {
        let part = raw_part.trim();
        if part.is_empty() {
            return Err("Hotkey contains an empty key or modifier.".to_string());
        }
        let lower = part.to_ascii_lowercase();
        let modifier = match lower.as_str() {
            "ctrl" | "control" => Some(("Ctrl", Modifiers::CONTROL)),
            "shift" => Some(("Shift", Modifiers::SHIFT)),
            "alt" => Some(("Alt", Modifiers::ALT)),
            "win" | "windows" | "super" | "meta" => Some(("Win", Modifiers::SUPER)),
            _ => None,
        };
        if let Some((name, flag)) = modifier {
            if !seen_modifiers.insert(name) {
                return Err(format!(
                    "Hotkey contains the {name} modifier more than once."
                ));
            }
            modifiers |= flag;
            continue;
        }

        if key.is_some() {
            return Err("Hotkey must contain exactly one non-modifier key.".to_string());
        }
        key = Some(parse_key(part)?);
    }

    let (key_name, code) =
        key.ok_or_else(|| "Hotkey must include one non-modifier keyboard key.".to_string())?;

    let ordered = [
        ("Ctrl", Modifiers::CONTROL),
        ("Shift", Modifiers::SHIFT),
        ("Alt", Modifiers::ALT),
        ("Win", Modifiers::SUPER),
    ];
    let mut normalized = ordered
        .into_iter()
        .filter_map(|(name, flag)| modifiers.contains(flag).then_some(name))
        .collect::<Vec<_>>();
    normalized.push(&key_name);

    Ok(ParsedHotkey {
        normalized: normalized.join(" + "),
        shortcut: Shortcut::new((!modifiers.is_empty()).then_some(modifiers), code),
    })
}

fn parse_key(input: &str) -> Result<(String, Code), String> {
    let trimmed = input.trim();
    let upper = trimmed.to_ascii_uppercase();
    let canonical = if upper.len() == 1 && upper.as_bytes()[0].is_ascii_alphabetic() {
        format!("Key{upper}")
    } else if upper.len() == 1 && upper.as_bytes()[0].is_ascii_digit() {
        format!("Digit{upper}")
    } else if let Some(number) = upper
        .strip_prefix('F')
        .and_then(|value| value.parse::<u8>().ok())
    {
        if (1..=35).contains(&number) {
            format!("F{number}")
        } else {
            trimmed.to_string()
        }
    } else {
        match upper.as_str() {
            "ESC" | "ESCAPE" => "Escape".to_string(),
            "DEL" | "DELETE" => "Delete".to_string(),
            "INS" | "INSERT" => "Insert".to_string(),
            "PGUP" | "PAGEUP" => "PageUp".to_string(),
            "PGDN" | "PAGEDOWN" => "PageDown".to_string(),
            "UP" | "ARROWUP" => "ArrowUp".to_string(),
            "DOWN" | "ARROWDOWN" => "ArrowDown".to_string(),
            "LEFT" | "ARROWLEFT" => "ArrowLeft".to_string(),
            "RIGHT" | "ARROWRIGHT" => "ArrowRight".to_string(),
            "SPACE" => "Space".to_string(),
            "RETURN" | "ENTER" => "Enter".to_string(),
            "NUMENTER" | "NUMPADENTER" => "NumpadEnter".to_string(),
            _ => trimmed.to_string(),
        }
    };
    let code = canonical.parse::<Code>().map_err(|_| {
        format!(
            "Unsupported hotkey key '{input}'. Press a standard keyboard, function, navigation, punctuation, or numpad key."
        )
    })?;
    if matches!(
        code,
        Code::AltLeft
            | Code::AltRight
            | Code::ControlLeft
            | Code::ControlRight
            | Code::MetaLeft
            | Code::MetaRight
            | Code::ShiftLeft
            | Code::ShiftRight
            | Code::Fn
            | Code::FnLock
            | Code::Unidentified
    ) {
        return Err("Hotkey must include one non-modifier keyboard key.".to_string());
    }
    let code_name = code.to_string();
    let display_name = code_name
        .strip_prefix("Key")
        .or_else(|| code_name.strip_prefix("Digit"))
        .unwrap_or(&code_name)
        .to_string();
    Ok((display_name, code))
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, Instant};

    use super::{
        parse_hotkey, SaveReplayHotkeyManager, ShortcutAction, DEFAULT_SAVE_REPLAY_HOTKEY,
    };

    #[test]
    fn default_hotkey_is_valid() {
        let parsed = parse_hotkey(DEFAULT_SAVE_REPLAY_HOTKEY).unwrap();
        assert_eq!(parsed.normalized, "Ctrl + Shift + F10");
    }

    #[test]
    fn normalizes_aliases_and_modifier_order() {
        let parsed = parse_hotkey("meta + alt + control + k").unwrap();
        assert_eq!(parsed.normalized, "Ctrl + Alt + Win + K");
    }

    #[test]
    fn accepts_unmodified_function_alphanumeric_and_numpad_keys() {
        assert_eq!(parse_hotkey("F8").unwrap().normalized, "F8");
        assert_eq!(parse_hotkey("R").unwrap().normalized, "R");
        assert_eq!(parse_hotkey("5").unwrap().normalized, "5");
        assert_eq!(parse_hotkey("Numpad5").unwrap().normalized, "Numpad5");
        assert_eq!(
            parse_hotkey("Shift + Numpad0").unwrap().normalized,
            "Shift + Numpad0"
        );
    }

    #[test]
    fn accepts_broad_modified_shortcuts_and_rejects_modifier_only_bindings() {
        assert_eq!(
            parse_hotkey("Ctrl + Alt + R").unwrap().normalized,
            "Ctrl + Alt + R"
        );
        assert_eq!(parse_hotkey("Ctrl + F24").unwrap().normalized, "Ctrl + F24");
        assert_eq!(
            parse_hotkey("Alt + Semicolon").unwrap().normalized,
            "Alt + Semicolon"
        );
        assert!(parse_hotkey("Ctrl + Shift").is_err());
        assert!(parse_hotkey("ControlLeft").is_err());
    }

    #[test]
    fn rejects_duplicate_modifiers_and_multiple_keys() {
        assert!(parse_hotkey("Ctrl + Control + A").is_err());
        assert!(parse_hotkey("Ctrl + A + B").is_err());
    }

    #[test]
    fn rejects_empty_and_unsupported_combinations() {
        assert!(parse_hotkey("Ctrl++A").is_err());
        assert!(parse_hotkey("Ctrl + NotARealKey").is_err());
    }

    fn manager_with_registered_default() -> (SaveReplayHotkeyManager, super::ParsedHotkey) {
        let manager = SaveReplayHotkeyManager::new(DEFAULT_SAVE_REPLAY_HOTKEY);
        let parsed = parse_hotkey(DEFAULT_SAVE_REPLAY_HOTKEY).unwrap();
        {
            let mut inner = manager.lock();
            inner.state.registered = true;
        }
        (manager, parsed)
    }

    #[test]
    fn recorder_state_suppresses_the_registered_shortcut_without_os_input() {
        let (manager, parsed) = manager_with_registered_default();
        assert_eq!(
            manager.shortcut_action(&parsed.shortcut, Instant::now()),
            ShortcutAction::SaveReplay
        );

        manager.set_recorder_active(true);
        assert_eq!(
            manager.shortcut_action(&parsed.shortcut, Instant::now()),
            ShortcutAction::Ignore
        );
        manager.set_recorder_active(false);
        assert_eq!(
            manager.shortcut_action(&parsed.shortcut, Instant::now()),
            ShortcutAction::SaveReplay
        );
    }

    #[test]
    fn successful_test_consumes_exactly_one_shortcut_press() {
        let (manager, parsed) = manager_with_registered_default();
        let (result, _) = manager.begin_test(Duration::from_secs(10));
        assert!(result.success);
        assert!(result.state.testing);
        assert_eq!(
            manager.shortcut_action(&parsed.shortcut, Instant::now()),
            ShortcutAction::TestSucceeded
        );
        assert_eq!(
            manager.shortcut_action(&parsed.shortcut, Instant::now()),
            ShortcutAction::SaveReplay
        );
    }

    #[test]
    fn wrong_shortcut_does_not_complete_test_and_cancel_clears_it() {
        let (manager, _) = manager_with_registered_default();
        let wrong = parse_hotkey("Ctrl + Shift + F9").unwrap();
        manager.begin_test(Duration::from_secs(10));
        assert_eq!(
            manager.shortcut_action(&wrong.shortcut, Instant::now()),
            ShortcutAction::Ignore
        );
        assert!(manager.state().testing);
        assert!(!manager.cancel_test().testing);
    }

    #[test]
    fn timeout_clears_test_and_restores_normal_save_behavior() {
        let (manager, parsed) = manager_with_registered_default();
        let (_, generation) = manager.begin_test(Duration::ZERO);
        assert!(manager.expire_test(generation.unwrap()));
        assert!(!manager.state().testing);
        assert_eq!(
            manager.shortcut_action(&parsed.shortcut, Instant::now()),
            ShortcutAction::SaveReplay
        );
    }

    #[test]
    fn failed_registration_retains_the_previous_working_shortcut() {
        let (manager, _) = manager_with_registered_default();
        let persistence_attempts = std::cell::Cell::new(0);
        let result = manager.rebind_with_operations(
            "F8",
            |_| Err("already registered".to_string()),
            |_| Ok(()),
            |_| {
                persistence_attempts.set(persistence_attempts.get() + 1);
                Ok(())
            },
        );

        assert!(!result.success);
        assert_eq!(persistence_attempts.get(), 0);
        assert!(result.state.registered);
        assert_eq!(result.state.current_combination, DEFAULT_SAVE_REPLAY_HOTKEY);
        assert!(result.error_message.unwrap().contains("already registered"));
    }

    #[test]
    fn successful_rebind_persists_then_releases_the_previous_shortcut() {
        let (manager, _) = manager_with_registered_default();
        let operations = std::cell::RefCell::new(Vec::new());
        let result = manager.rebind_with_operations(
            "Shift + Numpad0",
            |_| {
                operations.borrow_mut().push("register");
                Ok(())
            },
            |_| {
                operations.borrow_mut().push("unregister");
                Ok(())
            },
            |combination| {
                assert_eq!(combination, "Shift + Numpad0");
                operations.borrow_mut().push("persist");
                Ok(())
            },
        );

        assert!(result.success);
        assert_eq!(result.state.current_combination, "Shift + Numpad0");
        assert_eq!(*operations.borrow(), ["register", "persist", "unregister"]);
    }

    #[test]
    fn persistence_failure_rolls_back_new_registration_and_keeps_old_state() {
        let (manager, _) = manager_with_registered_default();
        let unregister_count = std::cell::Cell::new(0);
        let result = manager.rebind_with_operations(
            "F9",
            |_| Ok(()),
            |_| {
                unregister_count.set(unregister_count.get() + 1);
                Ok(())
            },
            |_| Err("disk full".to_string()),
        );

        assert!(!result.success);
        assert_eq!(unregister_count.get(), 1);
        assert!(result.state.registered);
        assert_eq!(result.state.current_combination, DEFAULT_SAVE_REPLAY_HOTKEY);
    }

    #[test]
    fn manager_initializes_from_a_persisted_shortcut_and_repairs_invalid_values() {
        let persisted = SaveReplayHotkeyManager::new("Shift + Numpad0");
        assert_eq!(persisted.state().current_combination, "Shift + Numpad0");
        assert!(persisted.state().last_registration_error.is_none());

        let repaired = SaveReplayHotkeyManager::new("Ctrl + NotARealKey");
        assert_eq!(
            repaired.state().current_combination,
            DEFAULT_SAVE_REPLAY_HOTKEY
        );
        assert!(repaired.state().last_registration_error.is_some());
    }
}
