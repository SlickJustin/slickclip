import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./App.css";
import { Sidebar, type PageId } from "./components/Sidebar";
import { TitleBar } from "./components/TitleBar";
import { featureVisibility } from "./config/features";
import { ClipsPage } from "./pages/ClipsPage";
import { EditorPage } from "./pages/EditorPage";
import { HelpPage } from "./pages/HelpPage";
import { HomePage } from "./pages/HomePage";
import { ReplayPage } from "./pages/ReplayPage";
import { ReplayRoulettePage } from "./pages/ReplayRoulettePage";
import { SettingsPage } from "./pages/SettingsPage";
import { WatchPartyPage } from "./pages/WatchPartyPage";
import type { ClipListItem } from "./types/clips";

function App() {
  const [activePage, setActivePage] = useState<PageId>("home");
  const [editorClip, setEditorClip] = useState<ClipListItem | null>(null);
  const [editorDirty, setEditorDirty] = useState(false);
  const [exportPlaybackClip, setExportPlaybackClip] = useState<ClipListItem | null>(null);
  const [toast, setToast] = useState<{ success: boolean; title: string; message: string } | null>(null);
  const [nameRequest, setNameRequest] = useState<{ clipId: string } | null>(null);
  const [clipName, setClipName] = useState("");
  const [clipNamePending, setClipNamePending] = useState(false);
  const [clipNameError, setClipNameError] = useState<string | null>(null);

  const showToast = useCallback((title: string, message: string, success: boolean) => {
    setToast({ title, message, success });
  }, []);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    void listen<{ clipId: string }>("save-replay-name-requested", (event) => {
      setNameRequest({ clipId: event.payload.clipId });
      setClipName("");
      setClipNameError(null);
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  async function saveClipName() {
    if (!nameRequest || clipNamePending || !clipName.trim()) return;
    setClipNamePending(true);
    setClipNameError(null);
    try {
      const result = await invoke<{ success: boolean; errorMessage: string | null }>("rename_clip_display_name", {
        request: { clipId: nameRequest.clipId, displayName: clipName.trim() },
      });
      if (!result.success) throw new Error(result.errorMessage ?? "The clip could not be renamed.");
      setNameRequest(null);
      setToast({ title: "Clip named", message: `Saved as “${clipName.trim()}”.`, success: true });
    } catch (error) {
      setClipNameError(error instanceof Error ? error.message : String(error));
    } finally {
      setClipNamePending(false);
    }
  }

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let unlistenAutoArm: UnlistenFn | undefined;
    let disposed = false;
    void listen<{ success: boolean; message: string }>("save-replay-hotkey-feedback", (event) => {
      setToast({ title: event.payload.success ? "Replay save" : "Could not save replay", message: event.payload.message, success: event.payload.success });
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    void listen<{ success: boolean; message: string }>("game-auto-arm-feedback", (event) => {
      setToast({ title: event.payload.success ? "Replay Buffer ready" : "Auto-arm needs attention", message: event.payload.message, success: event.payload.success });
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlistenAutoArm = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
      unlistenAutoArm?.();
    };
  }, []);

  useEffect(() => {
    if (!toast) return;
    const timer = window.setTimeout(() => setToast(null), 3_800);
    return () => window.clearTimeout(timer);
  }, [toast]);

  const navigate = useCallback((page: PageId) => {
    const leavingDirtyEditor = activePage === "editor" && page !== "editor" && editorDirty;
    const discardMessage = "Leave Editor?\n\nYour editable session is not saved. Any exported clips will remain in your Library.";
    if (leavingDirtyEditor && !window.confirm(discardMessage)) return;
    if (page !== "editor" || activePage !== "editor") {
      setEditorClip(null);
      setEditorDirty(false);
    }
    setActivePage(page);
  }, [activePage, editorDirty]);

  const openEditor = useCallback((clip: ClipListItem) => {
    setEditorClip(clip);
    setEditorDirty(false);
    setActivePage("editor");
  }, []);

  const closeEditor = useCallback(() => navigate("clips"), [navigate]);

  const playExport = useCallback((clip: ClipListItem) => {
    if (editorDirty && !window.confirm("Leave Editor?\n\nYour editable session is not saved. The exported clip will remain in your Library.")) return;
    setExportPlaybackClip(clip);
    setEditorClip(null);
    setEditorDirty(false);
    setActivePage("clips");
  }, [editorDirty]);

  const pages: Record<PageId, React.ReactNode> = {
    home: <HomePage onEditClip={openEditor} onOpenClips={() => navigate("clips")} onOpenReplay={() => navigate("replay")} onOpenSettings={() => navigate("settings")} onToast={showToast} />,
    replay: <ReplayPage />,
    watchParty: featureVisibility.watchParty ? <WatchPartyPage /> : <ReplayPage />,
    clips: <ClipsPage onEditClip={openEditor} playClip={exportPlaybackClip} onPlayClipConsumed={() => setExportPlaybackClip(null)} onToast={showToast} />,
    roulette: <ReplayRoulettePage onToast={showToast} />,
    editor: <EditorPage clip={editorClip} onBackToClips={closeEditor} onPlayExport={playExport} onDirtyChange={setEditorDirty} onToast={showToast} />,
    help: <HelpPage />,
    settings: <SettingsPage />,
  };

  return (
    <div className="app-shell">
      <TitleBar />
      <div className="app-body">
        <Sidebar activePage={activePage} onNavigate={navigate} />
        <main className="app-content">{pages[activePage]}</main>
      </div>
      {toast && (
        <div className={`hotkey-feedback ${toast.success ? "hotkey-feedback-success" : "hotkey-feedback-error"}`} role="status" aria-live="polite">
          <strong>{toast.title}</strong><span>{toast.message}</span>
        </div>
      )}
      {nameRequest && (
        <div className="save-name-backdrop" role="presentation">
          <section className="save-name-dialog" role="dialog" aria-modal="true" aria-labelledby="save-name-title">
            <div>
              <span className="save-name-kicker">Replay saved</span>
              <h2 id="save-name-title">Name this clip</h2>
              <p>The clip is already safely stored in your Library. Give it a memorable name, or keep the automatic name.</p>
            </div>
            <input
              autoFocus
              maxLength={120}
              value={clipName}
              placeholder="Example: Last-second comeback"
              aria-label="Clip name"
              onChange={(event) => setClipName(event.target.value)}
              onKeyDown={(event) => {
                if (event.key === "Enter") void saveClipName();
                if (event.key === "Escape" && !clipNamePending) setNameRequest(null);
              }}
            />
            {clipNameError && <span className="save-name-error" role="alert">{clipNameError}</span>}
            <div className="save-name-actions">
              <button className="secondary-button" type="button" disabled={clipNamePending} onClick={() => setNameRequest(null)}>Keep Default</button>
              <button className="primary-button" type="button" disabled={clipNamePending || !clipName.trim()} onClick={() => void saveClipName()}>{clipNamePending ? "Saving…" : "Save Name"}</button>
            </div>
          </section>
        </div>
      )}
    </div>
  );
}

export default App;
