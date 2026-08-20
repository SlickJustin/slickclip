import assert from "node:assert/strict";
import test from "node:test";
import type { ClipListItem } from "../src/types/clips.ts";
import {
  createEditorSession,
  formatEditorTime,
  resetEditorSession,
  timelinePositionToSeconds,
  timelineTickTimes,
  withEditorDuration,
  withEditorPlayhead,
  withEditorPlaybackState,
} from "../src/utils/editorSession.ts";

function clip(overrides: Partial<ClipListItem> = {}): ClipListItem {
  return {
    id: "clip-one",
    filePath: "C:\\Clips\\one.mp4",
    filename: "one.mp4",
    displayName: "One",
    createdAtMs: 0,
    fileModifiedAtMs: 0,
    fileSizeBytes: 100,
    duration100ns: 300_000_000,
    requestedDurationSeconds: 30,
    width: 1920,
    height: 1080,
    fpsNumerator: 60,
    fpsDenominator: 1,
    videoCodec: "hevc",
    videoProfile: null,
    videoBitrateBps: null,
    totalBitrateBps: null,
    captureTargetLabel: "Game",
    captureTargetType: "window",
    favorite: false,
    importedExistingFile: false,
    audioStreamCount: 1,
    defaultAudioStreamTitle: "Combined",
    audioTracks: [],
    ...overrides,
  };
}

test("editor sessions initialize against an immutable full-source range", () => {
  const sourceClip = clip();
  const session = createEditorSession(sourceClip);
  assert.equal(session.source.clipId, "clip-one");
  assert.equal(session.source.filePath, "C:\\Clips\\one.mp4");
  assert.equal(session.source.durationSeconds, 30);
  assert.deepEqual(session.editableRange, { startSeconds: 0, endSeconds: 30 });
  assert.equal(session.playheadSeconds, 0);
  assert.equal(session.playbackState, "loading");
});

test("playheads clamp and authoritative duration keeps a full-source range", () => {
  const initialized = createEditorSession(clip());
  const late = withEditorPlayhead(initialized, 45);
  assert.equal(late.playheadSeconds, 30);
  const reconciled = withEditorDuration(late, 42.5);
  assert.equal(reconciled.playheadSeconds, 30);
  assert.deepEqual(reconciled.editableRange, { startSeconds: 0, endSeconds: 42.5 });
  assert.equal(withEditorPlayhead(reconciled, -4).playheadSeconds, 0);
});

test("timeline position conversion and ticks handle bounds and clip lengths", () => {
  assert.equal(timelinePositionToSeconds(75, 300, 40), 10);
  assert.equal(timelinePositionToSeconds(-5, 300, 40), 0);
  assert.equal(timelinePositionToSeconds(500, 300, 40), 40);
  assert.deepEqual(timelineTickTimes(8, 4), [0, 2, 4, 6, 8]);
  assert.deepEqual(timelineTickTimes(125, 5), [0, 25, 50, 75, 100, 125]);
});

test("opening another source resets transient session state", () => {
  const first = withEditorPlaybackState(withEditorPlayhead(createEditorSession(clip()), 12), "playing");
  const second = resetEditorSession(clip({ id: "clip-two", filePath: "C:\\Clips\\two.mp4", displayName: "Two", duration100ns: 100_000_000 }));
  assert.equal(first.playheadSeconds, 12);
  assert.equal(second.source.clipId, "clip-two");
  assert.equal(second.source.filePath, "C:\\Clips\\two.mp4");
  assert.equal(second.playheadSeconds, 0);
  assert.equal(second.playbackState, "loading");
  assert.deepEqual(second.editableRange, { startSeconds: 0, endSeconds: 10 });
});

test("editor time formatting is stable for timeline readouts", () => {
  assert.equal(formatEditorTime(0), "0:00.00");
  assert.equal(formatEditorTime(65.349), "1:05.34");
  assert.equal(formatEditorTime(Number.NaN), "0:00.00");
});
