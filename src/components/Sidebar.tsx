export type PageId = "replay" | "clips" | "editor" | "settings";

type SidebarProps = {
  activePage: PageId;
  onNavigate: (page: PageId) => void;
};

const navigationItems: { id: PageId; label: string }[] = [
  { id: "replay", label: "Replay" },
  { id: "clips", label: "Clips" },
  { id: "editor", label: "Editor" },
  { id: "settings", label: "Settings" },
];

export function Sidebar({ activePage, onNavigate }: SidebarProps) {
  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <span className="brand-mark" aria-hidden="true">R</span>
        <div>
          <strong>JustIn Replay</strong>
          <span>DEV BUILD</span>
        </div>
      </div>

      <nav className="sidebar-nav" aria-label="Primary navigation">
        {navigationItems.map((item) => (
          <button
            className={`nav-item${activePage === item.id ? " nav-item-active" : ""}`}
            key={item.id}
            type="button"
            aria-current={activePage === item.id ? "page" : undefined}
            onClick={() => onNavigate(item.id)}
          >
            <span className="nav-indicator" aria-hidden="true" />
            {item.label}
          </button>
        ))}
      </nav>

      <span className="sidebar-version">v0.1.0</span>
    </aside>
  );
}
