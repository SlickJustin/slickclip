import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { AudioCaptureTest } from "../components/AudioCaptureTest";
import { Toggle } from "../components/Toggle";
import { combinationFromKeyboardEvent, isBareAlphanumericShortcut, isModifierCode, shortcutDraftFromKeyboardEvent } from "../lib/hotkeyShortcut";
import type { StorageCleanupExecutionResponse, StorageCleanupPreviewResponse, UiPreferences, UiPreferencesPatch, UiPreferencesResponse } from "../types/clips";
import { defaultUiPreferences, formatBytes } from "../types/clips";

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

type GameCandidate = {
  targetId: string;
  title: string;
  processName: string;
  processId: number;
  width: number;
  height: number;
  approved: boolean;
  reason: string;
};

type GameDetectionStatus = {
  success: boolean;
  enabled: boolean;
  autoArmEnabled: boolean;
  candidates: GameCandidate[];
  autoArmedTargetId: string | null;
  lastScanAtMs: number | null;
  errorMessage: string | null;
};

type UpdateConfiguration = {
  configured: boolean;
  currentVersion: string;
  message: string;
};

type UpdateCheck = {
  currentVersion: string;
  updateAvailable: boolean;
  version: string | null;
  notes: string | null;
  publishedAt: string | null;
};

const initialGameDetectionStatus: GameDetectionStatus = {
  success: true,
  enabled: false,
  autoArmEnabled: false,
  candidates: [],
  autoArmedTargetId: null,
  lastScanAtMs: null,
  errorMessage: null,
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
  const [hotkeyDraft, setHotkeyDraft] = useState("Press a shortcut…");
  const [hotkeyPending, setHotkeyPending] = useState(false);
  const [hotkeyMessage, setHotkeyMessage] = useState<{ text: string; success: boolean } | null>(null);
  const [preferences, setPreferences] = useState<UiPreferences>(defaultUiPreferences);
  const [desktopSettingsPending, setDesktopSettingsPending] = useState(false);
  const [desktopSettingsMessage, setDesktopSettingsMessage] = useState<{ text: string; success: boolean } | null>(null);
  const [gameDetection, setGameDetection] = useState<GameDetectionStatus>(initialGameDetectionStatus);
  const [storageQuotaInput, setStorageQuotaInput] = useState(String(defaultUiPreferences.storageQuotaGib));
  const [storagePreview, setStoragePreview] = useState<StorageCleanupPreviewResponse | null>(null);
  const [storagePending, setStoragePending] = useState(false);
  const [storageMessage, setStorageMessage] = useState<{ text: string; success: boolean } | null>(null);
  const [updateConfiguration, setUpdateConfiguration] = useState<UpdateConfiguration | null>(null);
  const [availableUpdate, setAvailableUpdate] = useState<UpdateCheck | null>(null);
  const [updatePending, setUpdatePending] = useState(false);
  const [updateMessage, setUpdateMessage] = useState<{ text: string; success: boolean } | null>(null);

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
      setStorageQuotaInput(String(preferenceResponse.preferences.storageQuotaGib));
    }).catch((error) => setHotkeyMessage({ text: error instanceof Error ? error.message : String(error), success: false }));
  }, []);

  useEffect(() => {
    void invoke<UpdateConfiguration>("get_update_configuration")
      .then(setUpdateConfiguration)
      .catch((error) => setUpdateMessage({ text: error instanceof Error ? error.message : String(error), success: false }));
  }, []);

  async function checkForUpdates() {
    if (updatePending || !updateConfiguration?.configured) return;
    setUpdatePending(true);
    setUpdateMessage(null);
    setAvailableUpdate(null);
    try {
      const response = await invoke<UpdateCheck>("check_for_slickclip_update");
      setAvailableUpdate(response);
      setUpdateMessage({
        text: response.updateAvailable && response.version
          ? `SlickClip ${response.version} is available.`
          : `SlickClip ${response.currentVersion} is up to date.`,
        success: true,
      });
    } catch (error) {
      setUpdateMessage({ text: error instanceof Error ? error.message : String(error), success: false });
    } finally {
      setUpdatePending(false);
    }
  }

  async function installUpdate() {
    if (updatePending || !availableUpdate?.updateAvailable || !availableUpdate.version) return;
    const version = availableUpdate.version;
    if (!window.confirm(`Download the signed SlickClip ${version} update, install it, and restart now?\n\nSave or finish any active work first.`)) return;
    setUpdatePending(true);
    setUpdateMessage({ text: `Downloading and verifying SlickClip ${version}. SlickClip will restart after installation.`, success: true });
    try {
      await invoke("install_slickclip_update", { expectedVersion: version });
    } catch (error) {
      setUpdateMessage({ text: error instanceof Error ? error.message : String(error), success: false });
      setUpdatePending(false);
    }
  }

  async function saveStorageQuota() {
    const parsed = Number(storageQuotaInput);
    if (!Number.isInteger(parsed) || parsed < 1 || parsed > 10_240) {
      throw new Error("Storage quota must be a whole number from 1 to 10,240 GB.");
    }
    const response = await invoke<UiPreferencesResponse>("update_ui_preferences", { patch: { storageQuotaGib: parsed } });
    setPreferences(response.preferences);
    setStorageQuotaInput(String(response.preferences.storageQuotaGib));
    if (!response.success) throw new Error(response.errorMessage ?? "The storage quota could not be saved.");
    return response.preferences.storageQuotaGib;
  }

  async function previewStorageCleanup() {
    if (storagePending) return;
    setStoragePending(true);
    setStorageMessage(null);
    setStoragePreview(null);
    try {
      const quotaGib = await saveStorageQuota();
      const response = await invoke<StorageCleanupPreviewResponse>("preview_storage_cleanup", { request: { quotaBytes: quotaGib * 1_073_741_824 } });
      if (!response.success) throw new Error(response.errorMessage ?? "Storage cleanup could not be previewed.");
      setStoragePreview(response);
    } catch (error) {
      setStorageMessage({ text: error instanceof Error ? error.message : String(error), success: false });
    } finally {
      setStoragePending(false);
    }
  }

  async function executeStorageCleanup() {
    if (storagePending || !storagePreview?.planId || storagePreview.candidates.length === 0) return;
    const confirmed = window.confirm(`Permanently delete ${storagePreview.candidates.length} unprotected clip${storagePreview.candidates.length === 1 ? "" : "s"} and reclaim about ${formatBytes(storagePreview.plannedReclaimBytes)}?\n\nOnly the clips listed in the preview will be deleted. This cannot be undone.`);
    if (!confirmed) return;
    setStoragePending(true);
    setStorageMessage(null);
    try {
      const response = await invoke<StorageCleanupExecutionResponse>("execute_storage_cleanup", { request: { planId: storagePreview.planId } });
      setStoragePreview(null);
      if (!response.success) throw new Error(response.errorMessage ?? "Storage cleanup did not complete.");
      setStorageMessage({ text: `Deleted ${response.deletedCount} clip${response.deletedCount === 1 ? "" : "s"}, reclaimed ${formatBytes(response.deletedBytes)}, and left ${formatBytes(response.remainingSizeBytes)} in the Library.`, success: true });
    } catch (error) {
      setStorageMessage({ text: error instanceof Error ? error.message : String(error), success: false });
    } finally {
      setStoragePending(false);
    }
  }

  useEffect(() => {
    if (!preferences.gameDetectionEnabled) {
      setGameDetection(initialGameDetectionStatus);
      return;
    }
    let disposed = false;
    async function refresh() {
      try {
        const status = await invoke<GameDetectionStatus>("get_game_detection_status");
        if (!disposed) setGameDetection(status);
      } catch (error) {
        if (!disposed) setGameDetection((current) => ({ ...current, success: false, errorMessage: error instanceof Error ? error.message : String(error) }));
      }
    }
    void refresh();
    const timer = window.setInterval(() => void refresh(), 2_000);
    return () => { disposed = true; window.clearInterval(timer); };
  }, [preferences.gameDetectionEnabled]);

  function processMatches(left: string, right: string) {
    return left.trim().replace(/\.exe$/i, "").toLocaleLowerCase() === right.trim().replace(/\.exe$/i, "").toLocaleLowerCase();
  }

  function approveProcess(processName: string) {
    const approved = [...preferences.gameDetectionApprovedProcesses.filter((item) => !processMatches(item, processName)), processName];
    const excluded = preferences.gameDetectionExcludedProcesses.filter((item) => !processMatches(item, processName));
    void updateDesktopPreference({ gameDetectionEnabled: true, gameDetectionApprovedProcesses: approved, gameDetectionExcludedProcesses: excluded });
  }

  function excludeProcess(processName: string) {
    const approved = preferences.gameDetectionApprovedProcesses.filter((item) => !processMatches(item, processName));
    const excluded = [...preferences.gameDetectionExcludedProcesses.filter((item) => !processMatches(item, processName)), processName];
    void updateDesktopPreference({ gameDetectionApprovedProcesses: approved, gameDetectionExcludedProcesses: excluded });
  }

  function removeProcessRule(processName: string, kind: "approved" | "excluded") {
    void updateDesktopPreference(kind === "approved"
      ? { gameDetectionApprovedProcesses: preferences.gameDetectionApprovedProcesses.filter((item) => !processMatches(item, processName)) }
      : { gameDetectionExcludedProcesses: preferences.gameDetectionExcludedProcesses.filter((item) => !processMatches(item, processName)) });
  }

  function addApprovedProcess() {
    const processName = window.prompt("Process executable to approve for game auto-arm (for example, GameName.exe):");
    if (processName?.trim()) approveProcess(processName.trim());
  }

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
        setHotkeyMessage({ text: "That key cannot be represented as a global shortcut. Try a standard, function, navigation, punctuation, or numpad key.", success: false });
        return;
      }
      setHotkeyDraft(combination);
      void submitHotkey(combination);
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

  useEffect(() => () => {
    if (recordingHotkey) void invoke("set_hotkey_recorder_active", { active: false });
  }, [recordingHotkey]);

  async function startHotkeyRecording() {
    setHotkeyMessage(null);
    try {
      const state = await invoke<HotkeyState>("set_hotkey_recorder_active", { active: true });
      setHotkey(state);
      setHotkeyDraft("Press a shortcut…");
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
    setHotkeyDraft("Press a shortcut…");
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
          ? isBareAlphanumericShortcut(result.state.currentCombination)
            ? `Save Replay hotkey changed to ${result.state.currentCombination}. Single-key shortcuts can trigger while typing in other applications.`
            : `Save Replay hotkey changed to ${result.state.currentCombination}.`
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
          <p>Configure SlickClip for your setup.</p>
        </div>
      </header>

      <div className="settings-grid">
        <SettingsCategory title="General" description="Windows startup and background behavior." defaultOpen>
          <SettingsToggle label="Start SlickClip with Windows" description="Launches quietly in the system tray after sign-in." checked={preferences.startWithWindows} disabled={desktopSettingsPending} onChange={(value) => void setStartWithWindows(value)} />
          <SettingsToggle label="Close or minimize to tray" description="Keeps active replay capture running in the background." checked={preferences.closeToTray} disabled={desktopSettingsPending} onChange={(value) => void updateDesktopPreference({ closeToTray: value })} />
          <SettingsToggle label="Show Replay Saved overlay" description="Shows a brief notification without taking focus." checked={preferences.saveOverlayEnabled} disabled={desktopSettingsPending} onChange={(value) => void updateDesktopPreference({ saveOverlayEnabled: value })} />
          {desktopSettingsMessage && <span className={desktopSettingsMessage.success ? "hotkey-message-success" : "hotkey-message-error"} role="status">{desktopSettingsMessage.text}</span>}
        </SettingsCategory>

        <SettingsCategory title="Capture" description="Defaults used when configuring replay capture.">
          <SettingSelect label="Default Capture Mode" value={captureMode} onChange={setCaptureMode} options={["Game", "Desktop", "Window"]} />
          <SettingSelect label="Default Clip Length" value={clipLength} onChange={setClipLength} options={["30 Seconds", "1 Minute", "2 Minutes", "3 Minutes", "5 Minutes"]} />
          <SettingSelect label="Resolution" value={resolution} onChange={setResolution} options={["720p", "1080p", "1440p"]} />
          <SettingSelect label="Frame Rate" value={frameRate} onChange={setFrameRate} options={["30 FPS", "60 FPS"]} />
          <SettingSelect label="Preferred Encoder" value={encoder} onChange={setEncoder} options={["NVIDIA NVENC AV1", "NVIDIA NVENC HEVC", "NVIDIA NVENC H.264", "Automatic"]} />
        </SettingsCategory>

        <SettingsCategory title="Hotkeys" description="Global controls that work while SlickClip is in the background.">
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
                {recordingHotkey ? hotkeyDraft : hotkey.currentCombination}
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
          {recordingHotkey && <small className="hotkey-recorder-help">Press the shortcut you want. Escape cancels. Bare letter and number keys are allowed but may trigger while typing.</small>}
          {(hotkeyMessage || hotkey.lastRegistrationError) && (
            <span className={hotkeyMessage?.success && !hotkey.lastRegistrationError ? "hotkey-message-success" : "hotkey-message-error"} role="status">
              {hotkeyMessage?.text ?? hotkey.lastRegistrationError}
            </span>
          )}
        </SettingsCategory>

        <SettingsCategory title="Storage" description="Clip location, quota, and safety-reviewed cleanup.">
          <div className="settings-row">
            <div><span>Save Location</span><small>Where completed clips will be stored</small></div>
            <div className="path-value">Videos\SlickClip\Clips</div>
          </div>
          <div className="settings-row storage-quota-row">
            <div><span>Library quota</span><small>Cleanup removes oldest unprotected clips first. Favorites are not protected automatically.</small></div>
            <label className="storage-quota-input"><span className="visually-hidden">Library quota in gigabytes</span><input type="number" min="1" max="10240" step="1" value={storageQuotaInput} onChange={(event) => { setStorageQuotaInput(event.target.value); setStoragePreview(null); }} /><span>GB</span></label>
          </div>
          <div className="storage-cleanup-actions">
            <button className="secondary-button" type="button" disabled={storagePending} onClick={() => void previewStorageCleanup()}>{storagePending ? "Checking…" : "Save Quota & Preview"}</button>
            <small>No files are deleted until you review a preview and confirm.</small>
          </div>
          {storagePreview && <div className="storage-cleanup-preview" role="status">
            <div className="storage-cleanup-summary"><strong>{storagePreview.bytesOverQuota === 0 ? "Library is within quota" : `${formatBytes(storagePreview.bytesOverQuota)} over quota`}</strong><span>{formatBytes(storagePreview.totalSizeBytes)} used · {formatBytes(storagePreview.protectedSizeBytes)} protected across {storagePreview.protectedCount} clip{storagePreview.protectedCount === 1 ? "" : "s"}</span></div>
            {storagePreview.candidates.length > 0 ? <>
              <p>{storagePreview.canMeetQuota ? `Deleting these ${storagePreview.candidates.length} oldest unprotected clips would leave about ${formatBytes(storagePreview.remainingSizeBytes)}.` : `All unprotected clips are listed, but protected clips keep the Library above quota. The remaining size would be about ${formatBytes(storagePreview.remainingSizeBytes)}.`}</p>
              <ol>{storagePreview.candidates.map((candidate) => <li key={candidate.clipId}><span>{candidate.displayName}</span><small>{new Date(candidate.createdAtMs).toLocaleString()} · {formatBytes(candidate.fileSizeBytes)}</small></li>)}</ol>
              <button className="danger" type="button" disabled={storagePending} onClick={() => void executeStorageCleanup()}>Delete Listed Clips…</button>
            </> : <p>No cleanup is needed. Protected clips are always excluded from automatic quota planning.</p>}
          </div>}
          {storageMessage && <span className={storageMessage.success ? "hotkey-message-success" : "hotkey-message-error"} role="status">{storageMessage.text}</span>}
        </SettingsCategory>

        <SettingsCategory title="Game Detection" description="Conservative process approval and auto-arm controls.">
          <SettingsToggle label="Detect likely games" description="Uses install, launcher, and window evidence to suggest games. Detection alone never starts capture." checked={preferences.gameDetectionEnabled} disabled={desktopSettingsPending} onChange={(value) => void updateDesktopPreference({ gameDetectionEnabled: value, gameAutoArm: value ? preferences.gameAutoArm : false })} />
          <SettingsToggle label="Auto-arm approved games" description="Starts only when exactly one explicitly approved game window is live; suggestions are never auto-armed." checked={preferences.gameAutoArm} disabled={desktopSettingsPending || !preferences.gameDetectionEnabled} onChange={(value) => void updateDesktopPreference({ gameAutoArm: value })} />
          <div className="game-detection-rules">
            <div className="game-detection-heading"><div><span>Process rules</span><small>Exclusions override approvals. Executable names are matched without case or .exe.</small></div><button className="secondary-button" type="button" disabled={desktopSettingsPending} onClick={addApprovedProcess}>+ Approve Process</button></div>
            <ProcessRuleList label="Approved for auto-arm" values={preferences.gameDetectionApprovedProcesses} onRemove={(value) => removeProcessRule(value, "approved")} />
            <ProcessRuleList label="Excluded" values={preferences.gameDetectionExcludedProcesses} onRemove={(value) => removeProcessRule(value, "excluded")} />
          </div>
          <div className="game-detection-live">
            <div className="game-detection-heading"><div><span>Live candidates</span><small>{preferences.gameDetectionEnabled ? "Review every process before allowing auto-arm." : "Enable detection to scan capturable windows."}</small></div>{gameDetection.autoArmedTargetId && <span className="game-auto-armed-status"><span className="status-dot status-dot-active" />Auto-armed</span>}</div>
            {gameDetection.errorMessage && <span className="hotkey-message-error" role="alert">{gameDetection.errorMessage}</span>}
            {preferences.gameDetectionEnabled && !gameDetection.errorMessage && gameDetection.candidates.length === 0 && <p className="game-detection-empty">No likely game windows are visible.</p>}
            {gameDetection.candidates.map((candidate) => <article className="game-candidate" key={candidate.targetId}>
              <div><strong>{candidate.processName}</strong><span>{candidate.title}</span><small>{candidate.width}×{candidate.height} · PID {candidate.processId} · {candidate.reason}</small></div>
              <div>{candidate.approved ? <span className="game-approved-badge">Approved</span> : <button className="secondary-button" type="button" disabled={desktopSettingsPending} onClick={() => approveProcess(candidate.processName)}>Approve</button>}<button className="secondary-button" type="button" disabled={desktopSettingsPending} onClick={() => excludeProcess(candidate.processName)}>Exclude</button></div>
            </article>)}
          </div>
        </SettingsCategory>

        <SettingsCategory title="Advanced" description="Diagnostics, hardware checks, and signed application updates.">
          <div className="advanced-settings-group">
            <div className="advanced-settings-heading"><h3>Audio Capture Test</h3><p>Inspect device and process-audio support without changing capture preferences.</p></div>
            <AudioCaptureTest />
          </div>
          <div className="advanced-settings-group">
            <div className="advanced-settings-heading"><h3>Updates</h3><p>Check the configured signed SlickClip release channel.</p></div>
            <div className="settings-row update-setting-row">
              <div><span>SlickClip {updateConfiguration?.currentVersion ?? "…"}</span><small>{updateConfiguration?.message ?? "Reading signed update configuration…"}</small></div>
              <button className="secondary-button" type="button" disabled={updatePending || !updateConfiguration?.configured} onClick={() => void checkForUpdates()}>{updatePending ? "Working…" : "Check for Updates"}</button>
            </div>
            {availableUpdate?.updateAvailable && availableUpdate.version && <div className="update-available-card" role="status">
              <div><strong>SlickClip {availableUpdate.version}</strong>{availableUpdate.publishedAt && <small>Published {new Date(availableUpdate.publishedAt).toLocaleString()}</small>}</div>
              {availableUpdate.notes && <p>{availableUpdate.notes}</p>}
              <button className="primary-button" type="button" disabled={updatePending} onClick={() => void installUpdate()}>Update & Restart</button>
            </div>}
            {updateMessage && <span className={updateMessage.success ? "hotkey-message-success" : "hotkey-message-error"} role="status">{updateMessage.text}</span>}
          </div>
        </SettingsCategory>
      </div>
    </div>
  );
}

function SettingsCategory({ title, description, children, defaultOpen = false }: { title: string; description: string; children: React.ReactNode; defaultOpen?: boolean }) {
  return (
    <details className="settings-category" open={defaultOpen || undefined}>
      <summary>
        <div><h2>{title}</h2><p>{description}</p></div>
        <span className="settings-category-chevron" aria-hidden="true">⌄</span>
      </summary>
      <div className="settings-section-body">{children}</div>
    </details>
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

function ProcessRuleList({ label, values, onRemove }: { label: string; values: string[]; onRemove: (value: string) => void }) {
  return <div className="process-rule-list"><small>{label}</small><div>{values.length === 0 ? <span>None</span> : values.map((value) => <button type="button" key={value} title={`Remove ${value}`} onClick={() => onRemove(value)}>{value}<span aria-hidden="true">×</span></button>)}</div></div>;
}
