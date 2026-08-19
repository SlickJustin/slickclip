use std::collections::HashSet;
use std::sync::{Mutex, MutexGuard};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Modifiers, Shortcut, ShortcutState};

use crate::clips::{ClipSaveManager, SaveJobState};

pub const DEFAULT_SAVE_REPLAY_HOTKEY: &str = "Ctrl + Shift + F10";

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HotkeyState {
    pub registered: bool,
    pub current_combination: String,
    pub last_registration_error: Option<String>,
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

struct ParsedHotkey {
    normalized: String,
    shortcut: Shortcut,
}

struct HotkeyInner {
    state: HotkeyState,
    shortcut: Shortcut,
    recorder_active: bool,
}

pub struct SaveReplayHotkeyManager {
    inner: Mutex<HotkeyInner>,
}

impl SaveReplayHotkeyManager {
    pub fn new() -> Self {
        let parsed = parse_hotkey(DEFAULT_SAVE_REPLAY_HOTKEY)
            .expect("the built-in Save Replay hotkey must remain valid");
        Self {
            inner: Mutex::new(HotkeyInner {
                state: HotkeyState {
                    registered: false,
                    current_combination: parsed.normalized,
                    last_registration_error: None,
                },
                shortcut: parsed.shortcut,
                recorder_active: false,
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, HotkeyInner> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn register_initial(&self, app: &AppHandle) {
        let mut inner = self.lock();
        match app.global_shortcut().register(inner.shortcut) {
            Ok(()) => {
                inner.state.registered = true;
                inner.state.last_registration_error = None;
            }
            Err(error) => {
                inner.state.registered = false;
                inner.state.last_registration_error = Some(format_registration_error(
                    &inner.state.current_combination,
                    error,
                ));
            }
        }
    }

    pub fn state(&self) -> HotkeyState {
        self.lock().state.clone()
    }

    pub fn set_recorder_active(&self, active: bool) -> HotkeyState {
        let mut inner = self.lock();
        inner.recorder_active = active;
        inner.state.clone()
    }

    pub fn rebind(&self, app: &AppHandle, combination: &str) -> HotkeyCommandResult {
        let parsed = match parse_hotkey(combination) {
            Ok(parsed) => parsed,
            Err(error) => {
                let mut inner = self.lock();
                inner.recorder_active = false;
                return HotkeyCommandResult {
                    success: false,
                    state: inner.state.clone(),
                    error_message: Some(error),
                };
            }
        };

        let mut inner = self.lock();
        if parsed.shortcut == inner.shortcut && inner.state.registered {
            inner.recorder_active = false;
            inner.state.last_registration_error = None;
            return HotkeyCommandResult {
                success: true,
                state: inner.state.clone(),
                error_message: None,
            };
        }

        if let Err(error) = app.global_shortcut().register(parsed.shortcut) {
            let message = format_registration_error(&parsed.normalized, error);
            inner.recorder_active = false;
            inner.state.last_registration_error = Some(message.clone());
            return HotkeyCommandResult {
                success: false,
                state: inner.state.clone(),
                error_message: Some(message),
            };
        }

        if inner.state.registered {
            if let Err(error) = app.global_shortcut().unregister(inner.shortcut) {
                let _ = app.global_shortcut().unregister(parsed.shortcut);
                let message = format!(
                    "The new hotkey was available, but the previous hotkey could not be unregistered: {error}"
                );
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
        };
        HotkeyCommandResult {
            success: true,
            state: inner.state.clone(),
            error_message: None,
        }
    }

    pub fn should_handle(&self, shortcut: &Shortcut) -> bool {
        let inner = self.lock();
        inner.state.registered && !inner.recorder_active && shortcut == &inner.shortcut
    }

    pub fn unregister(&self, app: &AppHandle) {
        let mut inner = self.lock();
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

pub fn handle_global_shortcut(app: &AppHandle, shortcut: &Shortcut, state: ShortcutState) {
    if state != ShortcutState::Pressed {
        return;
    }
    let Some(hotkey) = app.try_state::<SaveReplayHotkeyManager>() else {
        return;
    };
    if !hotkey.should_handle(shortcut) {
        return;
    }
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
}

#[tauri::command]
pub fn get_save_replay_hotkey(manager: tauri::State<'_, SaveReplayHotkeyManager>) -> HotkeyState {
    manager.state()
}

#[tauri::command]
pub fn set_save_replay_hotkey(
    app: AppHandle,
    manager: tauri::State<'_, SaveReplayHotkeyManager>,
    combination: String,
) -> HotkeyCommandResult {
    manager.rebind(&app, &combination)
}

#[tauri::command]
pub fn set_hotkey_recorder_active(
    manager: tauri::State<'_, SaveReplayHotkeyManager>,
    active: bool,
) -> HotkeyState {
    manager.set_recorder_active(active)
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

    if modifiers.is_empty() {
        return Err("Global hotkeys must include Ctrl, Shift, Alt, or Win.".to_string());
    }
    let (key_name, code) = key.ok_or_else(|| {
        "Hotkey must include one keyboard key in addition to its modifiers.".to_string()
    })?;

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
        shortcut: Shortcut::new(Some(modifiers), code),
    })
}

fn parse_key(input: &str) -> Result<(String, Code), String> {
    let upper = input.trim().to_ascii_uppercase();
    let result = match upper.as_str() {
        "A" => ("A", Code::KeyA),
        "B" => ("B", Code::KeyB),
        "C" => ("C", Code::KeyC),
        "D" => ("D", Code::KeyD),
        "E" => ("E", Code::KeyE),
        "F" => ("F", Code::KeyF),
        "G" => ("G", Code::KeyG),
        "H" => ("H", Code::KeyH),
        "I" => ("I", Code::KeyI),
        "J" => ("J", Code::KeyJ),
        "K" => ("K", Code::KeyK),
        "L" => ("L", Code::KeyL),
        "M" => ("M", Code::KeyM),
        "N" => ("N", Code::KeyN),
        "O" => ("O", Code::KeyO),
        "P" => ("P", Code::KeyP),
        "Q" => ("Q", Code::KeyQ),
        "R" => ("R", Code::KeyR),
        "S" => ("S", Code::KeyS),
        "T" => ("T", Code::KeyT),
        "U" => ("U", Code::KeyU),
        "V" => ("V", Code::KeyV),
        "W" => ("W", Code::KeyW),
        "X" => ("X", Code::KeyX),
        "Y" => ("Y", Code::KeyY),
        "Z" => ("Z", Code::KeyZ),
        "0" => ("0", Code::Digit0),
        "1" => ("1", Code::Digit1),
        "2" => ("2", Code::Digit2),
        "3" => ("3", Code::Digit3),
        "4" => ("4", Code::Digit4),
        "5" => ("5", Code::Digit5),
        "6" => ("6", Code::Digit6),
        "7" => ("7", Code::Digit7),
        "8" => ("8", Code::Digit8),
        "9" => ("9", Code::Digit9),
        "F1" => ("F1", Code::F1),
        "F2" => ("F2", Code::F2),
        "F3" => ("F3", Code::F3),
        "F4" => ("F4", Code::F4),
        "F5" => ("F5", Code::F5),
        "F6" => ("F6", Code::F6),
        "F7" => ("F7", Code::F7),
        "F8" => ("F8", Code::F8),
        "F9" => ("F9", Code::F9),
        "F10" => ("F10", Code::F10),
        "F11" => ("F11", Code::F11),
        "F12" => ("F12", Code::F12),
        "SPACE" => ("Space", Code::Space),
        "ENTER" => ("Enter", Code::Enter),
        "ESC" | "ESCAPE" => ("Escape", Code::Escape),
        "TAB" => ("Tab", Code::Tab),
        "BACKSPACE" => ("Backspace", Code::Backspace),
        "DELETE" | "DEL" => ("Delete", Code::Delete),
        "INSERT" | "INS" => ("Insert", Code::Insert),
        "HOME" => ("Home", Code::Home),
        "END" => ("End", Code::End),
        "PAGEUP" | "PGUP" => ("PageUp", Code::PageUp),
        "PAGEDOWN" | "PGDN" => ("PageDown", Code::PageDown),
        "ARROWUP" | "UP" => ("ArrowUp", Code::ArrowUp),
        "ARROWDOWN" | "DOWN" => ("ArrowDown", Code::ArrowDown),
        "ARROWLEFT" | "LEFT" => ("ArrowLeft", Code::ArrowLeft),
        "ARROWRIGHT" | "RIGHT" => ("ArrowRight", Code::ArrowRight),
        _ => {
            return Err(format!(
                "Unsupported hotkey key '{input}'. Use a letter, number, F1-F12, navigation key, Space, Enter, or Tab."
            ));
        }
    };
    Ok((result.0.to_string(), result.1))
}

#[cfg(test)]
mod tests {
    use super::{parse_hotkey, SaveReplayHotkeyManager, DEFAULT_SAVE_REPLAY_HOTKEY};

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
    fn rejects_single_keys_and_modifier_only_bindings() {
        assert!(parse_hotkey("A").is_err());
        assert!(parse_hotkey("Space").is_err());
        assert!(parse_hotkey("Ctrl + Shift").is_err());
    }

    #[test]
    fn rejects_duplicate_modifiers_and_multiple_keys() {
        assert!(parse_hotkey("Ctrl + Control + A").is_err());
        assert!(parse_hotkey("Ctrl + A + B").is_err());
    }

    #[test]
    fn rejects_empty_and_unsupported_combinations() {
        assert!(parse_hotkey("Ctrl++A").is_err());
        assert!(parse_hotkey("Ctrl + MediaPlayPause").is_err());
    }

    #[test]
    fn recorder_state_suppresses_the_registered_shortcut_without_os_input() {
        let manager = SaveReplayHotkeyManager::new();
        let parsed = parse_hotkey(DEFAULT_SAVE_REPLAY_HOTKEY).unwrap();
        assert!(!manager.should_handle(&parsed.shortcut));

        {
            let mut inner = manager.lock();
            inner.state.registered = true;
        }
        assert!(manager.should_handle(&parsed.shortcut));

        manager.set_recorder_active(true);
        assert!(!manager.should_handle(&parsed.shortcut));
        manager.set_recorder_active(false);
        assert!(manager.should_handle(&parsed.shortcut));
    }
}
