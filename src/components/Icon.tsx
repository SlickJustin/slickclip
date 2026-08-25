export type IconName =
  | "replay"
  | "watchParty"
  | "clips"
  | "roulette"
  | "editor"
  | "help"
  | "settings"
  | "minimize"
  | "maximize"
  | "restore"
  | "close";

type IconProps = {
  name: IconName;
  size?: number;
  className?: string;
};

export function Icon({ name, size = 18, className }: IconProps) {
  return (
    <svg
      aria-hidden="true"
      className={className}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth="1.7"
      strokeLinecap="round"
      strokeLinejoin="round"
    >
      {iconPaths[name]}
    </svg>
  );
}

const iconPaths: Record<IconName, React.ReactNode> = {
  replay: <><path d="M4.8 8.2A8 8 0 1 1 4 14" /><path d="M4.8 4.7v3.5h3.5" /><path d="m10 9 5 3-5 3Z" /></>,
  watchParty: <><rect x="3" y="5" width="12" height="14" rx="2" /><path d="m15 10 5-3v10l-5-3" /></>,
  clips: <><rect x="3" y="5" width="18" height="14" rx="2" /><path d="M7 5v14M17 5v14M3 9h4m10 0h4M3 15h4m10 0h4" /></>,
  roulette: <><path d="M16 3h5v5" /><path d="m21 3-6.2 6.2a4 4 0 0 1-5.6 0L3 3" /><path d="M16 21h5v-5" /><path d="m21 21-6.2-6.2a4 4 0 0 0-5.6 0L3 21" /></>,
  editor: <><path d="m4 4 16 16M14.5 14.5 20 9" /><circle cx="6" cy="17" r="3" /><circle cx="6" cy="7" r="3" /></>,
  help: <><circle cx="12" cy="12" r="9" /><path d="M9.8 9a2.35 2.35 0 1 1 3.45 2.08c-.78.42-1.25.91-1.25 1.92" /><path d="M12 17h.01" /></>,
  settings: <><circle cx="12" cy="12" r="3" /><path d="M19.4 15a1.7 1.7 0 0 0 .3 1.9l.1.1-2.8 2.8-.1-.1a1.7 1.7 0 0 0-1.9-.3 1.7 1.7 0 0 0-1 1.6v.2h-4V21a1.7 1.7 0 0 0-1-1.6 1.7 1.7 0 0 0-1.9.3l-.1.1L4.2 17l.1-.1a1.7 1.7 0 0 0 .3-1.9A1.7 1.7 0 0 0 3 14H2.8v-4H3a1.7 1.7 0 0 0 1.6-1 1.7 1.7 0 0 0-.3-1.9L4.2 7 7 4.2l.1.1A1.7 1.7 0 0 0 9 4.6 1.7 1.7 0 0 0 10 3v-.2h4V3a1.7 1.7 0 0 0 1 1.6 1.7 1.7 0 0 0 1.9-.3l.1-.1L19.8 7l-.1.1a1.7 1.7 0 0 0-.3 1.9 1.7 1.7 0 0 0 1.6 1h.2v4H21a1.7 1.7 0 0 0-1.6 1Z" /></>,
  minimize: <path d="M6 12h12" />,
  maximize: <rect x="6" y="6" width="12" height="12" rx=".5" />,
  restore: <><path d="M9 8V6h9v9h-2" /><rect x="6" y="9" width="9" height="9" rx=".5" /></>,
  close: <><path d="m7 7 10 10M17 7 7 17" /></>,
};
