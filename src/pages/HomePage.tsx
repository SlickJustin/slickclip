import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ClipPlayer } from "../components/ClipPlayer";
import { ClipThumbnail } from "../components/ClipThumbnail";
import type {
  ClipActionResponse,
  ClipListItem,
  ClipListResponse,
  ClipMutationResponse,
  ClipSortOrder,
  UiPreferences,
  UiPreferencesPatch,
  UiPreferencesResponse,
} from "../types/clips";
import {
  defaultUiPreferences,
  errorMessage,
  formatDuration100ns,
  formatFps,
} from "../types/clips";
import { formatReplayWindow } from "../utils/replayGuidance";
import {
  combinationFromKeyboardEvent,
  isModifierCode,
  shortcutDraftFromKeyboardEvent,
} from "../lib/hotkeyShortcut";

type Toast = (title: string, message: string, success: boolean) => void;

type Props = {
  onEditClip: (clip: ClipListItem) => void;
  onOpenClips: () => void;
  onOpenReplay: () => void;
  onOpenSettings: () => void;
  onToast: Toast;
};

type ReplayStatus = {
  state: "stopped" | "starting" | "running" | "stopping" | "error";
  errorMessage: string | null;
  targetId: string | null;
  targetLabel: string | null;
  actualEncoder: string | null;
  captureHealth: string;
  replayDurationSeconds: number;
  frameRate: number;
  width: number;
  height: number;
};

type GameDetectionStatus = {
  enabled: boolean;
  autoArmEnabled: boolean;
  replayState: string;
  candidates: { targetId: string; title: string; processName: string; processId: number }[];
};

type HotkeyState = {
  registered: boolean;
  currentCombination: string;
  lastRegistrationError?: string | null;
};

type HotkeyCommandResult = { success: boolean; state: HotkeyState; errorMessage: string | null };
type RecordingHotkey = "saveReplay" | "saveAndName";
type MonitorTarget = { id: string; displayIndex: number; friendlyName: string; width: number; height: number; primary: boolean };
type TargetListResult<T> = { success: boolean; targets: T[]; errorMessage: string | null };
type ReplayCommandResult = { success: boolean; status: ReplayStatus; errorMessage: string | null };

type QuickPanel = "capture" | "automatic" | "specs" | "hotkeys" | "settings";

const defaultReplayStatus: ReplayStatus = {
  state: "stopped",
  errorMessage: null,
  targetId: null,
  targetLabel: null,
  actualEncoder: null,
  captureHealth: "Idle",
  replayDurationSeconds: 120,
  frameRate: 60,
  width: 2560,
  height: 1440,
};

export function HomePage({ onEditClip, onOpenClips, onOpenReplay, onOpenSettings, onToast }: Props) {
  const [replay, setReplay] = useState<ReplayStatus>(defaultReplayStatus);
  const [detection, setDetection] = useState<GameDetectionStatus | null>(null);
  const [hotkey, setHotkey] = useState<HotkeyState | null>(null);
  const [saveAndNameHotkey, setSaveAndNameHotkey] = useState<HotkeyState | null>(null);
  const [quickPanel, setQuickPanel] = useState<QuickPanel | null>(null);
  const [quickPending, setQuickPending] = useState(false);
  const [quickMessage, setQuickMessage] = useState<{ text: string; success: boolean } | null>(null);
  const [recordingHotkey, setRecordingHotkey] = useState<RecordingHotkey | null>(null);
  const [hotkeyDraft, setHotkeyDraft] = useState("Press a shortcut…");
  const [monitors, setMonitors] = useState<MonitorTarget[]>([]);
  const [captureTargetId, setCaptureTargetId] = useState("");
  const [preferences, setPreferences] = useState<UiPreferences>(defaultUiPreferences);
  const [preferencesLoaded, setPreferencesLoaded] = useState(false);
  const [clips, setClips] = useState<ClipListItem[]>([]);
  const [totalCount, setTotalCount] = useState(0);
  const [playingClip, setPlayingClip] = useState<ClipListItem | null>(null);
  const [searchText, setSearchText] = useState("");
  const [sortOrder, setSortOrder] = useState<ClipSortOrder>("newestFirst");
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const captureToolsRef = useRef<HTMLDivElement>(null);

  const replaceClip = useCallback((updated: ClipListItem) => {
    setClips((current) => current.map((clip) => clip.id === updated.id ? updated : clip));
    setPlayingClip((current) => current?.id === updated.id ? updated : current);
  }, []);

  const persistPreferences = useCallback(async (patch: UiPreferencesPatch) => {
    setPreferences((current) => ({ ...current, ...patch }));
    const response = await invoke<UiPreferencesResponse>("update_ui_preferences", { patch });
    if (!response.success) throw new Error(response.errorMessage ?? "Playback preferences could not be saved.");
    setPreferences(response.preferences);
  }, []);

  const loadClips = useCallback(async () => {
    try {
      const response = await invoke<ClipListResponse>("list_clips", {
        request: {
          searchText,
          favoritesOnly: false,
          recentlyWatchedOnly: false,
          collectionId: null,
          sortOrder,
          limit: 12,
          offset: 0,
        },
      });
      if (!response.success) throw new Error(response.errorMessage ?? "Your clips could not be loaded.");
      setClips(response.clips);
      setTotalCount(response.totalCount);
      setError(null);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setLoading(false);
    }
  }, [searchText, sortOrder]);

  const refreshCaptureState = useCallback(async () => {
    const [replayStatus, gameDetection, hotkeyState, saveAndNameHotkeyState] = await Promise.all([
      invoke<ReplayStatus>("get_replay_buffer_status"),
      invoke<GameDetectionStatus>("get_game_detection_status"),
      invoke<HotkeyState>("get_save_replay_hotkey"),
      invoke<HotkeyState>("get_save_and_name_hotkey"),
    ]);
    setReplay(replayStatus);
    setDetection(gameDetection);
    setHotkey(hotkeyState);
    setSaveAndNameHotkey(saveAndNameHotkeyState);
  }, []);

  useEffect(() => {
    void invoke<UiPreferencesResponse>("get_ui_preferences")
      .then((response) => setPreferences(response.preferences))
      .catch(() => undefined)
      .finally(() => setPreferencesLoaded(true));
  }, []);

  useEffect(() => {
    const timer = window.setTimeout(() => void loadClips(), 160);
    return () => window.clearTimeout(timer);
  }, [loadClips]);

  useEffect(() => {
    void refreshCaptureState().catch(() => undefined);
    const timer = window.setInterval(() => void refreshCaptureState().catch(() => undefined), 1_000);
    let unlistenReplay: UnlistenFn | undefined;
    let unlistenLibrary: UnlistenFn | undefined;
    let disposed = false;
    void listen<ReplayStatus>("replay-buffer-status-changed", (event) => setReplay(event.payload))
      .then((cleanup) => {
        if (disposed) cleanup(); else unlistenReplay = cleanup;
      })
      .catch(() => undefined);
    void listen<string>("clip-library-changed", () => void loadClips())
      .then((cleanup) => {
        if (disposed) cleanup(); else unlistenLibrary = cleanup;
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      window.clearInterval(timer);
      unlistenReplay?.();
      unlistenLibrary?.();
    };
  }, [loadClips, refreshCaptureState]);

  useEffect(() => {
    if (!quickPanel) return;

    const closeOnOutsideClick = (event: PointerEvent) => {
      if (!captureToolsRef.current?.contains(event.target as Node)) {
        if (recordingHotkey) void stopHotkeyRecording();
        setQuickPanel(null);
      }
    };
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setQuickPanel(null);
    };

    document.addEventListener("pointerdown", closeOnOutsideClick);
    document.addEventListener("keydown", closeOnEscape);
    return () => {
      document.removeEventListener("pointerdown", closeOnOutsideClick);
      document.removeEventListener("keydown", closeOnEscape);
    };
  }, [quickPanel, recordingHotkey]);

  useEffect(() => {
    setQuickMessage(null);
    if (quickPanel !== "capture") return;
    void invoke<TargetListResult<MonitorTarget>>("list_capture_monitors")
      .then((result) => {
        setMonitors(result.targets);
        setCaptureTargetId((current) => current
          || detection?.candidates[0]?.targetId
          || result.targets.find((monitor) => monitor.primary)?.id
          || result.targets[0]?.id
          || "");
        if (!result.success && result.errorMessage) setQuickMessage({ text: result.errorMessage, success: false });
      })
      .catch((cause) => setQuickMessage({ text: errorMessage(cause), success: false }));
  }, [quickPanel]);

  useEffect(() => {
    if (!recordingHotkey) return;
    const handleKeyDown = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (event.repeat) return;
      if (isModifierCode(event.code)) {
        setHotkeyDraft(shortcutDraftFromKeyboardEvent(event));
        return;
      }
      if (event.code === "Escape" && !event.ctrlKey && !event.shiftKey && !event.altKey && !event.metaKey) {
        void stopHotkeyRecording();
        return;
      }
      const combination = combinationFromKeyboardEvent(event);
      if (!combination) {
        setQuickMessage({ text: "That key cannot be used as a global shortcut.", success: false });
        return;
      }
      setHotkeyDraft(combination);
      void submitHotkey(recordingHotkey, combination);
    };
    const handleKeyUp = (event: KeyboardEvent) => {
      event.preventDefault();
      event.stopPropagation();
      if (isModifierCode(event.code)) setHotkeyDraft(shortcutDraftFromKeyboardEvent(event));
    };
    window.addEventListener("keydown", handleKeyDown, true);
    window.addEventListener("keyup", handleKeyUp, true);
    return () => {
      window.removeEventListener("keydown", handleKeyDown, true);
      window.removeEventListener("keyup", handleKeyUp, true);
    };
  }, [recordingHotkey]);

  async function setFavorite(clip: ClipListItem) {
    try {
      const response = await invoke<ClipMutationResponse>("set_clip_favorite", {
        request: { clipId: clip.id, favorite: !clip.favorite },
      });
      if (!response.success || !response.clip) throw new Error(response.errorMessage ?? "Favorite update failed.");
      replaceClip(response.clip);
    } catch (cause) {
      onToast("Could not update favorite", errorMessage(cause), false);
    }
  }

  async function copyClip(clip: ClipListItem) {
    try {
      const response = await invoke<ClipActionResponse>("copy_clip_to_clipboard", { request: { clipId: clip.id } });
      if (!response.success) throw new Error(response.errorMessage ?? "The Windows clipboard rejected the clip.");
      onToast("Clip copied", "Paste it into Discord with Ctrl+V.", true);
    } catch (cause) {
      onToast("Could not copy clip", errorMessage(cause), false);
    }
  }

  async function updateQuickPreferences(patch: UiPreferencesPatch, successMessage: string) {
    if (quickPending) return;
    setQuickPending(true);
    setQuickMessage(null);
    try {
      const response = await invoke<UiPreferencesResponse>("update_ui_preferences", { patch });
      if (!response.success) throw new Error(response.errorMessage ?? "That setting could not be saved.");
      setPreferences(response.preferences);
      setQuickMessage({ text: successMessage, success: true });
      void refreshCaptureState().catch(() => undefined);
    } catch (cause) {
      setQuickMessage({ text: errorMessage(cause), success: false });
    } finally {
      setQuickPending(false);
    }
  }

  async function toggleReplayFromHome() {
    if (quickPending) return;
    setQuickPending(true);
    setQuickMessage(null);
    try {
      if (replay.state === "running" || replay.state === "starting") {
        const result = await invoke<ReplayCommandResult>("stop_replay_buffer");
        setReplay(result.status);
        if (!result.success) throw new Error(result.errorMessage ?? "Replay could not be stopped.");
        setQuickMessage({ text: "Replay stopped safely.", success: true });
        return;
      }
      if (!captureTargetId) throw new Error("Choose a game or display first.");
      const candidate = detection?.candidates.find((item) => item.targetId === captureTargetId);
      await invoke("set_game_detection_manual_override", { targetId: captureTargetId });
      const result = await invoke<ReplayCommandResult>("start_replay_buffer", {
        request: {
          target: { targetType: captureTargetId.startsWith("monitor:") ? "monitor" : "window", id: captureTargetId },
          captureMode: "auto",
          encoder: preferences.replayEncoder,
          replayDurationSeconds: preferences.replayDurationSeconds,
          frameRate: preferences.replayFrameRate,
          quality: preferences.replayQuality,
          audio: {
            tracks: candidate ? [{
              role: "game",
              enabled: true,
              sourceKind: "process",
              processId: candidate.processId,
              sourceLabel: candidate.processName,
            }] : [],
          },
        },
      });
      setReplay(result.status);
      if (!result.success) throw new Error(result.errorMessage ?? "Replay could not be started.");
      setQuickMessage({ text: "Replay is starting with these settings.", success: true });
    } catch (cause) {
      setQuickMessage({ text: errorMessage(cause), success: false });
    } finally {
      setQuickPending(false);
      void refreshCaptureState().catch(() => undefined);
    }
  }

  async function startHotkeyRecording(target: RecordingHotkey) {
    if (quickPending) return;
    setQuickMessage(null);
    try {
      const command = target === "saveReplay" ? "set_hotkey_recorder_active" : "set_save_and_name_hotkey_recorder_active";
      const state = await invoke<HotkeyState>(command, { active: true });
      if (target === "saveReplay") setHotkey(state); else setSaveAndNameHotkey(state);
      setHotkeyDraft("Press a shortcut…");
      setRecordingHotkey(target);
    } catch (cause) {
      setQuickMessage({ text: errorMessage(cause), success: false });
    }
  }

  async function stopHotkeyRecording() {
    const target = recordingHotkey;
    if (!target) return;
    setRecordingHotkey(null);
    setHotkeyDraft("Press a shortcut…");
    try {
      const command = target === "saveReplay" ? "set_hotkey_recorder_active" : "set_save_and_name_hotkey_recorder_active";
      const state = await invoke<HotkeyState>(command, { active: false });
      if (target === "saveReplay") setHotkey(state); else setSaveAndNameHotkey(state);
    } catch (cause) {
      setQuickMessage({ text: errorMessage(cause), success: false });
    }
  }

  async function submitHotkey(target: RecordingHotkey, combination: string) {
    if (quickPending) return;
    setQuickPending(true);
    setQuickMessage(null);
    try {
      const command = target === "saveReplay" ? "set_save_replay_hotkey" : "set_save_and_name_hotkey";
      const result = await invoke<HotkeyCommandResult>(command, { combination });
      if (!result.success) throw new Error(result.errorMessage ?? "That hotkey could not be registered.");
      if (target === "saveReplay") setHotkey(result.state); else setSaveAndNameHotkey(result.state);
      setRecordingHotkey(null);
      setQuickMessage({ text: `${target === "saveReplay" ? "Save Replay" : "Save & Name"} changed to ${result.state.currentCombination}.`, success: true });
    } catch (cause) {
      setRecordingHotkey(null);
      setQuickMessage({ text: errorMessage(cause), success: false });
    } finally {
      setQuickPending(false);
    }
  }

  async function disableSaveAndNameHotkey() {
    if (quickPending) return;
    setQuickPending(true);
    setQuickMessage(null);
    try {
      const result = await invoke<HotkeyCommandResult>("clear_save_and_name_hotkey");
      if (!result.success) throw new Error(result.errorMessage ?? "Save & Name could not be disabled.");
      setSaveAndNameHotkey(result.state);
      setQuickMessage({ text: "Save & Name disabled.", success: true });
    } catch (cause) {
      setQuickMessage({ text: errorMessage(cause), success: false });
    } finally {
      setQuickPending(false);
    }
  }

  async function updateStartWithWindows(enabled: boolean) {
    if (quickPending) return;
    setQuickPending(true);
    setQuickMessage(null);
    try {
      const response = await invoke<UiPreferencesResponse>("set_start_with_windows", { enabled });
      if (!response.success) throw new Error(response.errorMessage ?? "Windows startup could not be changed.");
      setPreferences(response.preferences);
      setQuickMessage({ text: enabled ? "SlickClip will start with Windows." : "Windows startup disabled.", success: true });
    } catch (cause) {
      setQuickMessage({ text: errorMessage(cause), success: false });
    } finally {
      setQuickPending(false);
    }
  }

  const replayReady = replay.state === "running" && replay.captureHealth === "Healthy";
  const replayRecovering = replay.captureHealth === "Recovering";
  const captureLabel = replayReady
    ? "Replay Ready"
    : replayRecovering
      ? "Recovering capture"
      : replay.state === "starting"
        ? "Starting Replay"
        : replay.state === "error"
          ? "Capture needs attention"
          : "Waiting for a game";
  const captureDetail = replay.targetLabel
    ?? (detection?.candidates[0]?.title ? `Detected: ${detection.candidates[0].title}` : "Open a game or select a display manually");
  const autoCaptureLabel = preferences.gameDetectionEnabled && preferences.gameAutoArm ? "Automatic capture on" : "Automatic capture off";
  const autoCaptureDetail = preferences.gameDetectionEnabled
    ? preferences.gameAutoArm ? "Starts Replay for detected games" : "Detection is watching without starting"
    : "Game detection is disabled";
  const configuredDuration = replay.state === "running" && replay.replayDurationSeconds ? replay.replayDurationSeconds : preferences.replayDurationSeconds;
  const configuredFrameRate = replay.state === "running" && replay.frameRate ? replay.frameRate : preferences.replayFrameRate;
  const configuredEncoder = replay.state === "running" && replay.actualEncoder
    ? replay.actualEncoder
    : preferences.replayEncoder === "hevc" ? "HEVC" : preferences.replayEncoder === "h264" ? "H.264" : "Automatic";
  const resolution = replay.state === "running" && replay.width > 0 && replay.height > 0 ? `${replay.width}×${replay.height}` : "Display native";

  function toggleQuickPanel(panel: QuickPanel) {
    if (recordingHotkey) void stopHotkeyRecording();
    setQuickPanel((current) => current === panel ? null : panel);
  }

  function openFromQuickPanel(destination: () => void) {
    setQuickPanel(null);
    destination();
  }

  return (
    <div className="page page-home">
      <header className="home-header">
        <div>
          <span className="home-eyebrow">Capture workspace</span>
          <h1>Home</h1>
        </div>
        <button className="home-header-link" type="button" onClick={onOpenClips}>Open full Library <span aria-hidden="true">→</span></button>
      </header>

      <div className="home-capture-tools" ref={captureToolsRef}>
        <section className="home-capture-strip" aria-label="Replay status and capture settings">
          <button className={`home-capture-state${quickPanel === "capture" ? " quick-open" : ""}`} type="button" aria-haspopup="dialog" aria-expanded={quickPanel === "capture"} aria-controls="home-quick-panel" onClick={() => toggleQuickPanel("capture")}>
            <span className={`home-status-dot${replayReady ? " ready" : replay.state === "error" ? " error" : replayRecovering ? " recovering" : ""}`} aria-hidden="true" />
            <span><strong>{captureLabel}</strong><small>{captureDetail}</small></span>
            <span className="home-strip-chevron" aria-hidden="true">›</span>
          </button>
          <button className={`home-capture-state home-auto-state${quickPanel === "automatic" ? " quick-open" : ""}`} type="button" aria-haspopup="dialog" aria-expanded={quickPanel === "automatic"} aria-controls="home-quick-panel" onClick={() => toggleQuickPanel("automatic")}>
            <span className={`home-status-dot${preferences.gameDetectionEnabled && preferences.gameAutoArm ? " ready" : ""}`} aria-hidden="true" />
            <span><strong>{autoCaptureLabel}</strong><small>{autoCaptureDetail}</small></span>
          </button>
          <button className={`home-capture-specs${quickPanel === "specs" ? " quick-open" : ""}`} type="button" aria-label="Show Replay capture details" aria-haspopup="dialog" aria-expanded={quickPanel === "specs"} aria-controls="home-quick-panel" onClick={() => toggleQuickPanel("specs")}>
            <span><small>Window</small><strong>{formatReplayWindow(configuredDuration)}</strong></span>
            <i aria-hidden="true" />
            <span><small>Encoder</small><strong>{configuredEncoder}</strong></span>
            <i aria-hidden="true" />
            <span><small>Output</small><strong>{resolution} · {configuredFrameRate} FPS</strong></span>
          </button>
          <button className={`home-hotkey${quickPanel === "hotkeys" ? " quick-open" : ""}`} type="button" aria-haspopup="dialog" aria-expanded={quickPanel === "hotkeys"} aria-controls="home-quick-panel" onClick={() => toggleQuickPanel("hotkeys")}>
            <small>Save Replay</small>
            <kbd>{hotkey?.currentCombination || "Not set"}</kbd>
          </button>
          <button className={`home-settings-button${quickPanel === "settings" ? " quick-open" : ""}`} type="button" aria-label="Show quick settings" title="Quick settings" aria-haspopup="dialog" aria-expanded={quickPanel === "settings"} aria-controls="home-quick-panel" onClick={() => toggleQuickPanel("settings")}>
            <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1A1.7 1.7 0 0 0 9 4.6 1.7 1.7 0 0 0 10 3v-.2h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z" /></svg>
          </button>
        </section>

        {quickPanel && (
          <aside id="home-quick-panel" className={`home-quick-panel panel-${quickPanel}`} role="dialog" aria-labelledby="home-quick-panel-title">
            {quickPanel === "capture" && (
              <>
                <div className="home-quick-heading"><span className={`home-status-dot${replayReady ? " ready" : replay.state === "error" ? " error" : replayRecovering ? " recovering" : ""}`} aria-hidden="true" /><div><small>Replay status</small><h2 id="home-quick-panel-title">{captureLabel}</h2></div></div>
                <dl className="home-quick-details"><div><dt>Recording</dt><dd>{captureDetail}</dd></div><div><dt>Health</dt><dd>{replay.captureHealth}</dd></div></dl>
                {replay.state !== "running" && replay.state !== "starting" && <label className="home-quick-field"><span>Capture source</span><select value={captureTargetId} disabled={quickPending} onChange={(event) => setCaptureTargetId(event.target.value)}><option value="">Choose a game or display</option>{detection?.candidates.length ? <optgroup label="Detected games">{detection.candidates.map((candidate) => <option value={candidate.targetId} key={candidate.targetId}>{candidate.title}</option>)}</optgroup> : null}<optgroup label="Displays">{monitors.map((monitor) => <option value={monitor.id} key={monitor.id}>Display {monitor.displayIndex} · {monitor.friendlyName} · {monitor.width}×{monitor.height}{monitor.primary ? " (Primary)" : ""}</option>)}</optgroup></select></label>}
                {replay.state !== "running" && captureTargetId.startsWith("monitor:") && <p className="home-quick-copy">Manual display quick start records video. Use advanced controls first when you need to choose separate Game, Voice Chat, or Microphone sources.</p>}
                <button className="home-quick-primary" type="button" disabled={quickPending || replay.state === "stopping" || (!captureTargetId && replay.state !== "running" && replay.state !== "starting")} onClick={() => void toggleReplayFromHome()}>{quickPending ? "Working…" : replay.state === "running" || replay.state === "starting" ? "Stop Replay" : "Start Replay"}<span aria-hidden="true">→</span></button>
                <button className="home-quick-text-action" type="button" onClick={() => openFromQuickPanel(onOpenReplay)}>Audio sources &amp; advanced controls</button>
              </>
            )}
            {quickPanel === "automatic" && (
              <>
                <div className="home-quick-heading"><span className={`home-status-dot${preferences.gameDetectionEnabled && preferences.gameAutoArm ? " ready" : ""}`} aria-hidden="true" /><div><small>Game detection</small><h2 id="home-quick-panel-title">{autoCaptureLabel}</h2></div></div>
                <div className="home-quick-switches">
                  <label><span><strong>Game Detection</strong><small>Watch for likely games</small></span><input type="checkbox" checked={preferences.gameDetectionEnabled} disabled={quickPending} onChange={(event) => void updateQuickPreferences({ gameDetectionEnabled: event.target.checked, gameAutoArm: event.target.checked ? preferences.gameAutoArm : false }, event.target.checked ? "Game Detection enabled." : "Game Detection disabled.")} /></label>
                  <label><span><strong>Start Replay automatically</strong><small>When a stable game is found</small></span><input type="checkbox" checked={preferences.gameAutoArm} disabled={quickPending || !preferences.gameDetectionEnabled} onChange={(event) => void updateQuickPreferences({ gameAutoArm: event.target.checked }, event.target.checked ? "Automatic Replay enabled." : "Automatic Replay disabled.")} /></label>
                  <label><span><strong>Stop when game closes</strong><small>Only for automatically started sessions</small></span><input type="checkbox" checked={preferences.gameStopReplayOnClose} disabled={quickPending || !preferences.gameDetectionEnabled} onChange={(event) => void updateQuickPreferences({ gameStopReplayOnClose: event.target.checked }, "Game-close behavior saved.")} /></label>
                </div>
                {detection?.candidates[0]?.title && <div className="home-quick-callout"><small>Game found</small><strong>{detection.candidates[0].title}</strong></div>}
                <button className="home-quick-text-action" type="button" onClick={() => openFromQuickPanel(onOpenSettings)}>Approval lists &amp; exclusions</button>
              </>
            )}
            {quickPanel === "specs" && (
              <>
                <div className="home-quick-heading"><div><small>Replay defaults</small><h2 id="home-quick-panel-title">Capture quality</h2></div></div>
                <div className="home-quick-form">
                  <label><span>Replay window</span><select value={preferences.replayDurationSeconds} disabled={quickPending || replay.state === "running"} onChange={(event) => void updateQuickPreferences({ replayDurationSeconds: Number(event.target.value) as UiPreferences["replayDurationSeconds"] }, "Replay window saved.")}><option value={30}>30 seconds</option><option value={60}>1 minute</option><option value={120}>2 minutes</option><option value={180}>3 minutes</option><option value={300}>5 minutes</option></select></label>
                  <label><span>Frame rate</span><select value={preferences.replayFrameRate} disabled={quickPending || replay.state === "running"} onChange={(event) => void updateQuickPreferences({ replayFrameRate: Number(event.target.value) as UiPreferences["replayFrameRate"] }, "Frame rate saved.")}><option value={30}>30 FPS</option><option value={60}>60 FPS</option></select></label>
                  <label><span>Video quality</span><select value={preferences.replayQuality} disabled={quickPending || replay.state === "running"} onChange={(event) => void updateQuickPreferences({ replayQuality: event.target.value as UiPreferences["replayQuality"] }, "Video quality saved.")}><option value="high">High</option><option value="balanced">Balanced</option><option value="smallerFiles">Smaller files</option></select></label>
                  <label><span>Encoder</span><select value={preferences.replayEncoder} disabled={quickPending || replay.state === "running"} onChange={(event) => void updateQuickPreferences({ replayEncoder: event.target.value as UiPreferences["replayEncoder"] }, "Encoder preference saved.")}><option value="automatic">Automatic</option><option value="hevc">HEVC</option><option value="h264">H.264</option></select></label>
                  <div><span>Resolution</span><strong>{resolution}</strong></div>
                </div>
                {replay.state === "running" && <p className="home-quick-copy">Stop Replay before changing capture defaults.</p>}
              </>
            )}
            {quickPanel === "hotkeys" && (
              <>
                <div className="home-quick-heading"><div><small>Keyboard shortcuts</small><h2 id="home-quick-panel-title">Save your moment</h2></div></div>
                <div className="home-quick-hotkey-editor">
                  <div><span><strong>Save Replay</strong><small>Save the last Replay window</small></span><kbd className={recordingHotkey === "saveReplay" ? "recording" : ""}>{recordingHotkey === "saveReplay" ? hotkeyDraft : hotkey?.currentCombination || "Not set"}</kbd><button type="button" disabled={quickPending || recordingHotkey === "saveAndName"} onClick={recordingHotkey === "saveReplay" ? () => void stopHotkeyRecording() : () => void startHotkeyRecording("saveReplay")}>{recordingHotkey === "saveReplay" ? "Cancel" : "Change"}</button></div>
                  <div><span><strong>Save &amp; Name</strong><small>Save once, then name it</small></span><kbd className={recordingHotkey === "saveAndName" ? "recording" : ""}>{recordingHotkey === "saveAndName" ? hotkeyDraft : saveAndNameHotkey?.currentCombination || "Disabled"}</kbd><button type="button" disabled={quickPending || recordingHotkey === "saveReplay"} onClick={recordingHotkey === "saveAndName" ? () => void stopHotkeyRecording() : () => void startHotkeyRecording("saveAndName")}>{recordingHotkey === "saveAndName" ? "Cancel" : saveAndNameHotkey?.registered ? "Change" : "Set"}</button>{saveAndNameHotkey?.registered && <button className="home-quick-disable" type="button" disabled={quickPending || Boolean(recordingHotkey)} onClick={() => void disableSaveAndNameHotkey()}>Disable</button>}</div>
                </div>
                {recordingHotkey && <p className="home-quick-copy">Press the new shortcut now. Escape cancels.</p>}
              </>
            )}
            {quickPanel === "settings" && (
              <>
                <div className="home-quick-heading"><div><small>Quick settings</small><h2 id="home-quick-panel-title">SlickClip behavior</h2></div></div>
                <div className="home-quick-switches">
                  <label><span><strong>Replay Saved overlay</strong><small>Show confirmation on the recorded display</small></span><input type="checkbox" checked={preferences.saveOverlayEnabled} disabled={quickPending} onChange={(event) => void updateQuickPreferences({ saveOverlayEnabled: event.target.checked }, "Overlay preference saved.")} /></label>
                  <label><span><strong>Close to tray</strong><small>Keep Replay available in the background</small></span><input type="checkbox" checked={preferences.closeToTray} disabled={quickPending} onChange={(event) => void updateQuickPreferences({ closeToTray: event.target.checked }, "Tray preference saved.")} /></label>
                  <label><span><strong>Start with Windows</strong><small>Launch SlickClip in the background</small></span><input type="checkbox" checked={preferences.startWithWindows} disabled={quickPending} onChange={(event) => void updateStartWithWindows(event.target.checked)} /></label>
                </div>
                <button className="home-quick-text-action" type="button" onClick={() => openFromQuickPanel(onOpenSettings)}>Open all settings</button>
              </>
            )}
            {quickMessage && <p className={`home-quick-message ${quickMessage.success ? "success" : "error"}`} role="status">{quickMessage.text}</p>}
          </aside>
        )}
      </div>

      {replay.state === "error" && replay.errorMessage && (
        <button className="home-capture-warning" type="button" onClick={onOpenReplay}>
          <strong>Replay needs attention</strong><span>{replay.errorMessage}</span><i aria-hidden="true">Review →</i>
        </button>
      )}

      <section className="home-clips-section" aria-labelledby="home-clips-heading">
        <div className="home-clips-toolbar">
          <div>
            <h2 id="home-clips-heading">All Clips <span>({totalCount})</span></h2>
            <small>Your newest local captures</small>
          </div>
          <div className="home-clips-controls">
            <label className="home-search-field">
              <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m20 20-4-4" /></svg>
              <span className="visually-hidden">Search recent clips</span>
              <input value={searchText} onChange={(event) => setSearchText(event.target.value)} placeholder="Filter clips" />
            </label>
            <label>
              <span className="visually-hidden">Sort recent clips</span>
              <select value={sortOrder} onChange={(event) => setSortOrder(event.target.value as ClipSortOrder)}>
                <option value="newestFirst">Newest</option>
                <option value="oldestFirst">Oldest</option>
                <option value="mostPlayed">Most Played</option>
                <option value="recentlyWatched">Recently Watched</option>
                <option value="longestFirst">Longest</option>
              </select>
            </label>
            <button className="home-view-all" type="button" onClick={onOpenClips}>View all</button>
          </div>
        </div>

        {error && <div className="home-library-state home-library-error" role="alert"><strong>Clips are unavailable</strong><span>{error}</span><button type="button" onClick={() => void loadClips()}>Retry</button></div>}
        {loading && !error && <div className="home-library-state"><strong>Loading clips…</strong><span>Reading your local Library.</span></div>}
        {!loading && !error && clips.length === 0 && <div className="home-library-state"><strong>No clips here yet</strong><span>Start Replay, play something worth remembering, then press {hotkey?.currentCombination || "your Save Replay hotkey"}.</span><button type="button" onClick={onOpenReplay}>Open Replay</button></div>}

        {!loading && !error && clips.length > 0 && (
          <div className="home-clip-grid">
            {clips.map((clip) => (
              <article className="home-clip-card" key={clip.id}>
                <div className="home-clip-thumbnail">
                  <ClipThumbnail clip={clip} onPlay={() => setPlayingClip(clip)} />
                  <span className="home-clip-duration">{formatDuration100ns(clip.duration100ns)}</span>
                  {clip.captureTargetLabel && <span className="home-clip-source" title={clip.captureTargetLabel}>{clip.captureTargetLabel}</span>}
                  <button className={`home-favorite${clip.favorite ? " active" : ""}`} type="button" aria-label={clip.favorite ? "Remove favorite" : "Add favorite"} title={clip.favorite ? "Remove favorite" : "Add favorite"} onClick={() => void setFavorite(clip)}>{clip.favorite ? "★" : "☆"}</button>
                </div>
                <div className="home-clip-meta">
                  <button className="home-clip-title" type="button" onClick={() => setPlayingClip(clip)}>{clip.displayName}</button>
                  <span className="home-clip-game">{clip.captureTargetLabel ?? "SlickClip Replay"}</span>
                  <div className="home-clip-footer">
                    <span>{new Date(clip.createdAtMs).toLocaleDateString(undefined, { month: "short", day: "numeric" })} · {formatFps(clip.fpsNumerator, clip.fpsDenominator)} FPS</span>
                    <div>
                      <button type="button" title="Edit clip" aria-label={`Edit ${clip.displayName}`} onClick={() => onEditClip(clip)}>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m4 4 16 16M14.5 14.5 20 9" /><circle cx="6" cy="17" r="3" /><circle cx="6" cy="7" r="3" /></svg>
                      </button>
                      <button type="button" title="Open in Clips" aria-label={`Open ${clip.displayName} in Clips`} onClick={onOpenClips}>
                        <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="5" cy="12" r="1" /><circle cx="12" cy="12" r="1" /><circle cx="19" cy="12" r="1" /></svg>
                      </button>
                    </div>
                  </div>
                </div>
              </article>
            ))}
          </div>
        )}
      </section>

      {playingClip && preferencesLoaded && (
        <ClipPlayer
          clip={playingClip}
          preferences={preferences}
          onPreferencesChange={persistPreferences}
          onClipUpdated={replaceClip}
          onCopy={() => void copyClip(playingClip)}
          onClose={() => setPlayingClip(null)}
        />
      )}
    </div>
  );
}
