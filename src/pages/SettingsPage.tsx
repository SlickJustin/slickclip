import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AudioCaptureTest } from "../components/AudioCaptureTest";
import { Toggle } from "../components/Toggle";

type HotkeyState = {
  registered: boolean;
  currentCombination: string;
  lastRegistrationError: string | null;
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
  const [toggles, setToggles] = useState({
    game: true,
    discord: true,
    microphone: true,
    other: false,
    windowsStartup: false,
    bufferStartup: false,
    desktopPrivacy: true,
  });

  function updateToggle(key: keyof typeof toggles, value: boolean) {
    setToggles((current) => ({ ...current, [key]: value }));
  }

  useEffect(() => {
    void invoke<HotkeyState>("get_save_replay_hotkey")
      .then(setHotkey)
      .catch((error) => setHotkeyMessage({ text: error instanceof Error ? error.message : String(error), success: false }));
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
              <small>Works globally while JustIn Replay is in the background.</small>
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
            </div>
          </div>
          {(hotkeyMessage || hotkey.lastRegistrationError) && (
            <span className={hotkeyMessage?.success && !hotkey.lastRegistrationError ? "hotkey-message-success" : "hotkey-message-error"} role="status">
              {hotkeyMessage?.text ?? hotkey.lastRegistrationError}
            </span>
          )}
        </SettingsSection>

        <SettingsSection title="Audio">
          <SettingsToggle label="Game Audio" checked={toggles.game} onChange={(value) => updateToggle("game", value)} />
          <SettingsToggle label="Discord" checked={toggles.discord} onChange={(value) => updateToggle("discord", value)} />
          <SettingsToggle label="Microphone" checked={toggles.microphone} onChange={(value) => updateToggle("microphone", value)} />
          <SettingsToggle label="Other Application" checked={toggles.other} onChange={(value) => updateToggle("other", value)} />
        </SettingsSection>

        <SettingsSection title="Audio Capture Test">
          <AudioCaptureTest />
        </SettingsSection>

        <SettingsSection title="Storage">
          <div className="settings-row">
            <div><span>Save Location</span><small>Where completed clips will be stored</small></div>
            <div className="path-value">Videos\Replay</div>
          </div>
        </SettingsSection>

        <SettingsSection title="Cloud">
          <div className="settings-row">
            <div><span>Cloud Storage</span><small>Optional backup and sharing</small></div>
            <span className="not-configured"><span className="status-dot" />Not configured</span>
          </div>
        </SettingsSection>

        <SettingsSection title="Startup">
          <SettingsToggle label="Start Replay with Windows" checked={toggles.windowsStartup} onChange={(value) => updateToggle("windowsStartup", value)} />
          <SettingsToggle label="Start Replay Buffer automatically" checked={toggles.bufferStartup} onChange={(value) => updateToggle("bufferStartup", value)} />
        </SettingsSection>

        <SettingsSection title="Privacy">
          <SettingsToggle
            label="Desktop Capture Privacy"
            description="Application exclusions and privacy rules will be added in a later stage."
            checked={toggles.desktopPrivacy}
            onChange={(value) => updateToggle("desktopPrivacy", value)}
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
};

function SettingsToggle({ label, description, checked, onChange }: SettingsToggleProps) {
  return (
    <div className="settings-row">
      <div><span>{label}</span>{description && <small>{description}</small>}</div>
      <Toggle label={label} checked={checked} onChange={onChange} />
    </div>
  );
}
