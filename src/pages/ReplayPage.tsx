import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";

type CaptureTestResult = {
  success: boolean;
  filePath: string | null;
  errorMessage: string | null;
  borderlessActive: boolean;
  borderlessStatus: string;
  borderedCaptureAvailable: boolean | null;
  requestedEncoder: string;
  actualEncoder: string | null;
};

type EncoderId = "automatic" | "av1" | "hevc" | "h264";

type EncoderInfo = {
  id: EncoderId;
  displayName: string;
  codec: string;
  available: boolean;
  reasonUnavailable: string | null;
  recommended: boolean;
  preferred: boolean;
};

type EncoderCapabilitiesResult = {
  success: boolean;
  encoders: EncoderInfo[];
  automaticEncoderId: EncoderId | null;
  detectionMethod: string;
  hardwareAccelerationRequested: boolean;
  hardwareEncodingVerified: boolean;
  errorMessage: string | null;
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

type ReplayLifecycleState = "stopped" | "starting" | "running" | "stopping" | "error";

type CompletedSegment = {
  sequenceNumber: number;
  filePath: string;
  startTimestampMs: number;
  endTimestampMs: number;
  actualDurationMs: number;
  codec: string;
  width: number;
  height: number;
  fileSize: number;
  finalized: boolean;
  finalizationTimeMs: number;
  rotationGapMs: number;
};

type ReplayBufferStatus = {
  state: ReplayLifecycleState;
  errorMessage: string | null;
  targetId: string | null;
  targetLabel: string | null;
  requestedEncoder: string | null;
  actualEncoder: string | null;
  replayDurationSeconds: number;
  expectedSegmentDurationSeconds: number;
  frameRate: number;
  width: number;
  height: number;
  sessionId: string | null;
  sessionDirectory: string | null;
  completedSegmentCount: number;
  retainedDurationSeconds: number;
  retainedBytes: number;
  pendingFinalizations: number;
  droppedSegments: number;
  lastSegmentDurationSeconds: number | null;
  lastRotationGapMs: number | null;
  lastFinalizeTimeMs: number | null;
  recentSegments: CompletedSegment[];
};

type ReplayCommandResult = {
  success: boolean;
  status: ReplayBufferStatus;
  errorMessage: string | null;
};

const replayDurationOptions = [
  { label: "30 Seconds", value: 30 },
  { label: "1 Minute", value: 60 },
  { label: "2 Minutes", value: 120 },
  { label: "3 Minutes", value: 180 },
  { label: "5 Minutes", value: 300 },
];

const initialReplayStatus: ReplayBufferStatus = {
  state: "stopped",
  errorMessage: null,
  targetId: null,
  targetLabel: null,
  requestedEncoder: null,
  actualEncoder: null,
  replayDurationSeconds: 0,
  expectedSegmentDurationSeconds: 2,
  frameRate: 0,
  width: 0,
  height: 0,
  sessionId: null,
  sessionDirectory: null,
  completedSegmentCount: 0,
  retainedDurationSeconds: 0,
  retainedBytes: 0,
  pendingFinalizations: 0,
  droppedSegments: 0,
  lastSegmentDurationSeconds: null,
  lastRotationGapMs: null,
  lastFinalizeTimeMs: null,
  recentSegments: [],
};

export function ReplayPage() {
  const [replayDuration, setReplayDuration] = useState(120);
  const [frameRate, setFrameRate] = useState(60);
  const [replayEncoder, setReplayEncoder] = useState<Exclude<EncoderId, "av1">>("automatic");
  const [replayStatus, setReplayStatus] = useState<ReplayBufferStatus>(initialReplayStatus);
  const [replayCommandActive, setReplayCommandActive] = useState(false);
  const [replayCommandError, setReplayCommandError] = useState<string | null>(null);
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
  const [captureTestEncoder, setCaptureTestEncoder] = useState<EncoderId>("automatic");
  const [encoderCapabilities, setEncoderCapabilities] = useState<EncoderCapabilitiesResult | null>(null);
  const [encodersLoading, setEncodersLoading] = useState(true);
  useEffect(() => {
    void refreshAllTargets();
    void refreshEncoderCapabilities();
    void refreshReplayStatus();
  }, []);

  useEffect(() => {
    if (!isReplayActive(replayStatus.state)) return;

    const timer = window.setInterval(() => void refreshReplayStatus(), 1_000);
    return () => window.clearInterval(timer);
  }, [replayStatus.state]);

  async function refreshEncoderCapabilities() {
    setEncodersLoading(true);

    try {
      const result = await invoke<EncoderCapabilitiesResult>("get_encoder_capabilities");
      setEncoderCapabilities(result);
    } catch (error) {
      setEncoderCapabilities({
        success: false,
        encoders: [],
        automaticEncoderId: null,
        detectionMethod: "Encoder capability detection did not complete.",
        hardwareAccelerationRequested: true,
        hardwareEncodingVerified: false,
        errorMessage: error instanceof Error ? error.message : String(error),
      });
    } finally {
      setEncodersLoading(false);
    }
  }

  async function refreshReplayStatus() {
    try {
      const status = await invoke<ReplayBufferStatus>("get_replay_buffer_status");
      setReplayStatus(status);
      if (status.state === "error") {
        setReplayCommandError(status.errorMessage ?? "The replay buffer entered an unknown error state.");
      }
    } catch (error) {
      setReplayCommandError(error instanceof Error ? error.message : String(error));
    }
  }

  async function startReplayBuffer() {
    if (!selectedTarget || replayCommandActive || isReplayActive(replayStatus.state)) return;

    setReplayCommandActive(true);
    setReplayCommandError(null);
    try {
      const result = await invoke<ReplayCommandResult>("start_replay_buffer", {
        request: {
          target: selectedTarget,
          encoder: replayEncoder,
          replayDurationSeconds: replayDuration,
          frameRate,
        },
      });
      setReplayStatus(result.status);
      if (!result.success) {
        setReplayCommandError(result.errorMessage ?? "The replay buffer could not start.");
      }
    } catch (error) {
      setReplayCommandError(error instanceof Error ? error.message : String(error));
      await refreshReplayStatus();
    } finally {
      setReplayCommandActive(false);
    }
  }

  async function stopReplayBuffer() {
    if (replayCommandActive || !isReplayActive(replayStatus.state)) return;

    setReplayCommandActive(true);
    setReplayCommandError(null);
    try {
      const result = await invoke<ReplayCommandResult>("stop_replay_buffer");
      setReplayStatus(result.status);
      if (!result.success) {
        setReplayCommandError(result.errorMessage ?? "The replay buffer did not stop cleanly.");
      }
    } catch (error) {
      setReplayCommandError(error instanceof Error ? error.message : String(error));
      await refreshReplayStatus();
    } finally {
      setReplayCommandActive(false);
    }
  }

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
    if (isReplayActive(replayStatus.state)) return;
    setTargetTab(tab);
    setSelectedTarget(null);
    setTargetsError(null);
  }

  async function recordCaptureTest() {
    if (captureTestActive || !selectedTarget) return;

    setCaptureTestActive(true);
    setCaptureTestResult(null);
    setCaptureTestStatus("preparing");
    setCaptureTestMessage("Checking encoder and borderless capture permission...");

    let unlisten: UnlistenFn | undefined;

    try {
      unlisten = await listen("capture-test-recording-started", () => {
        setCaptureTestStatus("recording");
        setCaptureTestMessage("Recording test...");
      });

      const result = await invoke<CaptureTestResult>("run_capture_test", {
        target: selectedTarget,
        encoder: captureTestEncoder,
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

  const selectedEncoderAvailable = encoderCapabilities?.encoders.find(
    (encoderOption) => encoderOption.id === captureTestEncoder,
  )?.available ?? false;

  const replayEncoderAvailable = encoderCapabilities?.encoders.find(
    (encoderOption) => encoderOption.id === replayEncoder,
  )?.available ?? false;
  const replayActive = isReplayActive(replayStatus.state);
  const selectedTargetLabel = getSelectedTargetLabel(selectedTarget, monitors, windows);

  return (
    <div className="page page-replay">
      <header className="page-header">
        <div>
          <h1>Replay</h1>
          <p>Capture the moments you actually want to keep.</p>
        </div>
        <span className="demo-badge">VIDEO BUFFER</span>
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
            disabled={targetsLoading || captureTestActive || replayActive}
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
            disabled={replayActive}
            onClick={() => changeTargetTab("monitor")}
          >
            Displays <span>{monitors.length}</span>
          </button>
          <button
            className={targetTab === "window" ? "capture-target-tab-active" : ""}
            type="button"
            aria-pressed={targetTab === "window"}
            disabled={replayActive}
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
                disabled={replayActive}
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
              disabled={replayActive}
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

        <div className="capture-encoder-section">
          <div className="capture-encoder-heading">
            <div>
              <span className="setting-label">Encoder</span>
              <small>Automatic resolves on the capture backend using AV1 -&gt; HEVC -&gt; H.264 priority.</small>
            </div>
            {encoderCapabilities?.automaticEncoderId && (
              <span className="capture-encoder-auto-result">
                Automatic: {formatEncoderId(encoderCapabilities.automaticEncoderId)}
              </span>
            )}
          </div>

          <div className="capture-encoder-options" aria-label="Test capture encoder">
            {encodersLoading ? (
              <div className="capture-encoder-loading">Probing Windows video encoders...</div>
            ) : encoderCapabilities?.encoders.length ? encoderCapabilities.encoders.map((encoderOption) => (
              <button
                className={`capture-encoder-option${captureTestEncoder === encoderOption.id ? " capture-encoder-selected" : ""}${encoderOption.available ? "" : " capture-encoder-unavailable"}`}
                type="button"
                aria-pressed={captureTestEncoder === encoderOption.id}
                disabled={captureTestActive || replayActive || !encoderOption.available}
                key={encoderOption.id}
                title={encoderOption.reasonUnavailable ?? undefined}
                onClick={() => setCaptureTestEncoder(encoderOption.id)}
              >
                <span className="capture-encoder-name">
                  {encoderOption.displayName}
                  {encoderOption.preferred && <span className="capture-encoder-preferred">Preferred</span>}
                  {encoderOption.recommended && !encoderOption.preferred && <span className="capture-encoder-preferred">Recommended</span>}
                </span>
                <span className={encoderOption.available ? "capture-encoder-available" : "capture-encoder-not-available"}>
                  {encoderOption.available ? "Available" : "Unavailable"}
                </span>
                {encoderOption.reasonUnavailable && <small>{encoderOption.reasonUnavailable}</small>}
              </button>
            )) : (
              <div className="capture-encoder-loading capture-target-load-error">
                {encoderCapabilities?.errorMessage ?? "Encoder capability information is unavailable."}
              </div>
            )}
          </div>

          {encoderCapabilities && (
            <p className="capture-encoder-method">
              {encoderCapabilities.errorMessage ?? `${encoderCapabilities.detectionMethod}. Hardware acceleration is ${encoderCapabilities.hardwareAccelerationRequested ? "requested" : "not requested"}, but hardware encoding is ${encoderCapabilities.hardwareEncodingVerified ? "verified" : "not distinguishable from system/software encoding through this API"}.`}
            </p>
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
                {captureTestResult.success && captureTestResult.actualEncoder && (
                  <span className="capture-test-encoder-result">
                    Requested: {captureTestResult.requestedEncoder} / Used: {captureTestResult.actualEncoder}
                  </span>
                )}
              </div>
            )}
          </div>
          <button
            className="primary-button capture-test-button"
            type="button"
            disabled={captureTestActive || replayActive || !selectedTarget || targetsLoading || encodersLoading || !selectedEncoderAvailable}
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
          <div className={`replay-state replay-state-${replayStatus.state}`}>
            <span className="status-dot" aria-hidden="true" />
            Status: {formatReplayState(replayStatus.state)}
          </div>
          <div className="replay-status-summary">
            <span>Target <strong>{replayStatus.targetLabel ?? selectedTargetLabel ?? "Not selected"}</strong></span>
            <span>Encoder <strong>{replayStatus.actualEncoder ?? "—"}</strong></span>
            <span>Window <strong>{formatDuration(replayStatus.replayDurationSeconds || replayDuration)}</strong></span>
            <span>Retained <strong>{replayStatus.retainedDurationSeconds.toFixed(1)} s</strong></span>
            <span>Segments <strong>{replayStatus.completedSegmentCount}</strong></span>
            <span>Buffer <strong>{formatBytes(replayStatus.retainedBytes)}</strong></span>
          </div>
          {(replayCommandError || replayStatus.errorMessage) && (
            <p className="replay-buffer-error" role="alert">
              {replayCommandError ?? replayStatus.errorMessage}
            </p>
          )}
        </div>
        <button
          className={`primary-button buffer-button${replayActive ? " stop-button" : ""}`}
          type="button"
          aria-pressed={replayActive}
          disabled={
            replayCommandActive ||
            replayStatus.state === "stopping" ||
            (!replayActive && (!selectedTarget || encodersLoading || !replayEncoderAvailable))
          }
          onClick={replayActive ? stopReplayBuffer : startReplayBuffer}
        >
          {replayStatus.state === "starting"
            ? "Starting..."
            : replayStatus.state === "stopping"
              ? "Stopping..."
              : replayActive
                ? "Stop Replay Buffer"
                : "Start Replay Buffer"}
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

          <div className="setting-row">
            <span className="setting-label">Capture Target</span>
            <span className="replay-setting-value">{selectedTargetLabel ?? "Select a target above"}</span>
          </div>
          <label className="setting-row">
            <span className="setting-label">Replay Duration</span>
            <select
              value={replayDuration}
              disabled={replayActive}
              onChange={(event) => setReplayDuration(Number(event.target.value))}
            >
              {replayDurationOptions.map((option) => (
                <option value={option.value} key={option.value}>{option.label}</option>
              ))}
            </select>
          </label>
          <label className="setting-row">
            <span className="setting-label">Frame Rate</span>
            <select
              value={frameRate}
              disabled={replayActive}
              onChange={(event) => setFrameRate(Number(event.target.value))}
            >
              <option value={30}>30 FPS</option>
              <option value={60}>60 FPS</option>
            </select>
          </label>
          <label className="setting-row">
            <span className="setting-label">Encoder</span>
            <select
              value={replayEncoder}
              disabled={replayActive || encodersLoading}
              onChange={(event) => setReplayEncoder(event.target.value as Exclude<EncoderId, "av1">)}
            >
              <option value="automatic">Automatic</option>
              <option value="hevc" disabled={!isEncoderAvailable(encoderCapabilities, "hevc")}>HEVC</option>
              <option value="h264" disabled={!isEncoderAvailable(encoderCapabilities, "h264")}>H.264</option>
            </select>
          </label>
          <p className="capture-config-note">
            Stage 6 captures the target at its native dimensions. Video only; no audio is recorded.
          </p>
        </section>

        <div className="replay-side-stack">
          <section className="panel replay-diagnostics" aria-labelledby="diagnostics-heading">
            <div className="section-heading">
              <div>
                <span className="eyebrow">DEVELOPER TELEMETRY</span>
                <h2 id="diagnostics-heading">Segment Diagnostics</h2>
              </div>
            </div>
            <dl className="diagnostic-grid">
              <Diagnostic label="Expected segment" value={`${replayStatus.expectedSegmentDurationSeconds.toFixed(2)} s`} />
              <Diagnostic label="Last segment" value={formatOptionalMetric(replayStatus.lastSegmentDurationSeconds, "s")} />
              <Diagnostic label="Last rotation gap" value={formatOptionalMetric(replayStatus.lastRotationGapMs, "ms")} />
              <Diagnostic label="Last finalize time" value={formatOptionalMetric(replayStatus.lastFinalizeTimeMs, "ms")} />
              <Diagnostic label="Pending finalizations" value={String(replayStatus.pendingFinalizations)} />
              <Diagnostic label="Dropped segments" value={String(replayStatus.droppedSegments)} />
              <Diagnostic label="Video format" value={replayStatus.width ? `${replayStatus.width} × ${replayStatus.height} @ ${replayStatus.frameRate} FPS` : "—"} />
              <Diagnostic label="Session" value={replayStatus.sessionId ?? "—"} />
            </dl>
            <div className="recent-segments">
              <span className="setting-label">Recent finalized segments</span>
              {replayStatus.recentSegments.length ? (
                replayStatus.recentSegments.map((segment) => (
                  <div className="recent-segment-row" key={segment.sequenceNumber}>
                    <code>#{String(segment.sequenceNumber).padStart(6, "0")}</code>
                    <span>{(segment.actualDurationMs / 1_000).toFixed(2)} s</span>
                    <span>{formatBytes(segment.fileSize)}</span>
                  </div>
                ))
              ) : (
                <p>No finalized segments yet.</p>
              )}
            </div>
          </section>

          <section className="panel save-panel" aria-labelledby="save-heading">
            <div className="section-heading">
              <div>
                <span className="eyebrow">MANUAL CAPTURE</span>
                <h2 id="save-heading">Save Replay</h2>
              </div>
            </div>
            <p className="save-stage-note">The rolling video segments stay temporary during this stage.</p>
            <button className="save-replay-button" type="button" disabled title="Available in Stage 7">
              SAVE REPLAY
            </button>
            <span className="disabled-reason">Save Replay will be enabled in the next stage.</span>
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

function formatEncoderId(encoder: EncoderId) {
  const labels: Record<EncoderId, string> = {
    automatic: "Automatic",
    av1: "AV1",
    hevc: "HEVC",
    h264: "H.264",
  };

  return labels[encoder];
}

function isReplayActive(state: ReplayLifecycleState) {
  return state === "starting" || state === "running" || state === "stopping";
}

function isEncoderAvailable(capabilities: EncoderCapabilitiesResult | null, id: EncoderId) {
  return capabilities?.encoders.some((encoder) => encoder.id === id && encoder.available) ?? false;
}

function getSelectedTargetLabel(
  selected: SelectedTarget | null,
  monitors: MonitorTarget[],
  windows: WindowTarget[],
) {
  if (!selected) return null;
  if (selected.targetType === "monitor") {
    const monitor = monitors.find((target) => target.id === selected.id);
    return monitor ? `Display ${monitor.displayIndex} - ${monitor.friendlyName}` : "Selected display";
  }

  const window = windows.find((target) => target.id === selected.id);
  return window ? `${window.processName ?? `Process ${window.processId}`} - ${window.title}` : "Selected window";
}

function formatReplayState(state: ReplayLifecycleState) {
  return state.charAt(0).toUpperCase() + state.slice(1);
}

function formatDuration(seconds: number) {
  if (seconds < 60) return `${seconds} seconds`;
  const minutes = seconds / 60;
  return `${minutes} minute${minutes === 1 ? "" : "s"}`;
}

function formatBytes(bytes: number) {
  if (bytes < 1_024) return `${bytes} B`;
  if (bytes < 1_048_576) return `${(bytes / 1_024).toFixed(1)} KB`;
  if (bytes < 1_073_741_824) return `${(bytes / 1_048_576).toFixed(1)} MB`;
  return `${(bytes / 1_073_741_824).toFixed(2)} GB`;
}

function formatOptionalMetric(value: number | null, unit: string) {
  return value === null ? "—" : `${value.toFixed(2)} ${unit}`;
}

function Diagnostic({ label, value }: { label: string; value: string }) {
  return (
    <div>
      <dt>{label}</dt>
      <dd>{value}</dd>
    </div>
  );
}
