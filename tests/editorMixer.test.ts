import assert from "node:assert/strict";
import test from "node:test";
import type { ClipAudioTrack, ClipListItem } from "../src/types/clips.ts";
import {
  AUDIO_DRIFT_THRESHOLD_MS,
  audioDriftCorrectionPlan,
  createEditorMixer,
  cutBoundarySyncPlan,
  editorMediaSyncPlan,
  editorTransportPlan,
  effectiveTrackGain,
  isEditorDirty,
  isEditorMixerDirty,
  isTrackAudible,
  resetEditorAudio,
  toggleEditorTrackMute,
  toggleEditorTrackSolo,
  withEditorTrackAvailability,
  withEditorTrackGain,
} from "../src/utils/editorMixer.ts";
import {
  createEditorSession,
  deleteSelectedSegment,
  secondsToMicroseconds,
  selectEditorSegment,
  splitAtPlayhead,
  totalEditedDurationUs,
  trimEditorSegment,
  withEditorPlayhead,
} from "../src/utils/editorSession.ts";

function audioTrack(streamIndex: number, role: string, title: string | null = role): ClipAudioTrack {
  return {
    streamIndex,
    role,
    title,
    handlerName: null,
    codec: "aac",
    profile: "LC",
    sampleRate: 48_000,
    channels: 2,
    bitrateBps: 160_000,
    isDefault: role === "Combined",
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
    audioStreamCount: 4,
    defaultAudioStreamTitle: "Combined",
    audioTracks: [],
  };
}

const metadata = [
  audioTrack(1, "Combined"),
  audioTrack(2, "Game"),
  audioTrack(3, "VoiceChat"),
  audioTrack(4, "Microphone"),
];

test("independent stems initialize at unity and exclude Combined", () => {
  const mixer = createEditorMixer(metadata);
  assert.deepEqual(mixer.tracks.map((track) => track.role), ["Game", "VoiceChat", "Microphone"]);
  assert.ok(mixer.tracks.every((track) => track.gainPercent === 100 && !track.muted && !track.solo));
  assert.ok(mixer.tracks.every((track) => track.availability === "preparing"));
});

test("Combined is exposed only as a legacy fallback", () => {
  const mixer = createEditorMixer([audioTrack(1, "Combined")]);
  assert.equal(mixer.tracks.length, 1);
  assert.equal(mixer.tracks[0].role, "CombinedFallback");
  assert.equal(mixer.tracks[0].title, "Combined");
});

test("missing roles are not manufactured and unknown titles remain safe", () => {
  const mixer = createEditorMixer([audioTrack(7, "Unknown", "Commentary")]);
  assert.deepEqual(mixer.tracks.map((track) => [track.role, track.title]), [["Unknown", "Commentary"]]);
});

test("gain supports exact 0, 100, and 300 percent", () => {
  let mixer = createEditorMixer(metadata);
  const id = mixer.tracks[0].id;
  mixer = withEditorTrackGain(mixer, id, 0);
  assert.equal(mixer.tracks[0].gainPercent, 0);
  mixer = withEditorTrackGain(mixer, id, 100);
  assert.equal(mixer.tracks[0].gainPercent, 100);
  mixer = withEditorTrackGain(mixer, id, 300);
  assert.equal(mixer.tracks[0].gainPercent, 300);
});

test("gain clamps outside the serializable 0 to 300 range", () => {
  let mixer = createEditorMixer(metadata);
  const id = mixer.tracks[0].id;
  mixer = withEditorTrackGain(mixer, id, -80);
  assert.equal(mixer.tracks[0].gainPercent, 0);
  mixer = withEditorTrackGain(mixer, id, 900);
  assert.equal(mixer.tracks[0].gainPercent, 300);
});

test("without solos every non-muted track is audible", () => {
  let mixer = createEditorMixer(metadata);
  mixer = toggleEditorTrackMute(mixer, mixer.tracks[0].id);
  assert.deepEqual(mixer.tracks.map((track) => isTrackAudible(track, mixer.tracks)), [false, true, true]);
  assert.equal(mixer.tracks[0].gainPercent, 100);
});

test("one solo excludes every non-solo track", () => {
  let mixer = createEditorMixer(metadata);
  mixer = toggleEditorTrackSolo(mixer, mixer.tracks[1].id);
  assert.deepEqual(mixer.tracks.map((track) => isTrackAudible(track, mixer.tracks)), [false, true, false]);
});

test("multiple solos are audible together", () => {
  let mixer = createEditorMixer(metadata);
  mixer = toggleEditorTrackSolo(mixer, mixer.tracks[0].id);
  mixer = toggleEditorTrackSolo(mixer, mixer.tracks[2].id);
  assert.deepEqual(mixer.tracks.map((track) => isTrackAudible(track, mixer.tracks)), [true, false, true]);
});

test("mute overrides solo without destroying gain", () => {
  let mixer = createEditorMixer(metadata);
  const id = mixer.tracks[0].id;
  mixer = withEditorTrackGain(mixer, id, 250);
  mixer = toggleEditorTrackSolo(mixer, id);
  mixer = toggleEditorTrackMute(mixer, id);
  assert.equal(isTrackAudible(mixer.tracks[0], mixer.tracks), false);
  assert.equal(effectiveTrackGain(mixer.tracks[0], mixer.tracks), 0);
  assert.equal(mixer.tracks[0].gainPercent, 250);
});

test("effective gain uses the exact displayed linear percentage", () => {
  let mixer = createEditorMixer(metadata);
  mixer = withEditorTrackGain(mixer, mixer.tracks[0].id, 175);
  assert.equal(effectiveTrackGain(mixer.tracks[0], mixer.tracks), 1.75);
});

test("Reset Audio restores gains, mutes, and solos but preserves availability", () => {
  let mixer = createEditorMixer(metadata);
  mixer = withEditorTrackGain(mixer, mixer.tracks[0].id, 250);
  mixer = toggleEditorTrackMute(mixer, mixer.tracks[1].id);
  mixer = toggleEditorTrackSolo(mixer, mixer.tracks[2].id);
  mixer = withEditorTrackAvailability(mixer, mixer.tracks[0].id, "ready");
  const reset = resetEditorAudio(mixer);
  assert.ok(reset.tracks.every((track) => track.gainPercent === 100 && !track.muted && !track.solo));
  assert.equal(reset.tracks[0].availability, "ready");
});

test("gain, mute, and solo independently make mixer decisions dirty", () => {
  const clean = createEditorMixer(metadata);
  assert.equal(isEditorMixerDirty(clean), false);
  assert.equal(isEditorMixerDirty(withEditorTrackGain(clean, clean.tracks[0].id, 101)), true);
  assert.equal(isEditorMixerDirty(toggleEditorTrackMute(clean, clean.tracks[0].id)), true);
  assert.equal(isEditorMixerDirty(toggleEditorTrackSolo(clean, clean.tracks[0].id)), true);
});

test("dirty clears at defaults unless the EDL remains edited", () => {
  const clean = createEditorMixer(metadata);
  const changed = withEditorTrackGain(clean, clean.tracks[0].id, 200);
  assert.equal(isEditorDirty(false, changed), true);
  assert.equal(isEditorDirty(false, resetEditorAudio(changed)), false);
  assert.equal(isEditorDirty(true, resetEditorAudio(changed)), true);
});

test("edited seek maps video and every audio follower to one source time", () => {
  let session = createEditorSession(clip());
  session = splitAtPlayhead(withEditorPlayhead(session, 5));
  session = splitAtPlayhead(withEditorPlayhead(session, 10));
  session = deleteSelectedSegment(selectEditorSegment(session, session.segments[1].id));
  const plan = editorMediaSyncPlan(session.segments, secondsToMicroseconds(7), ["game", "mic"]);
  assert.equal(plan?.sourceTimeUs, secondsToMicroseconds(12));
  assert.equal(plan?.videoSourceTimeUs, secondsToMicroseconds(12));
  assert.deepEqual(plan?.audioSourceTimes, { game: secondsToMicroseconds(12), mic: secondsToMicroseconds(12) });
});

test("cut boundary sends video and all tracks from source 5 to source 10", () => {
  const plan = cutBoundarySyncPlan(secondsToMicroseconds(10), ["game", "voice", "mic"]);
  assert.equal(plan.videoSourceTimeUs, secondsToMicroseconds(10));
  assert.deepEqual(Object.values(plan.audioSourceTimes), Array(3).fill(secondsToMicroseconds(10)));
});

test("final edited endpoint maps to the final source endpoint", () => {
  let session = createEditorSession(clip());
  session = trimEditorSegment(session, session.segments[0].id, "end", secondsToMicroseconds(24));
  const plan = editorMediaSyncPlan(session.segments, totalEditedDurationUs(session.segments), ["game"]);
  assert.equal(plan?.sourceTimeUs, secondsToMicroseconds(24));
});

test("audio timing automatically follows EDL delete and trim decisions", () => {
  let session = createEditorSession(clip());
  const mixer = createEditorMixer(metadata);
  session = splitAtPlayhead(withEditorPlayhead(session, 8));
  session = deleteSelectedSegment(session);
  session = trimEditorSegment(session, session.segments[0].id, "start", secondsToMicroseconds(2));
  const plan = editorMediaSyncPlan(session.segments, 0, mixer.tracks.map((track) => track.id));
  assert.equal(plan?.sourceTimeUs, secondsToMicroseconds(2));
  assert.equal(mixer.tracks.length, 3);
  assert.equal(isEditorMixerDirty(mixer), false);
});

test("drift below threshold is observed without correction", () => {
  const plan = audioDriftCorrectionPlan(10, 10.04);
  assert.equal(AUDIO_DRIFT_THRESHOLD_MS, 50);
  assert.equal(Math.round(plan.driftMs), 40);
  assert.equal(plan.shouldCorrect, false);
  assert.equal(plan.correctedTimeSeconds, null);
});

test("drift above threshold corrects to the authoritative video clock", () => {
  const plan = audioDriftCorrectionPlan(10, 10.061);
  assert.equal(Math.round(plan.driftMs), 61);
  assert.equal(plan.shouldCorrect, true);
  assert.equal(plan.correctedTimeSeconds, 10);
});

test("play and pause plans target every follower at the video source time", () => {
  assert.deepEqual(editorTransportPlan("play", 12.5, ["game", "mic"]), {
    action: "play",
    videoSourceTimeSeconds: 12.5,
    audio: {
      game: { action: "play", sourceTimeSeconds: 12.5 },
      mic: { action: "play", sourceTimeSeconds: 12.5 },
    },
  });
  assert.equal(editorTransportPlan("pause", 7, ["game"]).audio.game.action, "pause");
});
