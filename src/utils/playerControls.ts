export type PlaybackIntent = "play" | "pause";
export type PlayerShortcut = "togglePlayback" | "seekBackward" | "seekForward" | "toggleMute" | "toggleFullscreen" | "escape" | null;

export type SourceSwitchPlan = {
  restoreAtSeconds: number;
  resumePlaying: boolean;
  volume: number;
  muted: boolean;
};

export function clampMediaTime(value: number, duration: number) {
  if (!Number.isFinite(value) || !Number.isFinite(duration) || duration <= 0) return 0;
  return Math.min(Math.max(value, 0), duration);
}

export function mediaTimeToPercent(value: number, duration: number) {
  if (!Number.isFinite(duration) || duration <= 0) return 0;
  return clampMediaTime(value, duration) / duration * 100;
}

export function playbackIntent(paused: boolean): PlaybackIntent {
  return paused ? "play" : "pause";
}

export function volumePlan(value: number) {
  const volume = Math.min(Math.max(Number.isFinite(value) ? value : 0, 0), 1);
  return { volume, muted: volume === 0 };
}

export function toggledMuteState(muted: boolean, volume: number) {
  return volume <= 0 ? false : !muted;
}

export function planPlaybackSourceSwitch(
  currentTime: number,
  duration: number,
  paused: boolean,
  volume: number,
  muted: boolean,
): SourceSwitchPlan {
  return {
    restoreAtSeconds: clampMediaTime(currentTime, duration),
    resumePlaying: !paused,
    volume: volumePlan(volume).volume,
    muted,
  };
}

export function isEditableShortcutTarget(tagName: string | undefined, isContentEditable: boolean) {
  if (isContentEditable) return true;
  return tagName !== undefined && ["INPUT", "TEXTAREA", "SELECT"].includes(tagName.toUpperCase());
}

export function playerShortcut(
  key: string,
  code: string,
  tagName?: string,
  isContentEditable = false,
): PlayerShortcut {
  if (isEditableShortcutTarget(tagName, isContentEditable)) return null;
  if (code === "Space") return "togglePlayback";
  if (key === "ArrowLeft") return "seekBackward";
  if (key === "ArrowRight") return "seekForward";
  if (key.toLowerCase() === "m") return "toggleMute";
  if (key.toLowerCase() === "f") return "toggleFullscreen";
  if (key === "Escape") return "escape";
  return null;
}
