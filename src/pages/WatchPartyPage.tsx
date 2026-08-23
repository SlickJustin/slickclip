import { useCallback, useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

type MonitorTarget = { id: string; displayIndex: number; friendlyName: string; width: number; height: number; primary: boolean };
type WindowTarget = { id: string; title: string; processName: string | null; processId: number; width: number; height: number };
type TargetListResult<T> = { success: boolean; targets: T[]; errorMessage: string | null };
type ApplicationAudioProcess = { processId: number; displayName: string; processName: string };
type MicrophoneEndpoint = { id: string; friendlyName: string; isDefaultCommunications: boolean };
type AudioListResult<T> = { success: boolean; devices?: T[]; applications?: T[]; error: { message: string } | null };
type TargetType = "monitor" | "window";
type Layout = "reactionsRight" | "reactionStrip" | "pictureInPicture";
type SourceStatus = { label: string | null; width: number; height: number; framesReceived: number; closed: boolean; errorMessage: string | null };
type WatchPartyStatus = {
  state: "stopped" | "starting" | "recording" | "stopping" | "finalizing" | "completed" | "error";
  sessionId: string | null;
  layout: Layout;
  elapsedSeconds: number;
  finalizedSegmentCount: number;
  framesComposed: number;
  mainSource: SourceStatus;
  reactionSource: SourceStatus;
  outputPath: string | null;
  errorMessage: string | null;
  recoverableSessionIds: string[];
};
type CommandResult = { success: boolean; status: WatchPartyStatus; errorMessage: string | null };

const stoppedStatus: WatchPartyStatus = {
  state: "stopped", sessionId: null, layout: "reactionsRight", elapsedSeconds: 0,
  finalizedSegmentCount: 0, framesComposed: 0,
  mainSource: { label: null, width: 0, height: 0, framesReceived: 0, closed: false, errorMessage: null },
  reactionSource: { label: null, width: 0, height: 0, framesReceived: 0, closed: false, errorMessage: null },
  outputPath: null, errorMessage: null, recoverableSessionIds: [],
};

export function WatchPartyPage() {
  const [monitors, setMonitors] = useState<MonitorTarget[]>([]);
  const [windows, setWindows] = useState<WindowTarget[]>([]);
  const [applications, setApplications] = useState<ApplicationAudioProcess[]>([]);
  const [microphones, setMicrophones] = useState<MicrophoneEndpoint[]>([]);
  const [mainType, setMainType] = useState<TargetType>("monitor");
  const [mainId, setMainId] = useState("");
  const [reactionId, setReactionId] = useState("");
  const [mainAudioPid, setMainAudioPid] = useState("");
  const [discordAudioPid, setDiscordAudioPid] = useState("");
  const [microphoneId, setMicrophoneId] = useState("");
  const [layout, setLayout] = useState<Layout>("reactionsRight");
  const [status, setStatus] = useState<WatchPartyStatus>(stoppedStatus);
  const [loading, setLoading] = useState(false);
  const [loadError, setLoadError] = useState<string | null>(null);

  const discordWindows = useMemo(() => windows.filter((window) => {
    const name = (window.processName ?? "").replace(/\.exe$/i, "").toLowerCase();
    return name === "discord" || name.startsWith("discord");
  }), [windows]);
  const discordApplications = useMemo(() => applications.filter((application) => application.processName.replace(/\.exe$/i, "").toLowerCase().startsWith("discord")), [applications]);
  const reactionWindow = discordWindows.find((window) => window.id === reactionId) ?? null;
  const active = ["starting", "recording", "stopping", "finalizing"].includes(status.state);

  const refreshSources = useCallback(async () => {
    setLoading(true);
    setLoadError(null);
    try {
      const [monitorResult, windowResult, appResult, microphoneResult] = await Promise.all([
        invoke<TargetListResult<MonitorTarget>>("list_capture_monitors"),
        invoke<TargetListResult<WindowTarget>>("list_capture_windows"),
        invoke<AudioListResult<ApplicationAudioProcess>>("list_application_audio_processes"),
        invoke<AudioListResult<MicrophoneEndpoint>>("list_audio_microphones"),
      ]);
      if (!monitorResult.success) throw new Error(monitorResult.errorMessage ?? "Displays could not be loaded.");
      if (!windowResult.success) throw new Error(windowResult.errorMessage ?? "Windows could not be loaded.");
      if (!appResult.success) throw new Error(appResult.error?.message ?? "Application audio could not be loaded.");
      if (!microphoneResult.success) throw new Error(microphoneResult.error?.message ?? "Microphones could not be loaded.");
      setMonitors(monitorResult.targets);
      setWindows(windowResult.targets);
      setApplications(appResult.applications ?? []);
      setMicrophones(microphoneResult.devices ?? []);
      setMainId((current) => current || monitorResult.targets[0]?.id || "");
      const discord = windowResult.targets.find((window) => /discord/i.test(window.processName ?? ""));
      setReactionId((current) => current || discord?.id || "");
      const discordAudio = (appResult.applications ?? []).find((application) => /^discord(?:\.exe)?$/i.test(application.processName))
        ?? (appResult.applications ?? []).find((application) => /^discord/i.test(application.processName));
      setDiscordAudioPid((current) => current || String(discordAudio?.processId ?? ""));
      const defaultMic = (microphoneResult.devices ?? []).find((device) => device.isDefaultCommunications) ?? microphoneResult.devices?.[0];
      setMicrophoneId((current) => current || defaultMic?.id || "");
    } catch (error) {
      setLoadError(error instanceof Error ? error.message : String(error));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => { void refreshSources(); }, [refreshSources]);
  useEffect(() => {
    let disposed = false;
    const poll = async () => {
      try {
        const next = await invoke<WatchPartyStatus>("get_watch_party_status");
        if (!disposed) setStatus(next);
      } catch { /* retain the last actionable status */ }
    };
    void poll();
    const timer = window.setInterval(poll, active ? 500 : 2_000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [active]);

  const start = async () => {
    if (!mainId || !reactionWindow || !mainAudioPid || !discordAudioPid || !microphoneId) return;
    const mainAudio = applications.find((application) => String(application.processId) === mainAudioPid);
    const microphone = microphones.find((device) => device.id === microphoneId);
    const result = await invoke<CommandResult>("start_watch_party", { request: {
      mainTarget: { targetType: mainType, id: mainId }, reactionWindowId: reactionWindow.id, layout,
      audio: { tracks: [
        { role: "game", enabled: true, sourceKind: "process", processId: Number(mainAudioPid), sourceLabel: mainAudio?.displayName ?? "Main Content" },
        { role: "voiceChat", enabled: true, sourceKind: "process", processId: Number(discordAudioPid), sourceLabel: "Discord / Voice Chat" },
        { role: "microphone", enabled: true, sourceKind: "microphone", endpointId: microphoneId, sourceLabel: microphone?.friendlyName ?? "Microphone" },
      ] },
    } });
    setStatus(result.status);
  };

  const stop = async () => setStatus((await invoke<CommandResult>("stop_watch_party")).status);
  const recover = async (sessionId: string) => setStatus((await invoke<CommandResult>("recover_watch_party", { sessionId })).status);
  const selectedTargets = mainType === "monitor" ? monitors : windows;
  const canStart = !active && Boolean(mainId && reactionWindow && mainAudioPid && discordAudioPid && microphoneId);

  return <div className="page watch-party-page">
    <header className="page-header"><div><span className="eyebrow">LONG-FORM RECORDING</span><h1>Watch Party</h1><p>Record main content with one whole Discord reaction window and independent audio tracks.</p></div><span className="demo-badge">BETA</span></header>

    <section className={`status-card watch-party-status watch-party-status-${status.state}`}>
      <div><span className="eyebrow">SESSION STATUS</span><div className="replay-state"><span className="status-dot" />{formatState(status.state)}</div>
        <p>{status.state === "recording" ? `${formatDuration(status.elapsedSeconds)} recorded · ${status.finalizedSegmentCount} safe segments` : status.state === "finalizing" ? "Finalizing video, exact audio tracks, and Library entry…" : status.state === "completed" ? "Watch Party saved to your Clips Library." : "Configure both visual sources and all three audio tracks."}</p>
        {status.outputPath && <code>{status.outputPath}</code>}
        {status.errorMessage && <p className="replay-buffer-error">{status.errorMessage}</p>}
      </div>
      <button className="save-replay-button" type="button" disabled={!active || status.state === "finalizing"} onClick={() => void stop()}>Stop &amp; Finalize</button>
    </section>

    <div className="watch-party-grid">
      <section className="panel watch-party-panel"><div className="watch-party-panel-heading"><div><span className="eyebrow">VISUAL SOURCES</span><h2>Main content</h2></div><button className="capture-target-refresh" type="button" disabled={active || loading} onClick={() => void refreshSources()}>{loading ? "Refreshing…" : "Refresh"}</button></div>
        <div className="capture-target-tabs"><button className={mainType === "monitor" ? "capture-target-tab-active" : ""} disabled={active} onClick={() => { setMainType("monitor"); setMainId(monitors[0]?.id ?? ""); }}>Display</button><button className={mainType === "window" ? "capture-target-tab-active" : ""} disabled={active} onClick={() => { setMainType("window"); setMainId(windows[0]?.id ?? ""); }}>Window</button></div>
        <select value={mainId} disabled={active} onChange={(event) => setMainId(event.target.value)}><option value="">Select main content…</option>{selectedTargets.map((target) => <option key={target.id} value={target.id}>{"displayIndex" in target ? `Display ${target.displayIndex} · ${target.friendlyName}` : `${target.processName ?? "Application"} · ${target.title}`}</option>)}</select>
        <label>Main Content audio<select value={mainAudioPid} disabled={active} onChange={(event) => setMainAudioPid(event.target.value)}><option value="">Select application audio…</option>{applications.map((application) => <option key={application.processId} value={application.processId}>{application.displayName} · {application.processName}</option>)}</select></label>
      </section>

      <section className="panel watch-party-panel"><span className="eyebrow">REACTIONS</span><h2>Discord window</h2><p className="watch-party-help">SlickClip captures this entire Discord window. Participants may join, leave, or rearrange naturally inside Discord.</p>
        <select value={reactionId} disabled={active} onChange={(event) => setReactionId(event.target.value)}><option value="">Select a Discord window…</option>{discordWindows.map((window) => <option key={window.id} value={window.id}>{window.title} · {window.width}×{window.height}</option>)}</select>
        {!loading && discordWindows.length === 0 && <p className="capture-target-load-error">No capturable Discord desktop window found. Open Discord, then Refresh.</p>}
        <label>Discord / Voice Chat audio<select value={discordAudioPid} disabled={active} onChange={(event) => setDiscordAudioPid(event.target.value)}><option value="">Select Discord audio…</option>{discordApplications.map((application) => <option key={application.processId} value={application.processId}>{application.displayName} · {application.processName}</option>)}</select></label>
        <label>Microphone<select value={microphoneId} disabled={active} onChange={(event) => setMicrophoneId(event.target.value)}><option value="">Select microphone…</option>{microphones.map((microphone) => <option key={microphone.id} value={microphone.id}>{microphone.friendlyName}{microphone.isDefaultCommunications ? " · Default communications" : ""}</option>)}</select></label>
      </section>
    </div>

    <section className="panel watch-party-panel"><span className="eyebrow">COMPOSITION</span><h2>Layout</h2><div className="watch-party-layouts">{([
      ["reactionsRight", "Reactions right", "Main content with a dedicated right reaction rail."],
      ["reactionStrip", "Reaction strip", "Main content above a full-width reaction strip."],
      ["pictureInPicture", "Picture in picture", "Main content full-frame with Discord inset."],
    ] as [Layout, string, string][]).map(([id, name, detail]) => <button type="button" key={id} disabled={active} className={layout === id ? "selected" : ""} onClick={() => setLayout(id)}><span className={`watch-party-layout-preview ${id}`} aria-hidden="true"><i /><b /></span><strong>{name}</strong><small>{detail}</small></button>)}</div>
      {loadError && <p className="capture-target-load-error">{loadError}</p>}
      <button className="save-replay-button watch-party-start" type="button" disabled={!canStart} onClick={() => void start()}>Start Watch Party</button>
      {!canStart && !active && <span className="disabled-reason">Choose main video/audio, one Discord window/audio session, and a microphone.</span>}
    </section>

    {active && <section className="panel watch-party-panel"><span className="eyebrow">LIVE SOURCES</span><div className="watch-party-source-health"><SourceHealth name="Main content" source={status.mainSource} /><SourceHealth name="Discord reactions" source={status.reactionSource} /></div></section>}
    {status.recoverableSessionIds.length > 0 && !active && <section className="panel watch-party-panel"><span className="eyebrow">RECOVERY</span><h2>Finalized material found</h2><p className="watch-party-help">Recoverable sessions contain only atomically checkpointed, finalized segments. Recovery creates a video-only Library clip.</p>{status.recoverableSessionIds.map((id) => <div className="watch-party-recovery" key={id}><code>{id}</code><button type="button" onClick={() => void recover(id)}>Recover</button></div>)}</section>}
  </div>;
}

function SourceHealth({ name, source }: { name: string; source: SourceStatus }) {
  return <div className={source.closed ? "source-lost" : ""}><strong>{name}</strong><span>{source.label ?? "Waiting for source"}</span><small>{source.width}×{source.height} · {source.framesReceived.toLocaleString()} frames</small>{source.errorMessage && <small>{source.errorMessage}</small>}</div>;
}

function formatState(state: WatchPartyStatus["state"]) {
  return ({ stopped: "Ready", starting: "Starting", recording: "Recording", stopping: "Stopping", finalizing: "Finalizing", completed: "Completed", error: "Needs attention" })[state];
}

function formatDuration(seconds: number) {
  const total = Math.floor(seconds); return `${Math.floor(total / 3600)}:${String(Math.floor(total / 60) % 60).padStart(2, "0")}:${String(total % 60).padStart(2, "0")}`;
}
