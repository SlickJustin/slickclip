import assert from "node:assert/strict";
import test from "node:test";
import { rouletteWeight, selectRouletteClip } from "../src/utils/replayRoulette.ts";

const now = 2_000_000_000_000;

test("roulette returns null for an empty Library", () => {
  assert.equal(selectRouletteClip([], [], () => 0, now), null);
});

test("roulette avoids recent selections while another clip is available", () => {
  const clips = [
    { id: "recent", playCount: 0, lastWatchedAtMs: null },
    { id: "fresh", playCount: 50, lastWatchedAtMs: now },
  ];
  assert.equal(selectRouletteClip(clips, ["recent"], () => 0, now)?.id, "fresh");
});

test("roulette falls back to the full pool when every clip is recent", () => {
  const clips = [{ id: "only", playCount: 1, lastWatchedAtMs: now }];
  assert.equal(selectRouletteClip(clips, ["only"], () => 0.5, now)?.id, "only");
});

test("unwatched and less-played clips receive more weight", () => {
  const unwatched = rouletteWeight({ id: "new", playCount: 0, lastWatchedAtMs: null }, now);
  const heavilyPlayed = rouletteWeight({ id: "old", playCount: 25, lastWatchedAtMs: now - 30 * 24 * 60 * 60 * 1_000 }, now);
  const justWatched = rouletteWeight({ id: "recent", playCount: 0, lastWatchedAtMs: now }, now);
  assert.ok(unwatched > heavilyPlayed);
  assert.ok(heavilyPlayed > justWatched);
});

test("roulette uses weighted intervals and safely bounds the random source", () => {
  const clips = [
    { id: "first", playCount: 0, lastWatchedAtMs: null },
    { id: "last", playCount: 0, lastWatchedAtMs: null },
  ];
  assert.equal(selectRouletteClip(clips, [], () => -1, now)?.id, "first");
  assert.equal(selectRouletteClip(clips, [], () => 2, now)?.id, "last");
});
