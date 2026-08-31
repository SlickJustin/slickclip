import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { AudioCaptureTest } from "../components/AudioCaptureTest";
import { InfoTip } from "../components/InfoTip";
import { Toggle } from "../components/Toggle";
import { combinationFromKeyboardEvent, isBareAlphanumericShortcut, isModifierCode, shortcutDraftFromKeyboardEvent } from "../lib/hotkeyShortcut";
import type { StorageCleanupExecutionResponse, StorageCleanupPreviewResponse, UiPreferences, UiPreferencesPatch, UiPreferencesResponse } from "../types/clips";
import { defaultUiPreferences, formatBytes } from "../types/clips";
import { detectedReplayLabel, showCandidateApprovalControls, type DetectedReplayState } from "../utils/gameDetectionStatus";

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
  foreground: boolean;
  approved: boolean;
  reason: string;
};

type GameDetectionStatus = {
  success: boolean;
  enabled: boolean;
  autoArmEnabled: boolean;
  detectionMode: "anyDetectedGame" | "approvedGamesOnly";
  stopReplayOnClose: boolean;
  readyNotificationEnabled: boolean;
  candidates: GameCandidate[];
  autoArmedTargetId: string | null;
  replayReady: boolean;
  replayState: DetectedReplayState;
  pendingTargetId: string | null;
  manualOverrideActive: boolean;
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
  detectionMode: "anyDetectedGame",
  stopReplayOnClose: true,
  readyNotificationEnabled: true,
  candidates: [],
  autoArmedTargetId: null,
  replayReady: false,
  replayState: "replayStopped",
  pendingTargetId: null,
  manualOverrideActive: false,
  lastScanAtMs: null,
  errorMessage: null,
};

const initialHotkeyState: HotkeyState = {
  registered: false,
  currentCombination: "Ctrl + Shift + F10",
  lastRegistrationError: null,
  testing: false,
};

const initialOptionalHotkeyState: HotkeyState = {
  registered: false,
  currentCombination: "",
  lastRegistrationError: null,
  testing: false,
};

type RecordingHotkey = "saveReplay" | "saveAndName";

type SettingSelectProps = {
  label: string;
  value: string;
  options: string[];
  onChange: (value: string) => void;
};

export function SettingsPage() {
  const [hotkey, setHotkey] = useState<HotkeyState>(initialHotkeyState);
  const [saveAndNameHotkey, setSaveAndNameHotkey] = useState<HotkeyState>(initialOptionalHotkeyState);
  const [recordingHotkey, setRecordingHotkey] = useState<RecordingHotkey | null>(null);
  const [hotkeyDraft, setHotkeyDraft] = useState("Press a shortcut…");
  const [hotkeyPending, setHotkeyPending] = useState(false);
  const [hotkeyMessage, setHotkeyMessage] = useState<{ text: string; success: boolean } | null>(null);
  const [saveAndNameHotkeyMessage, setSaveAndNameHotkeyMessage] = useState<{ text: string; success: boolean } | null>(null);
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
      invoke<HotkeyState>("get_save_and_name_hotkey"),
      invoke<UiPreferencesResponse>("get_ui_preferences"),
    ]).then(([hotkeyState, saveAndNameState, preferenceResponse]) => {
      setHotkey(hotkeyState);
      setSaveAndNameHotkey(saveAndNameState);
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
    const confirmed = window.confirm(`Permanently delete ${storagePreview.candidates.length} clip${storagePreview.candidates.length === 1 ? "" : "s"} not protected from cleanup and reclaim about ${formatBytes(storagePreview.plannedReclaimBytes)}?\n\nOnly the clips listed in the preview will be deleted. This cannot be undone.`);
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

  function addExcludedProcess() {
    const processName = window.prompt("Application executable to exclude from automatic game capture (for example, Launcher.exe):");
    if (processName?.trim()) excludeProcess(processName.trim());
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

  useEffect(() => () => {
    if (recordingHotkey) {
      const command = recordingHotkey === "saveReplay"
        ? "set_hotkey_recorder_active"
        : "set_save_and_name_hotkey_recorder_active";
      void invoke(command, { active: false });
    }
  }, [recordingHotkey]);

  async function startHotkeyRecording(target: RecordingHotkey) {
    if (target === "saveReplay") setHotkeyMessage(null);
    else setSaveAndNameHotkeyMessage(null);
    try {
      const command = target === "saveReplay"
        ? "set_hotkey_recorder_active"
        : "set_save_and_name_hotkey_recorder_active";
      const state = await invoke<HotkeyState>(command, { active: true });
      if (target === "saveReplay") setHotkey(state);
      else setSaveAndNameHotkey(state);
      setHotkeyDraft("Press a shortcut…");
      setRecordingHotkey(target);
    } catch (error) {
      const message = { text: error instanceof Error ? error.message : String(error), success: false };
      if (target === "saveReplay") setHotkeyMessage(message);
      else setSaveAndNameHotkeyMessage(message);
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
    const target = recordingHotkey;
    if (!target) return;
    setRecordingHotkey(null);
    setHotkeyDraft("Press a shortcut…");
    try {
      const command = target === "saveReplay"
        ? "set_hotkey_recorder_active"
        : "set_save_and_name_hotkey_recorder_active";
      const state = await invoke<HotkeyState>(command, { active: false });
      if (target === "saveReplay") setHotkey(state);
      else setSaveAndNameHotkey(state);
    } catch (error) {
      const message = { text: error instanceof Error ? error.message : String(error), success: false };
      if (target === "saveReplay") setHotkeyMessage(message);
      else setSaveAndNameHotkeyMessage(message);
    }
  }

  async function submitHotkey(target: RecordingHotkey, combination: string) {
    if (hotkeyPending) return;
    setHotkeyPending(true);
    if (target === "saveReplay") setHotkeyMessage(null);
    else setSaveAndNameHotkeyMessage(null);
    try {
      const command = target === "saveReplay" ? "set_save_replay_hotkey" : "set_save_and_name_hotkey";
      const result = await invoke<HotkeyCommandResult>(command, { combination });
      if (target === "saveReplay") setHotkey(result.state);
      else setSaveAndNameHotkey(result.state);
      setRecordingHotkey(null);
      const message = {
        text: result.success
          ? isBareAlphanumericShortcut(result.state.currentCombination)
            ? `${target === "saveReplay" ? "Save Replay" : "Save & Name"} hotkey changed to ${result.state.currentCombination}. Single-key shortcuts can trigger while typing in other applications.`
            : `${target === "saveReplay" ? "Save Replay" : "Save & Name"} hotkey changed to ${result.state.currentCombination}.`
          : result.errorMessage ?? "The global hotkey could not be registered.",
        success: result.success,
      };
      if (target === "saveReplay") setHotkeyMessage(message);
      else setSaveAndNameHotkeyMessage(message);
    } catch (error) {
      setRecordingHotkey(null);
      const message = { text: error instanceof Error ? error.message : String(error), success: false };
      if (target === "saveReplay") setHotkeyMessage(message);
      else setSaveAndNameHotkeyMessage(message);
      const recorderCommand = target === "saveReplay"
        ? "set_hotkey_recorder_active"
        : "set_save_and_name_hotkey_recorder_active";
      await invoke<HotkeyState>(recorderCommand, { active: false })
        .then((state) => target === "saveReplay" ? setHotkey(state) : setSaveAndNameHotkey(state))
        .catch(() => undefined);
    } finally {
      setHotkeyPending(false);
    }
  }

  async function disableSaveAndNameHotkey() {
    if (hotkeyPending) return;
    setHotkeyPending(true);
    setSaveAndNameHotkeyMessage(null);
    try {
      const result = await invoke<HotkeyCommandResult>("clear_save_and_name_hotkey");
      setSaveAndNameHotkey(result.state);
      setSaveAndNameHotkeyMessage({
        text: result.success ? "Save & Name hotkey disabled." : result.errorMessage ?? "The hotkey could not be disabled.",
        success: result.success,
      });
    } catch (error) {
      setSaveAndNameHotkeyMessage({ text: error instanceof Error ? error.message : String(error), success: false });
    } finally {
      setHotkeyPending(false);
    }
  }

  return (
    <div className="page settings-page">
      <header className="page-header settings-page-header">
        <div>
          <span className="settings-page-eyebrow">SlickClip control center</span>
          <h1>Settings</h1>
          <p>Set your capture defaults once. SlickClip remembers the rest.</p>
        </div>
        <div className="settings-header-status" aria-label="Settings status">
          <span><i className="settings-live-dot" />Saved automatically</span>
          <span>Local to this PC</span>
        </div>
      </header>

      <div className="settings-workbench">
        <aside className="settings-index" aria-label="Settings sections">
          <div className="settings-index-heading">
            <span>Quick access</span>
            <strong>Your setup</strong>
            <small>Jump straight to the setting you need.</small>
          </div>
          <nav>
            <SettingsIndexItem number="01" title="General" detail="Startup & background" targetId="settings-general" />
            <SettingsIndexItem number="02" title="Capture" detail={`${replayDurationLabel(preferences.replayDurationSeconds)} · ${preferences.replayFrameRate} FPS`} targetId="settings-capture" />
            <SettingsIndexItem number="03" title="Hotkeys" detail={saveAndNameHotkey.registered ? "2 shortcuts ready" : "1 shortcut ready"} targetId="settings-hotkeys" />
            <SettingsIndexItem number="04" title="Storage" detail={`${storageQuotaInput || preferences.storageQuotaGib} GB quota`} targetId="settings-storage" />
            <SettingsIndexItem number="05" title="Game Detection" detail={preferences.gameDetectionEnabled ? (preferences.gameAutoArm ? "Automatic capture on" : "Detection only") : "Off"} targetId="settings-game-detection" />
            <SettingsIndexItem number="06" title="Advanced" detail="Tests & updates" targetId="settings-advanced" />
          </nav>
          <div className="settings-index-note">
            <span>Standalone by design</span>
            <small>FFmpeg ships inside SlickClip. Your friends install nothing else.</small>
          </div>
        </aside>

        <main className="settings-grid">
        <SettingsCategory id="settings-general" number="01" title="General" description="Windows startup and background behavior." status="3 essentials" defaultOpen>
          <SettingsToggle label="Start SlickClip with Windows" description="Launches quietly in the system tray after sign-in." checked={preferences.startWithWindows} disabled={desktopSettingsPending} onChange={(value) => void setStartWithWindows(value)} />
          <SettingsToggle label="Close or minimize to tray" description="Keeps active replay capture running in the background." checked={preferences.closeToTray} disabled={desktopSettingsPending} onChange={(value) => void updateDesktopPreference({ closeToTray: value })} />
          <SettingsToggle label="Show Replay Saved overlay" description="Shows a brief notification without taking focus." checked={preferences.saveOverlayEnabled} disabled={desktopSettingsPending} onChange={(value) => void updateDesktopPreference({ saveOverlayEnabled: value })} />
          {desktopSettingsMessage && <span className={desktopSettingsMessage.success ? "hotkey-message-success" : "hotkey-message-error"} role="status">{desktopSettingsMessage.text}</span>}
        </SettingsCategory>

        <SettingsCategory id="settings-capture" number="02" title="Capture" description="Defaults used when configuring replay capture." status={`${replayDurationLabel(preferences.replayDurationSeconds)} · ${preferences.replayFrameRate} FPS`}>
          <div className="settings-row">
            <div>
              <span className="concept-heading">Video capture <InfoTip label="About video capture">SlickClip uses its bundled FFmpeg engine to capture the physical display containing the detected game and keeps that same display for the whole Replay session, including while you Alt-Tab. No separate FFmpeg installation is needed.</InfoTip></span>
              <small>Bundled FFmpeg display capture</small>
            </div>
            <strong>Display Capture</strong>
          </div>
          <SettingSelect label="Default Clip Length" value={replayDurationLabel(preferences.replayDurationSeconds)} onChange={(value) => void updateDesktopPreference({ replayDurationSeconds: replayDurationFromLabel(value) })} options={["30 Seconds", "1 Minute", "2 Minutes", "3 Minutes", "5 Minutes"]} />
          <div className="settings-row"><div><span>Output Resolution</span><small>Replay records the selected physical display at its native dimensions.</small></div><strong>Display native</strong></div>
          <SettingSelect label="Frame Rate" value={`${preferences.replayFrameRate} FPS`} onChange={(value) => void updateDesktopPreference({ replayFrameRate: value.startsWith("30") ? 30 : 60 })} options={["30 FPS", "60 FPS"]} />
          <SettingSelect label="Video Quality" value={preferences.replayQuality === "smallerFiles" ? "Smaller Files" : preferences.replayQuality === "high" ? "High" : "Balanced"} onChange={(value) => void updateDesktopPreference({ replayQuality: value === "High" ? "high" : value === "Smaller Files" ? "smallerFiles" : "balanced" })} options={["High", "Balanced", "Smaller Files"]} />
          <SettingSelect label="Preferred Encoder" value={preferences.replayEncoder === "hevc" ? "HEVC" : preferences.replayEncoder === "h264" ? "H.264" : "Automatic"} onChange={(value) => void updateDesktopPreference({ replayEncoder: value === "HEVC" ? "hevc" : value === "H.264" ? "h264" : "automatic" })} options={["Automatic", "HEVC", "H.264"]} />
        </SettingsCategory>

        <SettingsCategory id="settings-hotkeys" number="03" title="Hotkeys" description="Global controls that work while SlickClip is in the background." status={saveAndNameHotkey.registered ? "2 ready" : "1 ready"}>
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
              <kbd className={recordingHotkey === "saveReplay" ? "hotkey-recording" : undefined}>
                {recordingHotkey === "saveReplay" ? hotkeyDraft : hotkey.currentCombination}
              </kbd>
              <button
                className="secondary-button"
                type="button"
                disabled={hotkeyPending || recordingHotkey === "saveAndName"}
                onClick={recordingHotkey === "saveReplay" ? stopHotkeyRecording : () => void startHotkeyRecording("saveReplay")}
              >
                {recordingHotkey === "saveReplay" ? "Cancel" : hotkeyPending ? "Registering..." : "Change"}
              </button>
              <button className="secondary-button" type="button" disabled={!hotkey.registered || Boolean(recordingHotkey) || hotkeyPending || hotkey.testing} onClick={() => void testHotkey()}>{hotkey.testing ? "Listening…" : "Test Hotkey"}</button>
            </div>
          </div>
          {recordingHotkey === "saveReplay" && <small className="hotkey-recorder-help">Press the shortcut you want. Escape cancels. Bare letter and number keys are allowed but may trigger while typing.</small>}
          {(hotkeyMessage || hotkey.lastRegistrationError) && (
            <span className={hotkeyMessage?.success && !hotkey.lastRegistrationError ? "hotkey-message-success" : "hotkey-message-error"} role="status">
              {hotkeyMessage?.text ?? hotkey.lastRegistrationError}
            </span>
          )}
          <div className="hotkey-setting">
            <div className="hotkey-setting-copy">
              <span>Save &amp; Name Hotkey</span>
              <small>Optional. Saves normally, then brings SlickClip forward to name the indexed clip.</small>
              <div className="hotkey-registration-status">
                <span className={`hotkey-status-dot ${saveAndNameHotkey.registered ? "hotkey-status-registered" : ""}`} />
                {saveAndNameHotkey.registered ? "Registered" : "Not configured"}
              </div>
            </div>
            <div className="hotkey-setting-controls">
              <kbd className={recordingHotkey === "saveAndName" ? "hotkey-recording" : undefined}>
                {recordingHotkey === "saveAndName" ? hotkeyDraft : saveAndNameHotkey.currentCombination || "Not configured"}
              </kbd>
              <button
                className="secondary-button"
                type="button"
                disabled={hotkeyPending || recordingHotkey === "saveReplay"}
                onClick={recordingHotkey === "saveAndName" ? stopHotkeyRecording : () => void startHotkeyRecording("saveAndName")}
              >
                {recordingHotkey === "saveAndName" ? "Cancel" : hotkeyPending ? "Registering..." : saveAndNameHotkey.registered ? "Change" : "Set Hotkey"}
              </button>
              {saveAndNameHotkey.registered && <button className="secondary-button" type="button" disabled={hotkeyPending || Boolean(recordingHotkey)} onClick={() => void disableSaveAndNameHotkey()}>Disable</button>}
            </div>
          </div>
          {recordingHotkey === "saveAndName" && <small className="hotkey-recorder-help">Press the shortcut you want. Escape cancels. This action intentionally brings SlickClip forward after the clip is safely indexed.</small>}
          {(saveAndNameHotkeyMessage || saveAndNameHotkey.lastRegistrationError) && (
            <span className={saveAndNameHotkeyMessage?.success && !saveAndNameHotkey.lastRegistrationError ? "hotkey-message-success" : "hotkey-message-error"} role="status">
              {saveAndNameHotkeyMessage?.text ?? saveAndNameHotkey.lastRegistrationError}
            </span>
          )}
        </SettingsCategory>

        <SettingsCategory id="settings-storage" number="04" title="Storage" description="Clip location, quota, and safety-reviewed cleanup." status={`${storageQuotaInput || preferences.storageQuotaGib} GB`}>
          <div className="settings-row">
            <div><span>Save Location</span><small>Where completed clips will be stored</small></div>
            <div className="path-value">Videos\SlickClip\Clips</div>
          </div>
          <div className="settings-row storage-quota-row">
            <div><span className="concept-heading">Library quota <InfoTip label="About Library quota">Sets how much storage SlickClip may use before removing the oldest clips that are not protected from cleanup.</InfoTip></span><small>Automatic cleanup removes the oldest clips not protected from cleanup first. Favorites do not receive cleanup protection automatically.</small></div>
            <label className="storage-quota-input"><span className="visually-hidden">Library quota in gigabytes</span><input type="number" min="1" max="10240" step="1" value={storageQuotaInput} onChange={(event) => { setStorageQuotaInput(event.target.value); setStoragePreview(null); }} /><span>GB</span></label>
          </div>
          <div className="storage-cleanup-actions">
            <button className="secondary-button" type="button" disabled={storagePending} onClick={() => void previewStorageCleanup()}>{storagePending ? "Checking…" : "Save Quota & Preview"}</button>
            <small>No files are deleted until you review a preview and confirm.</small>
          </div>
          {storagePreview && <div className="storage-cleanup-preview" role="status">
            <div className="storage-cleanup-summary"><strong>{storagePreview.bytesOverQuota === 0 ? "Library is within quota" : `${formatBytes(storagePreview.bytesOverQuota)} over quota`}</strong><span>{formatBytes(storagePreview.totalSizeBytes)} used · {formatBytes(storagePreview.protectedSizeBytes)} protected from cleanup across {storagePreview.protectedCount} clip{storagePreview.protectedCount === 1 ? "" : "s"}</span></div>
            {storagePreview.candidates.length > 0 ? <>
              <p>{storagePreview.canMeetQuota ? `Deleting these ${storagePreview.candidates.length} oldest clips not protected from cleanup would leave about ${formatBytes(storagePreview.remainingSizeBytes)}.` : `All clips not protected from cleanup are listed, but cleanup-protected clips keep the Library above quota. The remaining size would be about ${formatBytes(storagePreview.remainingSizeBytes)}.`}</p>
              <ol>{storagePreview.candidates.map((candidate) => <li key={candidate.clipId}><span>{candidate.displayName}</span><small>{new Date(candidate.createdAtMs).toLocaleString()} · {formatBytes(candidate.fileSizeBytes)}</small></li>)}</ol>
              <button className="danger" type="button" disabled={storagePending} onClick={() => void executeStorageCleanup()}>Delete Listed Clips…</button>
            </> : <p>No cleanup is needed. Clips protected from cleanup are always excluded from automatic quota planning, but can still be deleted manually.</p>}
          </div>}
          {storageMessage && <span className={storageMessage.success ? "hotkey-message-success" : "hotkey-message-error"} role="status">{storageMessage.text}</span>}
        </SettingsCategory>

        <SettingsCategory id="settings-game-detection" number="05" title="Game Detection" description="Automatically get Replay ready when a game opens." status={preferences.gameDetectionEnabled ? (preferences.gameAutoArm ? "Auto capture" : "Observing") : "Off"}>
          <SettingsToggle label="Game Detection" description="Watches capturable windows for strong game signals while SlickClip runs in the tray." checked={preferences.gameDetectionEnabled} disabled={desktopSettingsPending} onChange={(value) => void updateDesktopPreference({ gameDetectionEnabled: value, gameAutoArm: value ? preferences.gameAutoArm : false })} />
          <SettingsToggle label="Automatically start Replay for detected games" help="A game must remain the best matching target for multiple scans before Replay starts." description="Turn this off to observe detected games without ever starting Replay automatically." checked={preferences.gameAutoArm} disabled={desktopSettingsPending || !preferences.gameDetectionEnabled} onChange={(value) => void updateDesktopPreference({ gameAutoArm: value })} />
          <label className="setting-row">
            <span><span className="setting-label">Detection Mode</span><small>Use approvals only when you want a strict allowlist.</small></span>
            <select value={preferences.gameDetectionMode} disabled={desktopSettingsPending || !preferences.gameDetectionEnabled} onChange={(event) => void updateDesktopPreference({ gameDetectionMode: event.target.value as UiPreferences["gameDetectionMode"] })}>
              <option value="anyDetectedGame">Any detected game</option>
              <option value="approvedGamesOnly">Approved games only</option>
            </select>
          </label>
          <SettingsToggle label="Stop Replay when the game closes" description="Stops the automatically started buffer safely. Closing a game never saves a clip." checked={preferences.gameStopReplayOnClose} disabled={desktopSettingsPending || !preferences.gameDetectionEnabled} onChange={(value) => void updateDesktopPreference({ gameStopReplayOnClose: value })} />
          <SettingsToggle label="Show Replay Ready notification" description="Shows one small non-focus-stealing notice when automatic capture becomes ready." checked={preferences.gameReadyNotificationEnabled} disabled={desktopSettingsPending || !preferences.gameDetectionEnabled} onChange={(value) => void updateDesktopPreference({ gameReadyNotificationEnabled: value })} />
          <div className="game-detection-rules">
            <div className="game-detection-heading"><div><span>Excluded applications</span><small>Exclusions always win and can never become automatic targets.</small></div><button className="secondary-button" type="button" disabled={desktopSettingsPending} onClick={addExcludedProcess}>+ Exclude App</button></div>
            <ProcessRuleList label="Excluded" values={preferences.gameDetectionExcludedProcesses} onRemove={(value) => removeProcessRule(value, "excluded")} />
            <details className="game-detection-advanced">
              <summary>Advanced approved-game controls</summary>
              <div className="game-detection-heading"><div><span>Approved applications</span><small>Used as an allowlist only in Approved games only mode. Existing approvals are preserved.</small></div><button className="secondary-button" type="button" disabled={desktopSettingsPending} onClick={addApprovedProcess}>+ Approve Process</button></div>
              <ProcessRuleList label="Approved" values={preferences.gameDetectionApprovedProcesses} onRemove={(value) => removeProcessRule(value, "approved")} />
            </details>
          </div>
          <div className="game-detection-live">
            <div className="game-detection-heading"><div><span>Detected games</span><small>{preferences.gameDetectionEnabled ? (preferences.gameAutoArm ? "The best stable match starts automatically unless excluded." : "Detection is observing only; automatic Replay start is off.") : "Enable detection to scan capturable windows."}</small></div>{preferences.gameDetectionEnabled && (gameDetection.autoArmedTargetId || gameDetection.candidates.length > 0) && <span className={`game-auto-armed-status game-auto-armed-status-${gameDetection.replayState}`}><span className="status-dot status-dot-active" />{detectedReplayLabel(gameDetection.replayState)}</span>}</div>
            {gameDetection.manualOverrideActive && <p className="game-detection-empty">Manual capture override is active. Game Detection will not replace it.</p>}
            {gameDetection.errorMessage && <span className="hotkey-message-error" role="alert">{gameDetection.errorMessage}</span>}
            {preferences.gameDetectionEnabled && !gameDetection.errorMessage && gameDetection.candidates.length === 0 && <p className="game-detection-empty">No likely game windows are visible.</p>}
            {gameDetection.candidates.map((candidate) => <article className="game-candidate" key={candidate.targetId}>
              <div><strong>{candidate.title}</strong><span>{candidate.processName}</span><small>{candidate.width}×{candidate.height} · PID {candidate.processId}{candidate.foreground ? " · Foreground" : ""} · {candidate.reason}</small></div>
              <div>{showCandidateApprovalControls(preferences.gameDetectionMode) && (candidate.approved ? <span className="game-approved-badge">Approved</span> : <button className="secondary-button" type="button" disabled={desktopSettingsPending} onClick={() => approveProcess(candidate.processName)}>Approve</button>)}<button className="secondary-button" type="button" disabled={desktopSettingsPending} onClick={() => excludeProcess(candidate.processName)}>Exclude</button></div>
            </article>)}
          </div>
        </SettingsCategory>

        <SettingsCategory id="settings-advanced" number="06" title="Advanced" description="Diagnostics, hardware checks, and signed application updates." status="On demand">
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
        </main>
      </div>
    </div>
  );
}

function replayDurationLabel(seconds: UiPreferences["replayDurationSeconds"]) {
  return ({ 30: "30 Seconds", 60: "1 Minute", 120: "2 Minutes", 180: "3 Minutes", 300: "5 Minutes" } as const)[seconds];
}

function replayDurationFromLabel(label: string): UiPreferences["replayDurationSeconds"] {
  return ({ "30 Seconds": 30, "1 Minute": 60, "2 Minutes": 120, "3 Minutes": 180, "5 Minutes": 300 } as const)[label as "30 Seconds" | "1 Minute" | "2 Minutes" | "3 Minutes" | "5 Minutes"] ?? 120;
}

function openSettingsCategory(targetId: string) {
  const category = document.getElementById(targetId);
  if (!(category instanceof HTMLDetailsElement)) return;
  category.open = true;
  category.scrollIntoView({ behavior: "smooth", block: "start" });
  window.setTimeout(() => category.querySelector<HTMLElement>("summary")?.focus({ preventScroll: true }), 250);
}

function SettingsIndexItem({ number, title, detail, targetId }: { number: string; title: string; detail: string; targetId: string }) {
  return (
    <button type="button" onClick={() => openSettingsCategory(targetId)}>
      <span>{number}</span>
      <strong>{title}</strong>
      <small>{detail}</small>
      <b aria-hidden="true">→</b>
    </button>
  );
}

function SettingsCategory({ id, number, title, description, status, children, defaultOpen = false }: { id: string; number: string; title: string; description: string; status: string; children: React.ReactNode; defaultOpen?: boolean }) {
  return (
    <details className="settings-category" id={id} open={defaultOpen || undefined}>
      <summary>
        <div className="settings-category-heading"><span>{number}</span><div><h2>{title}</h2><p>{description}</p></div></div>
        <div className="settings-category-meta"><span>{status}</span><i className="settings-category-chevron" aria-hidden="true">⌄</i></div>
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
  help?: string;
  description?: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  disabled?: boolean;
};

function SettingsToggle({ label, help, description, checked, onChange, disabled = false }: SettingsToggleProps) {
  return (
    <div className="settings-row">
      <div><span className={help ? "concept-heading" : undefined}>{label}{help && <InfoTip label={`About ${label}`}>{help}</InfoTip>}</span>{description && <small>{description}</small>}</div>
      <Toggle label={label} checked={checked} onChange={onChange} disabled={disabled} />
    </div>
  );
}

function ProcessRuleList({ label, values, onRemove }: { label: string; values: string[]; onRemove: (value: string) => void }) {
  return <div className="process-rule-list"><small>{label}</small><div>{values.length === 0 ? <span>None</span> : values.map((value) => <button type="button" key={value} title={`Remove ${value}`} onClick={() => onRemove(value)}>{value}<span aria-hidden="true">×</span></button>)}</div></div>;
}
