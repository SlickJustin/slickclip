export type RouletteCandidate = {
  id: string;
  playCount: number;
  lastWatchedAtMs: number | null;
};

const HOUR_MS = 60 * 60 * 1_000;
const DAY_MS = 24 * HOUR_MS;

export function rouletteWeight(candidate: RouletteCandidate, nowMs = Date.now()) {
  const playCount = Math.max(0, candidate.playCount);
  const playWeight = 1 / (1 + Math.sqrt(playCount));
  if (candidate.lastWatchedAtMs === null) return playWeight * 4;

  const ageMs = Math.max(0, nowMs - candidate.lastWatchedAtMs);
  const recencyWeight = ageMs < HOUR_MS
    ? 0.12
    : ageMs < DAY_MS
      ? 0.3
      : ageMs < 7 * DAY_MS
        ? 0.65
        : 1;
  return playWeight * recencyWeight;
}

export function selectRouletteClip<T extends RouletteCandidate>(
  candidates: readonly T[],
  recentIds: readonly string[] = [],
  random: () => number = Math.random,
  nowMs = Date.now(),
): T | null {
  if (candidates.length === 0) return null;

  const recent = new Set(recentIds);
  const freshCandidates = candidates.filter((candidate) => !recent.has(candidate.id));
  const pool = freshCandidates.length > 0 ? freshCandidates : [...candidates];
  const weights = pool.map((candidate) => rouletteWeight(candidate, nowMs));
  const totalWeight = weights.reduce((total, weight) => total + weight, 0);
  const boundedRandom = Math.min(Math.max(random(), 0), 1 - Number.EPSILON);
  let target = boundedRandom * totalWeight;

  for (let index = 0; index < pool.length; index += 1) {
    target -= weights[index];
    if (target < 0) return pool[index];
  }
  return pool[pool.length - 1];
}
