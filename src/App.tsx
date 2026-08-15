import { useState } from "react";
import "./App.css";
import { Sidebar, type PageId } from "./components/Sidebar";
import { ClipsPage } from "./pages/ClipsPage";
import { EditorPage } from "./pages/EditorPage";
import { ReplayPage } from "./pages/ReplayPage";
import { SettingsPage } from "./pages/SettingsPage";

function App() {
  const [activePage, setActivePage] = useState<PageId>("replay");

  const pages: Record<PageId, React.ReactNode> = {
    replay: <ReplayPage />,
    clips: <ClipsPage />,
    editor: <EditorPage />,
    settings: <SettingsPage />,
  };

  return (
    <div className="app-shell">
      <Sidebar activePage={activePage} onNavigate={setActivePage} />
      <main className="app-content">{pages[activePage]}</main>
    </div>
  );
}

export default App;
