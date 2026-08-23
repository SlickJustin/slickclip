import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type AudioError = {
  code: string;
  message: string;
};

type MicrophoneEndpoint = {
  id: string;
  friendlyName: string;
  isDefaultMultimedia: boolean;
  isDefaultCommunications: boolean;
  state: string;
};

type ApplicationAudioProcess = {
  processId: number;
  displayName: string;
  processName: string;
  executablePath: string | null;
  sessionDisplayNames: string[];
  sessionCount: number;
  renderEndpointCount: number;
  sessionState: string;
};

type ProcessLoopbackCapability = {
  available: boolean;
  windowsBuild: number | null;
  minimumWindowsBuild: number;
  status: string;
  error: AudioError | null;
};

type MicrophoneListResult = {
  success: boolean;
  devices: MicrophoneEndpoint[];
  error: AudioError | null;
};

type ApplicationAudioListResult = {
  success: boolean;
  applications: ApplicationAudioProcess[];
  capability: ProcessLoopbackCapability;
  error: AudioError | null;
};

type ProcessActivationProbeResult = {
  success: boolean;
  processId: number;
  message: string;
  error: AudioError | null;
};

type ActivationProbeStatus = {
  state: "idle" | "running" | "succeeded" | "failed";
  processId: number | null;
  message: string | null;
};

type AudioFormatMetadata = {
  sampleFormat: string;
  formatTag: number;
  sampleRate: number;
  channelCount: number;
  bitsPerSample: number;
  validBitsPerSample: number | null;
  blockAlign: number;
  averageBytesPerSecond: number;
  channelMask: number | null;
  subFormat: string | null;
};

type AudioFormatDiagnostics = {
  getMixFormatStatus: string;
  formatRole: string;
};

type AudioTimingTelemetry = {
  monotonicCaptureStartQpc: number;
  monotonicCaptureEndQpc: number;
  qpcFrequency: number;
  actualWallClockDurationMs: number;
  expectedDurationFromCapturedFramesMs: number;
  expectedDurationInWavMs: number;
  capturedSampleFrames: number;
  writtenSampleFrames: number;
  audioPacketCount: number;
  silentPacketCount: number;
  discontinuityCount: number;
  timestampErrorCount: number;
  firstDevicePosition: number | null;
  lastDevicePosition: number | null;
  firstQpcPosition100ns: number | null;
  lastQpcPosition100ns: number | null;
  queueCapacityPackets: number;
  maximumQueueDepth: number;
  queueFullEvents: number;
  deliberatelyDroppedPackets: number;
  deliberatelyDroppedFrames: number;
};

type AudioCaptureStatus = {
  state: "idle" | "preparing" | "recording" | "finalizing" | "completed" | "error";
  kind: "microphone" | "processLoopback" | null;
  targetLabel: string | null;
  outputPath: string | null;
  format: AudioFormatMetadata | null;
  formatDiagnostics: AudioFormatDiagnostics | null;
  timing: AudioTimingTelemetry | null;
  error: AudioError | null;
};

type AudioCaptureCommandResult = {
  success: boolean;
  status: AudioCaptureStatus;
  error: AudioError | null;
};

const initialStatus: AudioCaptureStatus = {
  state: "idle",
  kind: null,
  targetLabel: null,
  outputPath: null,
  format: null,
  formatDiagnostics: null,
  timing: null,
  error: null,
};

const initialCapability: ProcessLoopbackCapability = {
  available: false,
  windowsBuild: null,
  minimumWindowsBuild: 20348,
  status: "Checking process-loopback support...",
  error: null,
};

const initialActivationProbe: ActivationProbeStatus = {
  state: "idle",
  processId: null,
  message: null,
};

export function AudioCaptureTest() {
  const [tab, setTab] = useState<"microphone" | "application">("microphone");
  const [microphones, setMicrophones] = useState<MicrophoneEndpoint[]>([]);
  const [applications, setApplications] = useState<ApplicationAudioProcess[]>([]);
  const [selectedMicrophoneId, setSelectedMicrophoneId] = useState("");
  const [selectedProcessId, setSelectedProcessId] = useState("");
  const [capability, setCapability] = useState(initialCapability);
  const [microphonesLoading, setMicrophonesLoading] = useState(true);
  const [applicationsLoading, setApplicationsLoading] = useState(true);
  const [microphoneError, setMicrophoneError] = useState<string | null>(null);
  const [applicationError, setApplicationError] = useState<string | null>(null);
  const [captureStatus, setCaptureStatus] = useState(initialStatus);
  const [activationProbe, setActivationProbe] = useState(initialActivationProbe);

  useEffect(() => {
    void refreshMicrophones();
    void refreshApplications();
    void refreshCaptureStatus();
  }, []);

  useEffect(() => {
    if (!isCaptureActive(captureStatus.state)) return;
    const timer = window.setInterval(() => void refreshCaptureStatus(), 500);
    return () => window.clearInterval(timer);
  }, [captureStatus.state]);

  async function refreshMicrophones() {
    setMicrophonesLoading(true);
    setMicrophoneError(null);
    try {
      const result = await invoke<MicrophoneListResult>("list_audio_microphones");
      setMicrophones(result.devices);
      setSelectedMicrophoneId((current) =>
        result.devices.some((device) => device.id === current)
          ? current
          : result.devices.find((device) => device.isDefaultCommunications)?.id
            ?? result.devices.find((device) => device.isDefaultMultimedia)?.id
            ?? result.devices[0]?.id
            ?? "",
      );
      if (!result.success) setMicrophoneError(result.error?.message ?? "Microphone discovery failed.");
    } catch (error) {
      setMicrophoneError(errorMessage(error));
    } finally {
      setMicrophonesLoading(false);
    }
  }

  async function refreshApplications() {
    setApplicationsLoading(true);
    setApplicationError(null);
    setActivationProbe(initialActivationProbe);
    try {
      const result = await invoke<ApplicationAudioListResult>("list_application_audio_processes");
      setApplications(result.applications);
      setCapability(result.capability);
      setSelectedProcessId((current) =>
        result.applications.some((application) => String(application.processId) === current)
          ? current
          : result.applications[0]
            ? String(result.applications[0].processId)
            : "",
      );
      if (!result.success) setApplicationError(result.error?.message ?? "Application audio discovery failed.");
    } catch (error) {
      setApplicationError(errorMessage(error));
    } finally {
      setApplicationsLoading(false);
    }
  }

  async function probeApplicationActivation() {
    const processId = Number(selectedProcessId);
    if (!processId || !capability.available || isCaptureActive(captureStatus.state)) return;
    setApplicationError(null);
    setActivationProbe({ state: "running", processId, message: null });
    try {
      const result = await invoke<ProcessActivationProbeResult>("probe_process_audio_activation", { processId });
      setActivationProbe({
        state: result.success ? "succeeded" : "failed",
        processId,
        message: result.message,
      });
      if (!result.success) setApplicationError(result.error?.message ?? result.message);
    } catch (error) {
      const message = errorMessage(error);
      setActivationProbe({ state: "failed", processId, message });
      setApplicationError(message);
    }
  }

  async function refreshCaptureStatus() {
    try {
      setCaptureStatus(await invoke<AudioCaptureStatus>("get_audio_capture_test_status"));
    } catch (error) {
      const message = errorMessage(error);
      if (tab === "microphone") setMicrophoneError(message);
      else setApplicationError(message);
    }
  }

  async function startMicrophoneTest() {
    if (!selectedMicrophoneId || isCaptureActive(captureStatus.state)) return;
    setMicrophoneError(null);
    try {
      const result = await invoke<AudioCaptureCommandResult>("start_microphone_audio_test", {
        deviceId: selectedMicrophoneId,
      });
      setCaptureStatus(result.status);
      if (!result.success) setMicrophoneError(result.error?.message ?? "Microphone capture could not start.");
    } catch (error) {
      setMicrophoneError(errorMessage(error));
      await refreshCaptureStatus();
    }
  }

  async function startApplicationTest() {
    const processId = Number(selectedProcessId);
    const activationReady = !import.meta.env.DEV
      || (activationProbe.state === "succeeded" && activationProbe.processId === processId);
    if (!processId || !capability.available || !activationReady || isCaptureActive(captureStatus.state)) return;
    setApplicationError(null);
    try {
      const result = await invoke<AudioCaptureCommandResult>("start_process_audio_test", { processId });
      setCaptureStatus(result.status);
      if (!result.success) setApplicationError(result.error?.message ?? "Application audio capture could not start.");
    } catch (error) {
      setApplicationError(errorMessage(error));
      await refreshCaptureStatus();
    }
  }

  const selectedMicrophone = microphones.find((device) => device.id === selectedMicrophoneId);
  const selectedApplication = applications.find((application) => String(application.processId) === selectedProcessId);
  const captureActive = isCaptureActive(captureStatus.state);
  const selectedProcessNumber = Number(selectedProcessId);
  const activationReady = !import.meta.env.DEV
    || (activationProbe.state === "succeeded" && activationProbe.processId === selectedProcessNumber);

  return (
    <div className="audio-test">
      <div className="audio-test-heading">
        <div>
          <span className="dev-label">Development proof</span>
          <p>Independent 10-second WASAPI tests. Audio is not connected to Replay Buffer or video.</p>
        </div>
        <span className={`audio-capability ${capability.available ? "available" : "unavailable"}`}>
          Process loopback: {capability.available ? "Available" : "Unavailable"}
        </span>
      </div>

      <div className="audio-test-tabs" role="tablist" aria-label="Audio capture source">
        <button className={tab === "microphone" ? "active" : ""} type="button" onClick={() => setTab("microphone")}>Microphone</button>
        <button className={tab === "application" ? "active" : ""} type="button" onClick={() => setTab("application")}>Application Audio</button>
      </div>

      {tab === "microphone" ? (
        <div className="audio-test-panel" role="tabpanel">
          <div className="audio-test-source-row">
            <label>
              <span>Input endpoint</span>
              <select value={selectedMicrophoneId} onChange={(event) => setSelectedMicrophoneId(event.target.value)} disabled={captureActive || microphonesLoading}>
                {!microphones.length && <option value="">No active microphones detected</option>}
                {microphones.map((device) => (
                  <option key={device.id} value={device.id}>
                    {device.friendlyName}{device.isDefaultCommunications ? " — Communications default" : device.isDefaultMultimedia ? " — Multimedia default" : ""}
                  </option>
                ))}
              </select>
            </label>
            <button className="secondary-button" type="button" onClick={() => void refreshMicrophones()} disabled={captureActive || microphonesLoading}>
              {microphonesLoading ? "Refreshing..." : "Refresh"}
            </button>
          </div>
          {selectedMicrophone && (
            <SourceIdentity
              title={selectedMicrophone.friendlyName}
              details={[selectedMicrophone.state, selectedMicrophone.isDefaultCommunications ? "Default communications input" : selectedMicrophone.isDefaultMultimedia ? "Default multimedia input" : "Non-default input"]}
              path={selectedMicrophone.id}
            />
          )}
          {microphoneError && <p className="audio-test-error" role="alert">{microphoneError}</p>}
          <button className="primary-button audio-record-button" type="button" onClick={() => void startMicrophoneTest()} disabled={!selectedMicrophoneId || captureActive || microphonesLoading}>
            {captureStatus.kind === "microphone" && captureActive ? captureButtonLabel(captureStatus.state) : "Record 10 Second Mic Test"}
          </button>
        </div>
      ) : (
        <div className="audio-test-panel" role="tabpanel">
          <p className="audio-capability-detail">{capability.status}</p>
          <div className="audio-test-source-row">
            <label>
              <span>Active render process</span>
              <select value={selectedProcessId} onChange={(event) => {
                setSelectedProcessId(event.target.value);
                setActivationProbe(initialActivationProbe);
              }} disabled={captureActive || applicationsLoading || !capability.available || activationProbe.state === "running"}>
                {!applications.length && <option value="">No active audio applications detected</option>}
                {applications.map((application) => (
                  <option key={application.processId} value={application.processId}>
                    {application.displayName} — {application.processName} — PID {application.processId}
                  </option>
                ))}
              </select>
            </label>
            <button className="secondary-button" type="button" onClick={() => void refreshApplications()} disabled={captureActive || applicationsLoading || activationProbe.state === "running"}>
              {applicationsLoading ? "Refreshing..." : "Refresh"}
            </button>
          </div>
          {selectedApplication && (
            <SourceIdentity
              title={selectedApplication.displayName}
              details={[
                selectedApplication.processName,
                `PID ${selectedApplication.processId}`,
                `${selectedApplication.sessionCount} session${selectedApplication.sessionCount === 1 ? "" : "s"}`,
                `${selectedApplication.renderEndpointCount} render endpoint${selectedApplication.renderEndpointCount === 1 ? "" : "s"}`,
              ]}
              path={selectedApplication.executablePath}
            />
          )}
          {applicationError && <p className="audio-test-error" role="alert">{applicationError}</p>}
          {import.meta.env.DEV && (
            <div className="audio-activation-probe">
              <button className="secondary-button" type="button" onClick={() => void probeApplicationActivation()} disabled={!selectedProcessId || !capability.available || captureActive || applicationsLoading || activationProbe.state === "running"}>
                {activationProbe.state === "running" ? "Activating Process Audio..." : "Activate Selected Process Audio"}
              </button>
              {activationProbe.state === "succeeded" && <span className="audio-activation-state">Activation: Passed</span>}
              <p className={activationProbe.state} aria-live="polite">
                {activationProbe.message ?? "Run activation-only first. This obtains and releases IAudioClient without initializing capture or creating a WAV."}
              </p>
            </div>
          )}
          <button className="primary-button audio-record-button" type="button" onClick={() => void startApplicationTest()} disabled={!selectedProcessId || !capability.available || !activationReady || captureActive || applicationsLoading || activationProbe.state === "running"}>
            {captureStatus.kind === "processLoopback" && captureActive ? captureButtonLabel(captureStatus.state) : "Record 10 Second App Audio Test"}
          </button>
        </div>
      )}

      <CaptureResult status={captureStatus} />
    </div>
  );
}

function SourceIdentity({ title, details, path }: { title: string; details: string[]; path: string | null }) {
  return (
    <div className="audio-source-identity">
      <strong>{title}</strong>
      <div>{details.map((detail) => <span key={detail}>{detail}</span>)}</div>
      {path && <code title={path}>{path}</code>}
    </div>
  );
}

function CaptureResult({ status }: { status: AudioCaptureStatus }) {
  if (status.state === "idle") {
    return <div className="audio-capture-result idle">No audio test has run in this session.</div>;
  }
  return (
    <div className={`audio-capture-result ${status.state}`} aria-live="polite">
      <div className="audio-result-title">
        <strong>{formatCaptureState(status.state)}</strong>
        {status.targetLabel && <span>{status.targetLabel}</span>}
      </div>
      {status.error && <p className="audio-test-error">{status.error.message}</p>}
      {status.outputPath && <code className="audio-output-path">{status.outputPath}</code>}
      {(status.formatDiagnostics || status.format || status.timing) && <details className="audio-test-diagnostics">
        <summary>Advanced diagnostics</summary>
        {status.kind === "processLoopback" && status.formatDiagnostics && (
          <div className="audio-telemetry-grid process-format-diagnostics">
            <Metric label="GetMixFormat" value={status.formatDiagnostics.getMixFormatStatus} />
            <Metric label="Format role" value={status.formatDiagnostics.formatRole} />
          </div>
        )}
        {status.format && (
          <div className="audio-telemetry-grid">
            <Metric label={status.kind === "processLoopback" ? "Client capture format" : "Endpoint mix format"} value={status.format.sampleFormat} />
            <Metric label="Sample rate" value={`${status.format.sampleRate.toLocaleString()} Hz`} />
            <Metric label="Channels" value={String(status.format.channelCount)} />
            <Metric label="Bits / valid bits" value={`${status.format.bitsPerSample} / ${status.format.validBitsPerSample ?? "n/a"}`} />
            <Metric label="Block align" value={`${status.format.blockAlign} bytes`} />
            <Metric label="Format tag" value={`0x${status.format.formatTag.toString(16).padStart(4, "0")}`} />
          </div>
        )}
        {status.timing && <TimingMetrics timing={status.timing} />}
      </details>}
    </div>
  );
}

function TimingMetrics({ timing }: { timing: AudioTimingTelemetry }) {
  return (
    <div className="audio-telemetry-grid timing">
      <Metric label="Wall duration" value={`${timing.actualWallClockDurationMs.toFixed(1)} ms`} />
      <Metric label="Captured duration" value={`${timing.expectedDurationFromCapturedFramesMs.toFixed(1)} ms`} />
      <Metric label="WAV duration" value={`${timing.expectedDurationInWavMs.toFixed(1)} ms`} />
      <Metric label="Captured / written frames" value={`${timing.capturedSampleFrames.toLocaleString()} / ${timing.writtenSampleFrames.toLocaleString()}`} />
      <Metric label="Packets / silent" value={`${timing.audioPacketCount} / ${timing.silentPacketCount}`} />
      <Metric label="Discontinuities" value={String(timing.discontinuityCount)} />
      <Metric label="Timestamp errors" value={String(timing.timestampErrorCount)} />
      <Metric label="Queue max / capacity" value={`${timing.maximumQueueDepth} / ${timing.queueCapacityPackets}`} />
      <Metric label="Queue full events" value={String(timing.queueFullEvents)} />
      <Metric label="Dropped packets / frames" value={`${timing.deliberatelyDroppedPackets} / ${timing.deliberatelyDroppedFrames}`} />
      <Metric label="Device position first / last" value={`${nullableNumber(timing.firstDevicePosition)} / ${nullableNumber(timing.lastDevicePosition)}`} />
      <Metric label="Packet QPC 100 ns first / last" value={`${nullableNumber(timing.firstQpcPosition100ns)} / ${nullableNumber(timing.lastQpcPosition100ns)}`} />
      <Metric label="Capture QPC start / end" value={`${timing.monotonicCaptureStartQpc} / ${timing.monotonicCaptureEndQpc}`} />
      <Metric label="QPC frequency" value={`${timing.qpcFrequency.toLocaleString()} Hz`} />
    </div>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return <div><span>{label}</span><strong>{value}</strong></div>;
}

function isCaptureActive(state: AudioCaptureStatus["state"]) {
  return state === "preparing" || state === "recording" || state === "finalizing";
}

function captureButtonLabel(state: AudioCaptureStatus["state"]) {
  if (state === "preparing") return "Preparing Audio...";
  if (state === "recording") return "Recording 10 Seconds...";
  return "Finalizing WAV...";
}

function formatCaptureState(state: AudioCaptureStatus["state"]) {
  return state.charAt(0).toUpperCase() + state.slice(1);
}

function nullableNumber(value: number | null) {
  return value === null ? "n/a" : value.toLocaleString();
}

function errorMessage(error: unknown) {
  return error instanceof Error ? error.message : String(error);
}
