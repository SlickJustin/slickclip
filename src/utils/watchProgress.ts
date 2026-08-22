export function playbackThresholdSeconds(durationSeconds: number) {
  if (!Number.isFinite(durationSeconds) || durationSeconds <= 0) return 3;
  return durationSeconds < 3 ? durationSeconds * 0.5 : 3;
}

export function addPlayedTime(
  accumulatedSeconds: number,
  elapsedPlaybackSeconds: number,
  durationSeconds: number,
  alreadyCounted: boolean,
) {
  const accumulated = Math.max(0, accumulatedSeconds);
  if (alreadyCounted || !Number.isFinite(elapsedPlaybackSeconds) || elapsedPlaybackSeconds <= 0) {
    return { accumulatedSeconds: accumulated, reachedThreshold: false };
  }
  const next = accumulated + elapsedPlaybackSeconds;
  return {
    accumulatedSeconds: next,
    reachedThreshold: next >= playbackThresholdSeconds(durationSeconds),
  };
}
