import { useEffect, useState } from "react";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import "./App.css";
import { Sidebar, type PageId } from "./components/Sidebar";
import { ClipsPage } from "./pages/ClipsPage";
import { EditorPage } from "./pages/EditorPage";
import { ReplayPage } from "./pages/ReplayPage";
import { SettingsPage } from "./pages/SettingsPage";
import type { ClipListItem } from "./types/clips";

function App() {
  const [activePage, setActivePage] = useState<PageId>("replay");
  const [editorClip, setEditorClip] = useState<ClipListItem | null>(null);
  const [hotkeyFeedback, setHotkeyFeedback] = useState<{ success: boolean; message: string } | null>(null);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    void listen<{ success: boolean; message: string }>("save-replay-hotkey-feedback", (event) => {
      setHotkeyFeedback(event.payload);
    }).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  useEffect(() => {
    if (!hotkeyFeedback) return;
    const timer = window.setTimeout(() => setHotkeyFeedback(null), 3_500);
    return () => window.clearTimeout(timer);
  }, [hotkeyFeedback]);

  function navigate(page: PageId) {
    if (page !== "editor" || activePage !== "editor") setEditorClip(null);
    setActivePage(page);
  }

  function openEditor(clip: ClipListItem) {
    setEditorClip(clip);
    setActivePage("editor");
  }

  function closeEditor() {
    setEditorClip(null);
    setActivePage("clips");
  }

  const pages: Record<PageId, React.ReactNode> = {
    replay: <ReplayPage />,
    clips: <ClipsPage onEditClip={openEditor} />,
    editor: <EditorPage clip={editorClip} onBackToClips={closeEditor} />,
    settings: <SettingsPage />,
  };

  return (
    <div className="app-shell">
      <Sidebar activePage={activePage} onNavigate={navigate} />
      <main className="app-content">{pages[activePage]}</main>
      {hotkeyFeedback && (
        <div className={`hotkey-feedback ${hotkeyFeedback.success ? "hotkey-feedback-success" : "hotkey-feedback-error"}`} role="status" aria-live="polite">
          {hotkeyFeedback.message}
        </div>
      )}
    </div>
  );
}

export default App;
