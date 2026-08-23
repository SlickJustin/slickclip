import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { AudioCaptureTest } from "../components/AudioCaptureTest";
import { Toggle } from "../components/Toggle";
import type { UiPreferences, UiPreferencesPatch, UiPreferencesResponse } from "../types/clips";
import { defaultUiPreferences } from "../types/clips";

type HotkeyState = {
  registered: boolean;
  currentCombination: string;
  lastRegistrationError: string | null;
  testing: boolean;
};

type HotkeyCommandResult = {
  success: boolean;
  state: HotkeyState;
  errorMessage: string | null;
};

const initialHotkeyState: HotkeyState = {
  registered: false,
  currentCombination: "Ctrl + Shift + F10",
  lastRegistrationError: null,
  testing: false,
};

type SettingSelectProps = {
  label: string;
  value: string;
  options: string[];
  onChange: (value: string) => void;
};

export function SettingsPage() {
  const [captureMode, setCaptureMode] = useState("Game");
  const [clipLength, setClipLength] = useState("2 Minutes");
  const [resolution, setResolution] = useState("1440p");
  const [frameRate, setFrameRate] = useState("60 FPS");
  const [encoder, setEncoder] = useState("Automatic");
  const [hotkey, setHotkey] = useState<HotkeyState>(initialHotkeyState);
  const [recordingHotkey, setRecordingHotkey] = useState(false);
  const [hotkeyPending, setHotkeyPending] = useState(false);
  const [hotkeyMessage, setHotkeyMessage] = useState<{ text: string; success: boolean } | null>(null);
  const [preferences, setPreferences] = useState<UiPreferences>(defaultUiPreferences);
  const [desktopSettingsPending, setDesktopSettingsPending] = useState(false);
  const [desktopSettingsMessage, setDesktopSettingsMessage] = useState<{ text: string; success: boolean } | null>(null);
  const [desktopPrivacy, setDesktopPrivacy] = useState(true);

  async function updateDesktopPreference(patch: UiPreferencesPatch) {
    setDesktopSettingsPending(true);
    setDesktopSettingsMessage(null);
    try {
      const response = await invoke<UiPreferencesResponse>("update_ui_preferences", { patch });
      setPreferences(response.preferences);
      if (!response.success) throw new Error(response.errorMessage ?? "The desktop preference could not be saved.");
    } catch (error) {
      setDesktopSettingsMessage({ text: error instanceof Error ? error.message : String(error), success: false });
    } finally {
      setDesktopSettingsPending(false);
    }
  }

  async function setStartWithWindows(enabled: boolean) {
    setDesktopSettingsPending(true);
    setDesktopSettingsMessage(null);
    try {
      const response = await invoke<UiPreferencesResponse>("set_start_with_windows", { enabled });
      setPreferences(response.preferences);
      if (!response.success) throw new Error(response.errorMessage ?? "Windows startup could not be updated.");
      setDesktopSettingsMessage({ text: enabled ? "SlickClip will start in the background with Windows." : "Windows startup disabled.", success: true });
    } catch (error) {
      setDesktopSettingsMessage({ text: error instanceof Error ? error.message : String(error), success: false });
    } finally {
      setDesktopSettingsPending(false);
    }
  }

  useEffect(() => {
    void Promise.all([
      invoke<HotkeyState>("get_save_replay_hotkey"),
      invoke<UiPreferencesResponse>("get_ui_preferences"),
    ]).then(([hotkeyState, preferenceResponse]) => {
      setHotkey(hotkeyState);
      setPreferences(preferenceResponse.preferences);
    }).catch((error) => setHotkeyMessage({ text: error instanceof Error ? error.message : String(error), success: false }));
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    void listen<{ success: boolean; message: string }>("save-replay-hotkey-test-result", (event) => {
      setHotkey((current) => ({ ...current, testing: false }));
      setHotkeyMessage({ text: event.payload.message, success: event.payload.success });
    }).then((cleanup) => { if (disposed) cleanup(); else unlisten = cleanup; });
    return () => {
      disposed = true;
      unlisten?.();
      void invoke("cancel_hotkey_test");
    };
  }, []);

  useEffect(() => {
    if (!recordingHotkey) return;

    const handleKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat || isModifierCode(event.code)) return;
      if (event.code === "Escape" && !event.ctrlKey && !event.shiftKey && !event.altKey && !event.metaKey) {
        void stopHotkeyRecording();
        return;
      }

      const combination = combinationFromKeyboardEvent(event);
      if (!combination) {
        setHotkeyMessage({ text: "Use Ctrl, Shift, Alt, or Win with a supported keyboard key.", success: false });
        return;
      }
      void submitHotkey(combination);
    };

    window.addEventListener("keydown", handleKeyDown, true);
    return () => window.removeEventListener("keydown", handleKeyDown, true);
  }, [recordingHotkey]);

  useEffect(() => () => {
    if (recordingHotkey) void invoke("set_hotkey_recorder_active", { active: false });
  }, [recordingHotkey]);

  async function startHotkeyRecording() {
    setHotkeyMessage(null);
    try {
      const state = await invoke<HotkeyState>("set_hotkey_recorder_active", { active: true });
      setHotkey(state);
      setRecordingHotkey(true);
    } catch (error) {
      setHotkeyMessage({ text: error instanceof Error ? error.message : String(error), success: false });
    }
  }

  async function testHotkey() {
    setHotkeyMessage(null);
    try {
      const result = await invoke<HotkeyCommandResult>("begin_hotkey_test");
      setHotkey(result.state);
      setHotkeyMessage({
        text: result.success ? `Press ${result.state.currentCombination}…` : result.errorMessage ?? "The hotkey cannot be tested.",
        success: result.success,
      });
    } catch (error) {
      setHotkeyMessage({ text: error instanceof Error ? error.message : String(error), success: false });
    }
  }

  async function stopHotkeyRecording() {
    setRecordingHotkey(false);
    try {
      const state = await invoke<HotkeyState>("set_hotkey_recorder_active", { active: false });
      setHotkey(state);
    } catch (error) {
      setHotkeyMessage({ text: error instanceof Error ? error.message : String(error), success: false });
    }
  }

  async function submitHotkey(combination: string) {
    if (hotkeyPending) return;
    setHotkeyPending(true);
    setHotkeyMessage(null);
    try {
      const result = await invoke<HotkeyCommandResult>("set_save_replay_hotkey", { combination });
      setHotkey(result.state);
      setRecordingHotkey(false);
      setHotkeyMessage({
        text: result.success
          ? `Save Replay hotkey changed to ${result.state.currentCombination}.`
          : result.errorMessage ?? "The global hotkey could not be registered.",
        success: result.success,
      });
    } catch (error) {
      setRecordingHotkey(false);
      setHotkeyMessage({ text: error instanceof Error ? error.message : String(error), success: false });
      await invoke<HotkeyState>("set_hotkey_recorder_active", { active: false })
        .then(setHotkey)
        .catch(() => undefined);
    } finally {
      setHotkeyPending(false);
    }
  }

  return (
    <div className="page settings-page">
      <header className="page-header">
        <div>
          <h1>Settings</h1>
          <p>Configure Replay for your setup.</p>
        </div>
      </header>

      <div className="settings-grid">
        <SettingsSection title="Capture">
          <SettingSelect label="Default Capture Mode" value={captureMode} onChange={setCaptureMode} options={["Game", "Desktop", "Window"]} />
          <SettingSelect label="Default Clip Length" value={clipLength} onChange={setClipLength} options={["30 Seconds", "1 Minute", "2 Minutes", "3 Minutes", "5 Minutes"]} />
          <SettingSelect label="Resolution" value={resolution} onChange={setResolution} options={["720p", "1080p", "1440p"]} />
          <SettingSelect label="Frame Rate" value={frameRate} onChange={setFrameRate} options={["30 FPS", "60 FPS"]} />
          <SettingSelect label="Preferred Encoder" value={encoder} onChange={setEncoder} options={["NVIDIA NVENC AV1", "NVIDIA NVENC HEVC", "NVIDIA NVENC H.264", "Automatic"]} />
        </SettingsSection>

        <SettingsSection title="Hotkeys">
          <div className="hotkey-setting">
            <div className="hotkey-setting-copy">
              <span>Save Replay Hotkey</span>
              <small>Works globally while SlickClip is in the background.</small>
              <div className="hotkey-registration-status">
                <span className={`hotkey-status-dot ${hotkey.registered ? "hotkey-status-registered" : "hotkey-status-error"}`} />
                {hotkey.registered ? "Registered" : "Not registered"}
              </div>
            </div>
            <div className="hotkey-setting-controls">
              <kbd className={recordingHotkey ? "hotkey-recording" : undefined}>
                {recordingHotkey ? "Press a combination..." : hotkey.currentCombination}
              </kbd>
              <button
                className="secondary-button"
                type="button"
                disabled={hotkeyPending}
                onClick={recordingHotkey ? stopHotkeyRecording : startHotkeyRecording}
              >
                {recordingHotkey ? "Cancel" : hotkeyPending ? "Registering..." : "Change"}
              </button>
              <button className="secondary-button" type="button" disabled={!hotkey.registered || recordingHotkey || hotkeyPending || hotkey.testing} onClick={() => void testHotkey()}>{hotkey.testing ? "Listening…" : "Test Hotkey"}</button>
            </div>
          </div>
          {(hotkeyMessage || hotkey.lastRegistrationError) && (
            <span className={hotkeyMessage?.success && !hotkey.lastRegistrationError ? "hotkey-message-success" : "hotkey-message-error"} role="status">
              {hotkeyMessage?.text ?? hotkey.lastRegistrationError}
            </span>
          )}
        </SettingsSection>

        <SettingsSection title="Audio Capture Test">
          <AudioCaptureTest />
        </SettingsSection>

        <SettingsSection title="Storage">
          <div className="settings-row">
            <div><span>Save Location</span><small>Where completed clips will be stored</small></div>
            <div className="path-value">Videos\JustIn Replay\Clips</div>
          </div>
        </SettingsSection>

        <SettingsSection title="Cloud">
          <div className="settings-row">
            <div><span>Cloud Storage</span><small>Optional backup and sharing</small></div>
            <span className="not-configured"><span className="status-dot" />Not configured</span>
          </div>
        </SettingsSection>

        <SettingsSection title="Startup">
          <SettingsToggle label="Start SlickClip with Windows" description="Launches quietly in the system tray after sign-in." checked={preferences.startWithWindows} disabled={desktopSettingsPending} onChange={(value) => void setStartWithWindows(value)} />
          <SettingsToggle label="Close or minimize to tray" description="Keeps active replay capture running in the background." checked={preferences.closeToTray} disabled={desktopSettingsPending} onChange={(value) => void updateDesktopPreference({ closeToTray: value })} />
          <SettingsToggle label="Show Replay Saved overlay" description="Shows a brief notification without taking focus." checked={preferences.saveOverlayEnabled} disabled={desktopSettingsPending} onChange={(value) => void updateDesktopPreference({ saveOverlayEnabled: value })} />
          <SettingsToggle label="Start Replay Buffer automatically" description="Available after Stage 23 game auto-arm is configured." checked={false} disabled onChange={() => undefined} />
          {desktopSettingsMessage && <span className={desktopSettingsMessage.success ? "hotkey-message-success" : "hotkey-message-error"} role="status">{desktopSettingsMessage.text}</span>}
        </SettingsSection>

        <SettingsSection title="Privacy">
          <SettingsToggle
            label="Desktop Capture Privacy"
            description="Application exclusions and privacy rules will be added in a later stage."
            checked={desktopPrivacy}
            onChange={setDesktopPrivacy}
          />
        </SettingsSection>
      </div>
    </div>
  );
}

function isModifierCode(code: string) {
  return ["ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight", "AltLeft", "AltRight", "MetaLeft", "MetaRight"].includes(code);
}

function combinationFromKeyboardEvent(event: KeyboardEvent) {
  const key = displayKeyFromCode(event.code);
  if (!key) return null;
  const parts = [
    event.ctrlKey ? "Ctrl" : null,
    event.shiftKey ? "Shift" : null,
    event.altKey ? "Alt" : null,
    event.metaKey ? "Win" : null,
  ].filter((part): part is string => part !== null);
  if (!parts.length) return null;
  return [...parts, key].join(" + ");
}

function displayKeyFromCode(code: string) {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^F(?:[1-9]|1[0-2])$/.test(code)) return code;
  const keys: Record<string, string> = {
    Space: "Space",
    Enter: "Enter",
    Escape: "Escape",
    Tab: "Tab",
    Backspace: "Backspace",
    Delete: "Delete",
    Insert: "Insert",
    Home: "Home",
    End: "End",
    PageUp: "PageUp",
    PageDown: "PageDown",
    ArrowUp: "ArrowUp",
    ArrowDown: "ArrowDown",
    ArrowLeft: "ArrowLeft",
    ArrowRight: "ArrowRight",
  };
  return keys[code] ?? null;
}

function SettingsSection({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <section className="settings-section">
      <h2>{title}</h2>
      <div className="settings-section-body">{children}</div>
    </section>
  );
}

function SettingSelect({ label, value, options, onChange }: SettingSelectProps) {
  return (
    <label className="settings-row">
      <span>{label}</span>
      <select value={value} onChange={(event) => onChange(event.target.value)}>
        {options.map((option) => <option key={option}>{option}</option>)}
      </select>
    </label>
  );
}

type SettingsToggleProps = {
  label: string;
  description?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
};

function SettingsToggle({ label, description, checked, onChange, disabled = false }: SettingsToggleProps) {
  return (
    <div className="settings-row">
      <div><span>{label}</span>{description && <small>{description}</small>}</div>
      <Toggle label={label} checked={checked} onChange={onChange} disabled={disabled} />
    </div>
  );
}
