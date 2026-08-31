export type HelpSection = {
  id: string;
  title: string;
  paragraphs?: string[];
  steps?: string[];
  bullets?: string[];
};

export function buildHelpSections(saveReplayHotkey: string): HelpSection[] {
  const hotkey = saveReplayHotkey.trim() || "your configured Save Replay hotkey";

  return [
    {
      id: "getting-started",
      title: "Getting Started",
      paragraphs: [
        "Replay Buffer does not continuously create permanent recordings. It only keeps a temporary rolling window until you choose to save it.",
      ],
      steps: [
        "Choose what SlickClip should capture on the Replay page.",
        "Start Replay Buffer. SlickClip begins keeping the most recent configured amount of gameplay ready.",
        "Play normally.",
        `When something happens, press ${hotkey}. SlickClip saves the previous configured duration—not the next duration.`,
        "Open Clips to watch, organize, edit, copy, or share the saved clip.",
      ],
    },
    {
      id: "how-replay-works",
      title: "How Replay Works",
      paragraphs: [
        "Replay Buffer continually replaces its oldest temporary footage with new footage. Nothing becomes a permanent clip until you save a replay.",
        "The duration selected on Replay controls how far back a save can reach. A newly started buffer may need time to fill that whole window.",
      ],
    },
    {
      id: "saving-a-replay",
      title: "Saving a Replay",
      paragraphs: [
        `The primary way to save is ${hotkey}. You can change and test this shortcut in Settings.`,
        "You can also configure an optional Save & Name hotkey. It saves the same Replay exactly once, then brings SlickClip forward to name it after Library indexing succeeds.",
        "While Replay Buffer is running, Save Last on the Replay card provides the same manual fallback. Save progress and completion appear on that card.",
      ],
    },
    {
      id: "game-detection",
      title: "Game Detection",
      paragraphs: [
        "For the normal automatic workflow, leave Game Detection and Automatically start Replay enabled, then launch a game. SlickClip waits for a stable, high-confidence game window, starts one Replay Buffer, and shows Replay Ready when capture is running.",
        `Play normally and press ${hotkey} after a moment you want to keep. When the game closes, SlickClip safely stops its automatically started buffer without saving anything on its own.`,
        "Any detected game is the recommended mode. Approved games only is an optional strict allowlist for advanced setups. Exclusions always take priority in either mode.",
        "Choosing a source manually on Replay temporarily overrides automatic detection, so SlickClip will not replace your selection.",
      ],
    },
    {
      id: "audio-setup",
      title: "Audio Setup",
      paragraphs: [
        "On Replay, enable only the sources you want and choose an application or microphone for each one. Game, voice chat, and microphone audio are retained as separate adjustable tracks.",
        "Normal clip playback uses the combined track. The Editor exposes the separate tracks when they are available.",
      ],
    },
    {
      id: "clips-and-collections",
      title: "Clips & Collections",
      paragraphs: [
        "Clips is your local library for watching, searching, favoriting, organizing, copying, and opening saved clips. Collections organize clips without moving the underlying video files.",
      ],
    },
    {
      id: "editing-a-clip",
      title: "Editing a Clip",
      paragraphs: [
        "Editing is nondestructive: trims, cuts, and audio adjustments do not alter the source clip. Export creates a new flattened video and adds it to Clips.",
      ],
    },
    {
      id: "storage-and-cleanup",
      title: "Storage & Cleanup",
      paragraphs: [
        "The Library quota in Settings can identify the oldest clips for cleanup when storage exceeds your limit. You review a preview before anything is deleted.",
        "Protect from Cleanup excludes a clip from automatic cleanup. It does not prevent you from deleting that clip manually.",
      ],
    },
    {
      id: "keyboard-shortcuts",
      title: "Keyboard Shortcuts",
      bullets: [
        `${hotkey}: save the previous Replay window while Replay Buffer is running.`,
        "Ctrl+A in Clips: select all visible clips.",
        "Escape in Clips: clear the current selection or close the open clip player.",
      ],
    },
    {
      id: "troubleshooting",
      title: "Troubleshooting",
      bullets: [
        "If a source is missing, use Refresh Sources and make sure its window or audio application is open.",
        "If Replay will not start, confirm a capture source is selected and every enabled audio row has a source.",
        "Why wasn’t my game detected? Keep its main game window visible for a few seconds, check that Game Detection and automatic start are enabled, confirm the app is not excluded, and use Any detected game mode. Launchers, helpers, tiny windows, and ordinary desktop apps are intentionally filtered out.",
        "If the save shortcut does not respond, use Test Hotkey in Settings and choose a shortcut that is not already reserved by another app.",
        "For capture or save errors, expand Capture diagnostics on Replay after reproducing the problem.",
      ],
    },
  ];
}
