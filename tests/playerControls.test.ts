import assert from "node:assert/strict";
import test from "node:test";
import {
  clampMediaTime,
  isEditableShortcutTarget,
  mediaTimeToPercent,
  playbackIntent,
  planPlaybackSourceSwitch,
  playerShortcut,
  toggledMuteState,
  volumePlan,
} from "../src/utils/playerControls.ts";

test("seek values clamp and convert to percentage", () => {
  assert.equal(clampMediaTime(-5, 30), 0);
  assert.equal(clampMediaTime(31, 30), 30);
  assert.equal(mediaTimeToPercent(7.5, 30), 25);
  assert.equal(mediaTimeToPercent(10, 0), 0);
});

test("play pause intent reflects the media element state", () => {
  assert.equal(playbackIntent(true), "play");
  assert.equal(playbackIntent(false), "pause");
});

test("volume and mute plans remain synchronized", () => {
  assert.deepEqual(volumePlan(0), { volume: 0, muted: true });
  assert.deepEqual(volumePlan(0.65), { volume: 0.65, muted: false });
  assert.equal(toggledMuteState(false, 0.65), true);
  assert.equal(toggledMuteState(true, 0.65), false);
});

test("audio source switches preserve time playback volume and mute", () => {
  assert.deepEqual(planPlaybackSourceSwitch(12.5, 30, false, 0.4, true), {
    restoreAtSeconds: 12.5,
    resumePlaying: true,
    volume: 0.4,
    muted: true,
  });
});

test("shortcuts are suppressed in editable controls", () => {
  assert.equal(isEditableShortcutTarget("input", false), true);
  assert.equal(isEditableShortcutTarget("DIV", true), true);
  assert.equal(playerShortcut(" ", "Space", "INPUT"), null);
  assert.equal(playerShortcut("ArrowRight", "ArrowRight", "SELECT"), null);
  assert.equal(playerShortcut("m", "KeyM", "DIV"), "toggleMute");
});
