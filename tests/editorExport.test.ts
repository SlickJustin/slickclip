import assert from "node:assert/strict";
import test from "node:test";
import type { EditorExportPhase, EditorExportStatus } from "../src/types/clips.ts";
import {
  adoptEditorExportStatus,
  applyEditorExportEvent,
  areEditorControlsLocked,
  createEditorExportUiState,
  isEditorExportActive,
  requestEditorExportCancellation,
  snapshotEditorExport,
} from "../src/utils/editorExport.ts";
import { createEditorMixer, withEditorTrackGain } from "../src/utils/editorMixer.ts";
import { createEditorSession, splitAtPlayhead, withEditorPlayhead } from "../src/utils/editorSession.ts";
import type { ClipAudioTrack, ClipListItem } from "../src/types/clips.ts";

function status(exportId: string, phase: EditorExportPhase, progressPercent = 0): EditorExportStatus {
  return {
    exportId,
    sourceClipId: "clip-one",
    phase,
    progressPercent,
    encodedTimeUs: 0,
    totalTimeUs: 10_000_000,
    encoder: null,
    encoderHardware: null,
    encoderSettings: null,
    attemptedEncoders: [],
    filterPlan: null,
    plannedDurationUs: null,
    verifiedDurationUs: null,
    outputClip: null,
    outputDisplayName: null,
    indexingWarning: null,
    errorMessage: null,
    diagnostics: [],
  };
}

function clip(): ClipListItem {
  return {
    id: "clip-one",
    filePath: "C:\\Clips\\one.mp4",
    filename: "one.mp4",
    displayName: "One",
    createdAtMs: 0,
    fileModifiedAtMs: 0,
    fileSizeBytes: 1,
    duration100ns: 100_000_000,
    requestedDurationSeconds: 10,
    width: 1920,
    height: 1080,
    fpsNumerator: 60,
    fpsDenominator: 1,
    videoCodec: "hevc",
    videoProfile: null,
    videoBitrateBps: null,
    totalBitrateBps: null,
    captureTargetLabel: null,
    captureTargetType: null,
    favorite: false,
    pinned: false,
    importedExistingFile: false,
    audioStreamCount: 1,
    defaultAudioStreamTitle: "Game",
    audioTracks: [],
  };
}

const game: ClipAudioTrack = {
  streamIndex: 2,
  role: "Game",
  title: "Game",
  handlerName: "Game",
  codec: "aac",
  profile: "LC",
  sampleRate: 48_000,
  channels: 2,
  bitrateBps: 192_000,
  isDefault: false,
};

test("all active phases lock controls and terminal phases restore them", () => {
  for (const phase of ["preparing", "rendering", "verifying", "finalizing"] as const) {
    assert.equal(isEditorExportActive(status("one", phase)), true);
    assert.equal(areEditorControlsLocked(adoptEditorExportStatus(status("one", phase))), true);
  }
  for (const phase of ["idle", "complete", "failed", "cancelled"] as const) {
    assert.equal(isEditorExportActive(status("one", phase)), false);
    assert.equal(areEditorControlsLocked(adoptEditorExportStatus(status("one", phase))), false);
  }
});

test("preparing advances through rendering verifying finalizing and complete", () => {
  let state = adoptEditorExportStatus(status("one", "preparing"));
  for (const phase of ["rendering", "verifying", "finalizing", "complete"] as const) {
    state = applyEditorExportEvent(state, status("one", phase));
    assert.equal(state.status?.phase, phase);
  }
});

test("stale progress for an older export ID is ignored", () => {
  const current = adoptEditorExportStatus(status("new", "rendering", 40));
  const next = applyEditorExportEvent(current, status("old", "complete", 100));
  assert.equal(next, current);
  assert.equal(next.status?.progressPercent, 40);
});

test("cancel request stays local until the matching cancelled event restores controls", () => {
  let state = adoptEditorExportStatus(status("one", "rendering", 20));
  state = requestEditorExportCancellation(state);
  assert.equal(state.cancellationRequested, true);
  assert.equal(areEditorControlsLocked(state), true);
  state = applyEditorExportEvent(state, status("one", "cancelled", 20));
  assert.equal(state.cancellationRequested, false);
  assert.equal(areEditorControlsLocked(state), false);
});

test("failure restores controls and a new export can be adopted explicitly", () => {
  let state = adoptEditorExportStatus(status("one", "rendering"));
  state = applyEditorExportEvent(state, status("one", "failed"));
  assert.equal(areEditorControlsLocked(state), false);
  state = adoptEditorExportStatus(status("two", "preparing"));
  assert.equal(state.status?.exportId, "two");
  assert.equal(areEditorControlsLocked(state), true);
});

test("export snapshot copies only immutable EDL and mixer decisions", () => {
  let session = createEditorSession(clip());
  session = splitAtPlayhead(withEditorPlayhead(session, 5));
  let mixer = createEditorMixer([game]);
  mixer = withEditorTrackGain(mixer, mixer.tracks[0].id, 175);
  const snapshot = snapshotEditorExport("clip-one", session.segments, mixer);
  session = withEditorPlayhead(session, 2);
  mixer = withEditorTrackGain(mixer, mixer.tracks[0].id, 20);
  assert.equal(snapshot.segments.length, 2);
  assert.equal(snapshot.mixer[0].gainPercent, 175);
  assert.deepEqual(Object.keys(snapshot.mixer[0]).sort(), ["gainPercent", "muted", "solo", "streamIndex"]);
});

test("idle state has no status and does not accept cancellation", () => {
  const idle = createEditorExportUiState();
  assert.equal(idle.status, null);
  assert.equal(requestEditorExportCancellation(idle), idle);
});
