import { useState } from "react";
import { Toggle } from "../components/Toggle";

type CaptureMode = "Game" | "Desktop" | "Window";

const captureModes: CaptureMode[] = ["Game", "Desktop", "Window"];

export function ReplayPage() {
  const [demoRunning, setDemoRunning] = useState(false);
  const [captureMode, setCaptureMode] = useState<CaptureMode>("Game");
  const [clipLength, setClipLength] = useState("2 Minutes");
  const [resolution, setResolution] = useState("1440p");
  const [frameRate, setFrameRate] = useState("60 FPS");
  const [encoder, setEncoder] = useState("Automatic");
  const [audioSources, setAudioSources] = useState({
    game: true,
    discord: true,
    microphone: true,
    other: false,
  });

  function setAudioSource(source: keyof typeof audioSources, enabled: boolean) {
    setAudioSources((current) => ({ ...current, [source]: enabled }));
  }

  return (
    <div className="page page-replay">
      <header className="page-header">
        <div>
          <h1>Replay</h1>
          <p>Capture the moments you actually want to keep.</p>
        </div>
        <span className="demo-badge">UI DEMO</span>
      </header>

      <section className="status-card" aria-labelledby="buffer-heading">
        <div className="status-card-copy">
          <span className="eyebrow">CAPTURE STATUS</span>
          <h2 id="buffer-heading">Replay Buffer</h2>
          <div className="offline-status">
            <span className="status-dot" aria-hidden="true" />
            Status: OFFLINE
          </div>
          <p>Capture engine not connected. This control is a temporary interface preview.</p>
        </div>
        <button
          className={`primary-button buffer-button${demoRunning ? " stop-button" : ""}`}
          type="button"
          aria-pressed={demoRunning}
          onClick={() => setDemoRunning((running) => !running)}
        >
          {demoRunning ? "Stop Replay Buffer" : "Start Replay Buffer"}
        </button>
      </section>

      <div className="replay-grid">
        <section className="panel" aria-labelledby="capture-heading">
          <div className="section-heading">
            <div>
              <span className="eyebrow">CONFIGURATION</span>
              <h2 id="capture-heading">Capture</h2>
            </div>
            <span className="section-note">Session only</span>
          </div>

          <div className="setting-row setting-row-stacked">
            <span className="setting-label">Capture Mode</span>
            <div className="segmented-control" aria-label="Capture mode">
              {captureModes.map((mode) => (
                <button
                  className={captureMode === mode ? "segment-active" : ""}
                  type="button"
                  aria-pressed={captureMode === mode}
                  key={mode}
                  onClick={() => setCaptureMode(mode)}
                >
                  {mode}
                </button>
              ))}
            </div>
          </div>

          <SelectSetting label="Clip Length" value={clipLength} onChange={setClipLength} options={["30 Seconds", "1 Minute", "2 Minutes", "3 Minutes", "5 Minutes"]} />
          <SelectSetting label="Resolution" value={resolution} onChange={setResolution} options={["720p", "1080p", "1440p"]} />
          <SelectSetting label="Frame Rate" value={frameRate} onChange={setFrameRate} options={["30 FPS", "60 FPS"]} />
          <SelectSetting label="Encoder" value={encoder} onChange={setEncoder} options={["NVIDIA NVENC AV1", "NVIDIA NVENC HEVC", "NVIDIA NVENC H.264", "Automatic"]} />
        </section>

        <div className="replay-side-stack">
          <section className="panel" aria-labelledby="audio-heading">
            <div className="section-heading">
              <div>
                <span className="eyebrow">MIX</span>
                <h2 id="audio-heading">Audio Sources</h2>
              </div>
            </div>
            <ToggleSetting label="Game Audio" checked={audioSources.game} onChange={(value) => setAudioSource("game", value)} />
            <ToggleSetting label="Discord" checked={audioSources.discord} onChange={(value) => setAudioSource("discord", value)} />
            <ToggleSetting label="Microphone" checked={audioSources.microphone} onChange={(value) => setAudioSource("microphone", value)} />
            <ToggleSetting label="Other Application" checked={audioSources.other} onChange={(value) => setAudioSource("other", value)} />
          </section>

          <section className="panel save-panel" aria-labelledby="save-heading">
            <div className="section-heading">
              <div>
                <span className="eyebrow">MANUAL CAPTURE</span>
                <h2 id="save-heading">Save Replay</h2>
              </div>
            </div>
            <div className="hotkey-row">
              <div>
                <span className="setting-label">Replay Hotkey</span>
                <kbd>Ctrl + Shift + F10</kbd>
              </div>
              <button className="secondary-button" type="button">Change</button>
            </div>
            <button className="save-replay-button" type="button" disabled title="Capture engine not connected">
              SAVE REPLAY
            </button>
            <span className="disabled-reason">Capture engine not connected</span>
          </section>
        </div>
      </div>
    </div>
  );
}

type SelectSettingProps = {
  label: string;
  value: string;
  options: string[];
  onChange: (value: string) => void;
};

function SelectSetting({ label, value, options, onChange }: SelectSettingProps) {
  return (
    <label className="setting-row">
      <span className="setting-label">{label}</span>
      <select value={value} onChange={(event) => onChange(event.target.value)}>
        {options.map((option) => <option key={option}>{option}</option>)}
      </select>
    </label>
  );
}

type ToggleSettingProps = {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
};

function ToggleSetting({ label, checked, onChange }: ToggleSettingProps) {
  return (
    <div className="setting-row toggle-row">
      <span className="setting-label">{label}</span>
      <Toggle label={label} checked={checked} onChange={onChange} />
    </div>
  );
}
