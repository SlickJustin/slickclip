import slickClipLogo from "../assets/branding/slickclip-logo.svg";
import { featureVisibility } from "../config/features";
import { Icon, type IconName } from "./Icon";

export type PageId = "replay" | "watchParty" | "clips" | "roulette" | "editor" | "help" | "settings";

type SidebarProps = {
  activePage: PageId;
  onNavigate: (page: PageId) => void;
};

type NavigationItem = { id: PageId; label: string; icon: IconName };

const navigationGroups: { label: string; items: NavigationItem[] }[] = [
  { label: "Capture", items: [
    { id: "replay", label: "Replay", icon: "replay" },
    ...(featureVisibility.watchParty
      ? [{ id: "watchParty", label: "Watch Party", icon: "watchParty" } satisfies NavigationItem]
      : []),
  ] },
  { label: "Library", items: [
    { id: "clips", label: "Clips", icon: "clips" },
    { id: "roulette", label: "Replay Roulette", icon: "roulette" },
    { id: "editor", label: "Editor", icon: "editor" },
  ] },
];

function NavigationButton({ item, activePage, onNavigate }: { item: NavigationItem; activePage: PageId; onNavigate: (page: PageId) => void }) {
  return (
    <button
      className={`nav-item${activePage === item.id ? " nav-item-active" : ""}`}
      type="button"
      aria-current={activePage === item.id ? "page" : undefined}
      onClick={() => onNavigate(item.id)}
    >
      <span className="nav-indicator" aria-hidden="true" />
      <Icon className="nav-icon" name={item.icon} />
      <span>{item.label}</span>
    </button>
  );
}

export function Sidebar({ activePage, onNavigate }: SidebarProps) {
  return (
    <aside className="sidebar">
      <div className="sidebar-brand">
        <img src={slickClipLogo} alt="SlickClip" />
        <span>Capture workspace</span>
      </div>

      <div className="sidebar-navigation">
        {navigationGroups.map((group) => (
          <section className="sidebar-nav-section" key={group.label}>
            <span className="sidebar-nav-label">{group.label}</span>
            <nav className="sidebar-nav" aria-label={`${group.label} navigation`}>
              {group.items.map((item) => <NavigationButton item={item} activePage={activePage} onNavigate={onNavigate} key={item.id} />)}
            </nav>
          </section>
        ))}
      </div>

      <div className="sidebar-utility">
        <NavigationButton item={{ id: "help", label: "Help", icon: "help" }} activePage={activePage} onNavigate={onNavigate} />
        <NavigationButton item={{ id: "settings", label: "Settings", icon: "settings" }} activePage={activePage} onNavigate={onNavigate} />
        <div className="sidebar-footer">
          <span className="sidebar-footer-status"><i aria-hidden="true" />Local workspace</span>
          <span className="sidebar-version">v1.0.2</span>
        </div>
      </div>
    </aside>
  );
}
