import slickClipLogo from "../assets/branding/slickclip-logo.svg";

export type PageId = "replay" | "clips" | "roulette" | "editor" | "settings";

type SidebarProps = {
  activePage: PageId;
  onNavigate: (page: PageId) => void;
};

const navigationItems: { id: PageId; label: string }[] = [
  { id: "replay", label: "Replay" },
  { id: "clips", label: "Clips" },
  { id: "roulette", label: "Replay Roulette" },
  { id: "editor", label: "Editor" },
  { id: "settings", label: "Settings" },
];

export function Sidebar({ activePage, onNavigate }: SidebarProps) {
  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <img src={slickClipLogo} alt="SlickClip" />
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

      <span className="sidebar-version">v1.0.0</span>
    </aside>
  );
}
