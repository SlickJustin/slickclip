import { useState } from "react";
import { Toggle } from "../components/Toggle";

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

        <SettingsSection title="Audio">
          <SettingsToggle label="Game Audio" checked={toggles.game} onChange={(value) => updateToggle("game", value)} />
          <SettingsToggle label="Discord" checked={toggles.discord} onChange={(value) => updateToggle("discord", value)} />
          <SettingsToggle label="Microphone" checked={toggles.microphone} onChange={(value) => updateToggle("microphone", value)} />
          <SettingsToggle label="Other Application" checked={toggles.other} onChange={(value) => updateToggle("other", value)} />
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
