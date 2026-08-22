import assert from "node:assert/strict";
import test from "node:test";
import type { ClipListItem } from "../src/types/clips.ts";
import {
  EDITOR_HISTORY_LIMIT,
  MIN_SEGMENT_DURATION_US,
  canDeleteSelectedSegment,
  canSplitAtPlayhead,
  createEditorSession,
  deleteSelectedSegment,
  editedTimeToSourceTime,
  formatEditorTime,
  formatEditorTimeUs,
  microsecondsToSeconds,
  previewTrimmedSegments,
  redoEditorEdit,
  resetEditorEdits,
  resetEditorSession,
  secondsToMicroseconds,
  segmentEditedOffsets,
  segmentSourcePositionToEditedTime,
  selectEditorSegment,
  sourceTimeToEditedTime,
  splitAtPlayhead,
  timelinePositionToSeconds,
  timelineTickTimes,
  totalEditedDurationUs,
  trimEditorSegment,
  undoEditorEdit,
  validateEditorSegments,
  withEditorDuration,
  withEditorPlaybackState,
  withEditorPlayhead,
  withEditorPlayheadUs,
  type EditorSession,
} from "../src/utils/editorSession.ts";

const second = secondsToMicroseconds;

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

function splitAt(session: EditorSession, seconds: number) {
  return splitAtPlayhead(withEditorPlayhead(session, seconds));
}

function threeSegments() {
  return splitAt(splitAt(createEditorSession(clip()), 10), 20);
}

test("untouched sessions contain one immutable full-source segment", () => {
  const session = createEditorSession(clip());
  assert.equal(session.source.clipId, "clip-one");
  assert.equal(session.source.durationUs, second(30));
  assert.deepEqual(session.segments, [{ id: "clip-one:segment:0", sourceStartUs: 0, sourceEndUs: second(30) }]);
  assert.equal(totalEditedDurationUs(session.segments), second(30));
  assert.equal(session.selectedSegmentId, "clip-one:segment:0");
  assert.equal(session.playheadUs, 0);
  assert.equal(session.dirty, false);
  assert.deepEqual(session.undoStack, []);
});

test("authoritative media duration updates a pristine timeline without history", () => {
  const session = withEditorDuration(createEditorSession(clip()), 42.5);
  assert.equal(session.source.durationUs, second(42.5));
  assert.equal(session.segments[0].sourceEndUs, second(42.5));
  assert.equal(session.undoStack.length, 0);
  assert.equal(session.dirty, false);
});

test("authoritative duration reconciliation clamps dirty end-of-source segments", () => {
  const edited = deleteSelectedSegment(selectEditorSegment(threeSegments(), "clip-one:segment:1"));
  const reconciled = withEditorDuration(edited, 29.999783);
  assert.deepEqual(reconciled.segments.map(({ sourceStartUs, sourceEndUs }) => [sourceStartUs, sourceEndUs]), [
    [0, second(10)],
    [second(20), 29_999_783],
  ]);
  assert.equal(reconciled.source.durationUs, 29_999_783);
  assert.equal(totalEditedDurationUs(reconciled.segments), 19_999_783);
  assert.equal(validateEditorSegments(reconciled.segments, reconciled.source.durationUs), true);
  assert.equal(reconciled.dirty, true);
  assert.ok(reconciled.undoStack.every((state) => state.segments.every((segment) => segment.sourceEndUs <= 29_999_783)));
});

test("trim start changes only the selected source start", () => {
  const session = createEditorSession(clip());
  const trimmed = trimEditorSegment(session, session.segments[0].id, "start", second(5.5));
  assert.deepEqual(trimmed.segments[0], { ...session.segments[0], sourceStartUs: second(5.5) });
  assert.equal(trimmed.segments[0].sourceEndUs, second(30));
  assert.equal(trimmed.dirty, true);
  assert.equal(trimmed.undoStack.length, 1);
});

test("trim end changes only the selected source end", () => {
  const session = createEditorSession(clip());
  const trimmed = trimEditorSegment(session, session.segments[0].id, "end", second(24.25));
  assert.equal(trimmed.segments[0].sourceStartUs, 0);
  assert.equal(trimmed.segments[0].sourceEndUs, second(24.25));
});

test("trim handles enforce the minimum segment duration", () => {
  const session = createEditorSession(clip());
  const startTrimmed = trimEditorSegment(session, session.segments[0].id, "start", second(30));
  assert.equal(startTrimmed.segments[0].sourceStartUs, second(30) - MIN_SEGMENT_DURATION_US);
  const endTrimmed = trimEditorSegment(session, session.segments[0].id, "end", 0);
  assert.equal(endTrimmed.segments[0].sourceEndUs, MIN_SEGMENT_DURATION_US);
});

test("transient trim previews do not mutate the authoritative segment array", () => {
  const session = createEditorSession(clip());
  const preview = previewTrimmedSegments(session.segments, session.source.durationUs, session.segments[0].id, "start", second(3));
  assert.equal(session.segments[0].sourceStartUs, 0);
  assert.equal(preview[0].sourceStartUs, second(3));
  assert.equal(session.undoStack.length, 0);
});

test("split creates two valid stable segment identities", () => {
  const session = createEditorSession(clip());
  const split = splitAt(session, 12);
  assert.deepEqual(split.segments, [
    { id: "clip-one:segment:0", sourceStartUs: 0, sourceEndUs: second(12) },
    { id: "clip-one:segment:1", sourceStartUs: second(12), sourceEndUs: second(30) },
  ]);
  assert.equal(split.selectedSegmentId, "clip-one:segment:1");
  assert.equal(split.nextSegmentOrdinal, 2);
});

test("split rejects exact and minimum-tolerance boundaries", () => {
  const session = createEditorSession(clip());
  assert.equal(canSplitAtPlayhead(session), false);
  assert.strictEqual(splitAtPlayhead(session), session);
  assert.equal(canSplitAtPlayhead(withEditorPlayheadUs(session, MIN_SEGMENT_DURATION_US - 1)), false);
  assert.equal(canSplitAtPlayhead(withEditorPlayheadUs(session, MIN_SEGMENT_DURATION_US)), true);
  assert.equal(canSplitAtPlayhead(withEditorPlayhead(session, 30)), false);
});

test("delete removes a middle segment and closes edited time", () => {
  const session = threeSegments();
  const middle = session.segments[1].id;
  const deleted = deleteSelectedSegment(selectEditorSegment(session, middle));
  assert.deepEqual(deleted.segments.map(({ sourceStartUs, sourceEndUs }) => [sourceStartUs, sourceEndUs]), [
    [0, second(10)],
    [second(20), second(30)],
  ]);
  assert.equal(totalEditedDurationUs(deleted.segments), second(20));
});

test("delete can remove the first segment", () => {
  const session = threeSegments();
  const deleted = deleteSelectedSegment(selectEditorSegment(session, session.segments[0].id));
  assert.deepEqual(deleted.segments.map((segment) => segment.sourceStartUs), [second(10), second(20)]);
});

test("delete can remove the last segment", () => {
  const session = threeSegments();
  const deleted = deleteSelectedSegment(selectEditorSegment(session, session.segments[2].id));
  assert.deepEqual(deleted.segments.map((segment) => segment.sourceEndUs), [second(10), second(20)]);
});

test("delete refuses to remove the only remaining segment", () => {
  const session = createEditorSession(clip());
  assert.equal(canDeleteSelectedSegment(session), false);
  assert.strictEqual(deleteSelectedSegment(session), session);
});

test("edited time maps through multiple source ranges", () => {
  const session = deleteSelectedSegment(selectEditorSegment(threeSegments(), "clip-one:segment:1"));
  assert.equal(editedTimeToSourceTime(session.segments, second(7))?.sourceTimeUs, second(7));
  assert.equal(editedTimeToSourceTime(session.segments, second(12))?.sourceTimeUs, second(22));
  assert.equal(editedTimeToSourceTime(session.segments, second(20))?.sourceTimeUs, second(30));
});

test("source positions map to explicit edited segment offsets", () => {
  const session = deleteSelectedSegment(selectEditorSegment(threeSegments(), "clip-one:segment:1"));
  const offsets = segmentEditedOffsets(session.segments);
  assert.deepEqual(offsets.map(({ editedStartUs, editedEndUs }) => [editedStartUs, editedEndUs]), [
    [0, second(10)],
    [second(10), second(20)],
  ]);
  assert.equal(segmentSourcePositionToEditedTime(session.segments, session.segments[1].id, second(24)), second(14));
  assert.equal(sourceTimeToEditedTime(session.segments, second(15)), null);
});

test("seeking immediately before and after a cut selects the correct source side", () => {
  const session = deleteSelectedSegment(selectEditorSegment(threeSegments(), "clip-one:segment:1"));
  assert.equal(editedTimeToSourceTime(session.segments, second(10) - 1)?.sourceTimeUs, second(10) - 1);
  const after = editedTimeToSourceTime(session.segments, second(10));
  assert.equal(after?.segmentId, "clip-one:segment:2");
  assert.equal(after?.sourceTimeUs, second(20));
});

test("undo and redo restore a trim", () => {
  const session = createEditorSession(clip());
  const trimmed = trimEditorSegment(session, session.segments[0].id, "start", second(4));
  const undone = undoEditorEdit(trimmed);
  assert.deepEqual(undone.segments, session.segments);
  assert.equal(undone.dirty, false);
  assert.deepEqual(redoEditorEdit(undone).segments, trimmed.segments);
});

test("undo restores split identities and redo returns them", () => {
  const split = splitAt(createEditorSession(clip()), 8);
  const ids = split.segments.map((segment) => segment.id);
  const undone = undoEditorEdit(split);
  assert.equal(undone.segments.length, 1);
  assert.deepEqual(redoEditorEdit(undone).segments.map((segment) => segment.id), ids);
});

test("undo restores a deleted segment", () => {
  const split = splitAt(createEditorSession(clip()), 10);
  const deleted = deleteSelectedSegment(split);
  assert.equal(deleted.segments.length, 1);
  assert.deepEqual(undoEditorEdit(deleted).segments, split.segments);
});

test("a new edit clears redo history", () => {
  const split = splitAt(createEditorSession(clip()), 10);
  const undone = undoEditorEdit(split);
  assert.equal(undone.redoStack.length, 1);
  const trimmed = trimEditorSegment(undone, undone.segments[0].id, "start", second(2));
  assert.equal(trimmed.redoStack.length, 0);
});

test("reset restores the full source and is undoable", () => {
  const trimmed = trimEditorSegment(createEditorSession(clip()), "clip-one:segment:0", "start", second(3));
  const reset = resetEditorEdits(trimmed);
  assert.deepEqual(reset.segments.map(({ sourceStartUs, sourceEndUs }) => [sourceStartUs, sourceEndUs]), [[0, second(30)]]);
  assert.equal(reset.dirty, false);
  assert.deepEqual(undoEditorEdit(reset).segments, trimmed.segments);
});

test("history is bounded to the configured limit", () => {
  let session = createEditorSession(clip());
  for (let index = 0; index < EDITOR_HISTORY_LIMIT + 12; index += 1) {
    const requestedUs = index % 2 === 0 ? second(1) : second(2);
    session = trimEditorSegment(session, session.segments[0].id, "start", requestedUs);
  }
  assert.equal(session.undoStack.length, EDITOR_HISTORY_LIMIT);
});

test("deleting material under the playhead clamps to the nearest cut boundary", () => {
  let session = threeSegments();
  session = withEditorPlayhead(session, 15);
  session = selectEditorSegment(session, session.segments[1].id);
  const deleted = deleteSelectedSegment(session);
  assert.equal(deleted.playheadUs, second(10));
  assert.equal(editedTimeToSourceTime(deleted.segments, deleted.playheadUs)?.sourceTimeUs, second(20));
});

test("all edit operations preserve valid ordered non-overlapping ranges", () => {
  const session = threeSegments();
  const invalid = [
    { id: "a", sourceStartUs: -1, sourceEndUs: second(2) },
    { id: "b", sourceStartUs: second(1), sourceEndUs: second(3) },
  ];
  assert.equal(validateEditorSegments(invalid, second(30)), false);
  assert.equal(validateEditorSegments(session.segments, session.source.durationUs), true);
  const trimmed = trimEditorSegment(session, session.segments[1].id, "start", second(9));
  assert.equal(trimmed.segments[1].sourceStartUs, session.segments[0].sourceEndUs);
  assert.equal(validateEditorSegments(trimmed.segments, trimmed.source.durationUs), true);
});

test("opening another source resets edits, history, selection, and transport", () => {
  const first = withEditorPlaybackState(splitAt(createEditorSession(clip()), 12), "playing");
  const secondSession = resetEditorSession(clip({ id: "clip-two", filePath: "C:\\Clips\\two.mp4", displayName: "Two", duration100ns: 100_000_000 }));
  assert.equal(first.dirty, true);
  assert.equal(secondSession.source.clipId, "clip-two");
  assert.equal(secondSession.segments.length, 1);
  assert.equal(secondSession.playheadUs, 0);
  assert.equal(secondSession.playbackState, "loading");
  assert.equal(secondSession.undoStack.length, 0);
});

test("time conversion, timeline ticks, and formatting retain subsecond precision", () => {
  assert.equal(secondsToMicroseconds(1.234567), 1_234_567);
  assert.equal(microsecondsToSeconds(1_234_567), 1.234567);
  assert.equal(timelinePositionToSeconds(75, 300, 40), 10);
  assert.deepEqual(timelineTickTimes(8, 4), [0, 2, 4, 6, 8]);
  assert.equal(formatEditorTime(65.349), "1:05.34");
  assert.equal(formatEditorTimeUs(65_349_000), "1:05.34");
  assert.equal(formatEditorTime(Number.NaN), "0:00.00");
});
