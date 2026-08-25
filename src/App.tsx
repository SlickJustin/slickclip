import { useCallback, useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./App.css";
import { Sidebar, type PageId } from "./components/Sidebar";
import { TitleBar } from "./components/TitleBar";
import { featureVisibility } from "./config/features";
import { ClipsPage } from "./pages/ClipsPage";
import { EditorPage } from "./pages/EditorPage";
import { ReplayPage } from "./pages/ReplayPage";
import { ReplayRoulettePage } from "./pages/ReplayRoulettePage";
import { SettingsPage } from "./pages/SettingsPage";
import { WatchPartyPage } from "./pages/WatchPartyPage";
import type { ClipListItem } from "./types/clips";

function App() {
  const [activePage, setActivePage] = useState<PageId>("replay");
  const [editorClip, setEditorClip] = useState<ClipListItem | null>(null);
  const [editorDirty, setEditorDirty] = useState(false);
  const [exportPlaybackClip, setExportPlaybackClip] = useState<ClipListItem | null>(null);
  const [toast, setToast] = useState<{ success: boolean; title: string; message: string } | null>(null);

  const showToast = useCallback((title: string, message: string, success: boolean) => {
    setToast({ title, message, success });
  }, []);

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
    replay: <ReplayPage />,
    watchParty: featureVisibility.watchParty ? <WatchPartyPage /> : <ReplayPage />,
    clips: <ClipsPage onEditClip={openEditor} playClip={exportPlaybackClip} onPlayClipConsumed={() => setExportPlaybackClip(null)} onToast={showToast} />,
    roulette: <ReplayRoulettePage onToast={showToast} />,
    editor: <EditorPage clip={editorClip} onBackToClips={closeEditor} onPlayExport={playExport} onDirtyChange={setEditorDirty} onToast={showToast} />,
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
    </div>
  );
}

export default App;
