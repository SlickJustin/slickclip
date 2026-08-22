import assert from "node:assert/strict";
import test from "node:test";
import { addPlayedTime, playbackThresholdSeconds } from "../src/utils/watchProgress.ts";

test("normal clips require three seconds of actual playback", () => {
  assert.equal(playbackThresholdSeconds(30), 3);
  assert.equal(addPlayedTime(2.9, 0.09, 30, false).reachedThreshold, false);
  assert.equal(addPlayedTime(2.9, 0.1, 30, false).reachedThreshold, true);
});

test("clips shorter than three seconds count at fifty percent", () => {
  assert.equal(playbackThresholdSeconds(2), 1);
  assert.equal(addPlayedTime(0.49, 0.5, 2, false).reachedThreshold, false);
  assert.equal(addPlayedTime(0.5, 0.5, 2, false).reachedThreshold, true);
});

test("a counted player session cannot count again", () => {
  const result = addPlayedTime(3, 20, 60, true);
  assert.deepEqual(result, { accumulatedSeconds: 3, reachedThreshold: false });
});
