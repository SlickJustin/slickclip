import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { Toggle } from "../components/Toggle";

type CaptureMode = "Game" | "Desktop" | "Window";

const captureModes: CaptureMode[] = ["Game", "Desktop", "Window"];

type CaptureTestResult = {
  success: boolean;
  filePath: string | null;
  errorMessage: string | null;
  borderlessActive: boolean;
  borderlessStatus: string;
  borderedCaptureAvailable: boolean | null;
};

type MonitorTarget = {
  id: string;
  displayIndex: number;
  friendlyName: string;
  width: number;
  height: number;
  refreshRate: number | null;
  primary: boolean;
};

type WindowTarget = {
  id: string;
  title: string;
  processName: string | null;
  processId: number;
  width: number;
  height: number;
};

type TargetListResult<T> = {
  success: boolean;
  targets: T[];
  errorMessage: string | null;
};

type TargetTab = "monitor" | "window";
type SelectedTarget = { targetType: TargetTab; id: string };
type CaptureTestStatus = "idle" | "preparing" | "recording" | "success" | "error";

export function ReplayPage() {
  const [demoRunning, setDemoRunning] = useState(false);
  const [captureMode, setCaptureMode] = useState<CaptureMode>("Game");
  const [clipLength, setClipLength] = useState("2 Minutes");
  const [resolution, setResolution] = useState("1440p");
  const [frameRate, setFrameRate] = useState("60 FPS");
  const [encoder, setEncoder] = useState("Automatic");
  const [captureTestActive, setCaptureTestActive] = useState(false);
  const [captureTestStatus, setCaptureTestStatus] = useState<CaptureTestStatus>("idle");
  const [captureTestResult, setCaptureTestResult] = useState<CaptureTestResult | null>(null);
  const [captureTestMessage, setCaptureTestMessage] = useState(
    "Select a capture target to record a temporary video-only proof.",
  );
  const [targetTab, setTargetTab] = useState<TargetTab>("monitor");
  const [monitors, setMonitors] = useState<MonitorTarget[]>([]);
  const [windows, setWindows] = useState<WindowTarget[]>([]);
  const [selectedTarget, setSelectedTarget] = useState<SelectedTarget | null>(null);
  const [targetsLoading, setTargetsLoading] = useState(true);
  const [targetsError, setTargetsError] = useState<string | null>(null);
  const [audioSources, setAudioSources] = useState({
    game: true,
    discord: true,
    microphone: true,
    other: false,
  });

  function setAudioSource(source: keyof typeof audioSources, enabled: boolean) {
    setAudioSources((current) => ({ ...current, [source]: enabled }));
  }

  useEffect(() => {
    void refreshAllTargets();
  }, []);

  async function refreshAllTargets() {
    setTargetsLoading(true);
    setTargetsError(null);

    try {
      const [monitorResult, windowResult] = await Promise.all([
        invoke<TargetListResult<MonitorTarget>>("list_capture_monitors"),
        invoke<TargetListResult<WindowTarget>>("list_capture_windows"),
      ]);
      setMonitors(monitorResult.targets);
      setWindows(windowResult.targets);
      setTargetsError(
        [monitorResult.errorMessage, windowResult.errorMessage].filter(Boolean).join(" ") || null,
      );
    } catch (error) {
      setTargetsError(error instanceof Error ? error.message : String(error));
    } finally {
      setTargetsLoading(false);
    }
  }

  async function refreshVisibleTargets() {
    setTargetsLoading(true);
    setTargetsError(null);

    try {
      if (targetTab === "monitor") {
        const result = await invoke<TargetListResult<MonitorTarget>>("list_capture_monitors");
        setMonitors(result.targets);
        setTargetsError(result.errorMessage);
        setSelectedTarget((current) =>
          current?.targetType === "monitor" && result.targets.some((target) => target.id === current.id)
            ? current
            : null,
        );
      } else {
        const result = await invoke<TargetListResult<WindowTarget>>("list_capture_windows");
        setWindows(result.targets);
        setTargetsError(result.errorMessage);
        setSelectedTarget((current) =>
          current?.targetType === "window" && result.targets.some((target) => target.id === current.id)
            ? current
            : null,
        );
      }
    } catch (error) {
      setTargetsError(error instanceof Error ? error.message : String(error));
    } finally {
      setTargetsLoading(false);
    }
  }

  function changeTargetTab(tab: TargetTab) {
    setTargetTab(tab);
    setSelectedTarget(null);
    setTargetsError(null);
  }

  async function recordCaptureTest() {
    if (captureTestActive || !selectedTarget) return;

    setCaptureTestActive(true);
    setCaptureTestResult(null);
    setCaptureTestStatus("preparing");
    setCaptureTestMessage("Requesting borderless capture permission...");

    let unlisten: UnlistenFn | undefined;

    try {
      unlisten = await listen("capture-test-recording-started", () => {
        setCaptureTestStatus("recording");
        setCaptureTestMessage("Recording test...");
      });

      const result = await invoke<CaptureTestResult>("run_capture_test", {
        target: selectedTarget,
      });
      setCaptureTestResult(result);
      if (result.success && result.filePath) {
        setCaptureTestStatus("success");
        setCaptureTestMessage("Capture test completed successfully.");
      } else {
        setCaptureTestStatus("error");
        setCaptureTestMessage(result.errorMessage ?? "Native capture failed without an error message.");
      }
    } catch (error) {
      setCaptureTestStatus("error");
      setCaptureTestMessage(error instanceof Error ? error.message : String(error));
    } finally {
      unlisten?.();
      setCaptureTestActive(false);
    }
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

      <section className="native-capture-test" aria-labelledby="native-capture-test-heading">
        <div className="native-capture-test-header">
          <div className="native-capture-test-copy">
            <span className="eyebrow">DEVELOPMENT PROOF</span>
            <h2 id="native-capture-test-heading">NATIVE CAPTURE TEST</h2>
            <p>Select a display or application window, then record five seconds of video only.</p>
          </div>
          <button
            className="secondary-button capture-target-refresh"
            type="button"
            disabled={targetsLoading || captureTestActive}
            onClick={refreshVisibleTargets}
          >
            {targetsLoading ? "Refreshing..." : "Refresh"}
          </button>
        </div>

        <div className="capture-target-tabs" aria-label="Capture target type">
          <button
            className={targetTab === "monitor" ? "capture-target-tab-active" : ""}
            type="button"
            aria-pressed={targetTab === "monitor"}
            onClick={() => changeTargetTab("monitor")}
          >
            Displays <span>{monitors.length}</span>
          </button>
          <button
            className={targetTab === "window" ? "capture-target-tab-active" : ""}
            type="button"
            aria-pressed={targetTab === "window"}
            onClick={() => changeTargetTab("window")}
          >
            Windows <span>{windows.length}</span>
          </button>
        </div>

        <div className={`capture-target-list capture-target-list-${targetTab}`}>
          {targetsLoading ? (
            <div className="capture-target-empty">Detecting available {targetTab === "monitor" ? "displays" : "windows"}...</div>
          ) : targetsError ? (
            <div className="capture-target-empty capture-target-load-error">{targetsError}</div>
          ) : targetTab === "monitor" ? (
            monitors.length > 0 ? monitors.map((monitor) => (
              <button
                className={`capture-target-card${selectedTarget?.id === monitor.id ? " capture-target-selected" : ""}`}
                type="button"
                aria-pressed={selectedTarget?.id === monitor.id}
                key={monitor.id}
                onClick={() => setSelectedTarget({ targetType: "monitor", id: monitor.id })}
              >
                <span className="capture-target-card-title">
                  Display {monitor.displayIndex}
                  {monitor.primary && <span className="capture-target-primary">Primary</span>}
                </span>
                <span className="capture-target-friendly-name">{monitor.friendlyName}</span>
                <span className="capture-target-details">
                  {monitor.width} × {monitor.height}
                  {monitor.refreshRate && <span>{monitor.refreshRate} Hz</span>}
                </span>
              </button>
            )) : (
              <div className="capture-target-empty">No capturable displays were detected.</div>
            )
          ) : windows.length > 0 ? windows.map((window) => (
            <button
              className={`capture-window-row${selectedTarget?.id === window.id ? " capture-target-selected" : ""}`}
              type="button"
              aria-pressed={selectedTarget?.id === window.id}
              key={window.id}
              onClick={() => setSelectedTarget({ targetType: "window", id: window.id })}
            >
              <span className="capture-window-app">{window.processName ?? `Process ${window.processId}`}</span>
              <span className="capture-window-title">{window.title}</span>
              <span className="capture-window-size">{window.width} × {window.height}</span>
            </button>
          )) : (
            <div className="capture-target-empty">No capturable application windows were detected.</div>
          )}
        </div>

        <div className="native-capture-test-footer">
          <div
            className={`capture-test-result capture-test-${captureTestStatus}`}
            role="status"
            aria-live="polite"
          >
            <span className="capture-test-message">{captureTestMessage}</span>
            {captureTestResult && (
              <div className="capture-test-result-details">
                <span className={captureTestResult.borderlessActive ? "borderless-active" : "borderless-inactive"}>
                  Borderless capture: {captureTestResult.borderlessActive
                    ? "Active"
                    : formatBorderlessStatus(captureTestResult.borderlessStatus)}
                </span>
                {captureTestResult.borderedCaptureAvailable === true && !captureTestResult.borderlessActive && (
                  <span>Normal bordered capture is supported on this system.</span>
                )}
                {captureTestResult.filePath && (
                  <code>{captureTestResult.filePath}</code>
                )}
              </div>
            )}
          </div>
          <button
            className="primary-button capture-test-button"
            type="button"
            disabled={captureTestActive || !selectedTarget || targetsLoading}
            onClick={recordCaptureTest}
          >
            {captureTestStatus === "recording" ? "Recording test..." : captureTestActive ? "Preparing capture..." : "Record 5 Second Test"}
          </button>
        </div>
      </section>

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

function formatBorderlessStatus(status: string) {
  const labels: Record<string, string> = {
    capability_not_declared: "Required capability not declared",
    denied_by_system: "Denied by Windows",
    denied_by_user: "Denied by user",
    permission_check_failed: "Support check failed",
    permission_request_failed: "Permission request failed",
    user_prompt_required: "User consent still required",
    unsupported: "Unsupported by this Windows version",
    permission_granted: "Permission granted; capture failed later",
    not_attempted: "Not active",
  };

  return labels[status] ?? status.split("_").join(" ");
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
