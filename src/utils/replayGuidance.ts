export function formatReplayWindow(seconds: number) {
  const safeSeconds = Number.isFinite(seconds) ? Math.max(0, Math.round(seconds)) : 0;
  if (safeSeconds >= 60 && safeSeconds % 60 === 0) {
    const minutes = safeSeconds / 60;
    return `${minutes} ${minutes === 1 ? "minute" : "minutes"}`;
  }
  return `${safeSeconds} ${safeSeconds === 1 ? "second" : "seconds"}`;
}

export function replayHotkeyGuidance(hotkey: string, seconds: number) {
  const shortcut = hotkey.trim() || "your Save Replay hotkey";
  return `Press ${shortcut} anytime to save the previous ${formatReplayWindow(seconds)}.`;
}

export function saveLastLabel(seconds: number) {
  return `Save Last ${formatReplayWindow(seconds)}`;
}
