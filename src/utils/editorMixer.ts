import type { ClipAudioTrack } from "../types/clips";
import {
  editedTimeToSourceTime,
  type EditorSegment,
} from "./editorSession.ts";

export const DEFAULT_EDITOR_GAIN_PERCENT = 100;
export const MAX_EDITOR_GAIN_PERCENT = 300;
export const AUDIO_DRIFT_THRESHOLD_MS = 50;

export type EditorAudioRole = "Game" | "VoiceChat" | "Microphone" | "Other" | "CombinedFallback" | "Unknown";
export type EditorAudioAvailability = "preparing" | "ready" | "unavailable" | "error";

export type EditorTrackMix = Readonly<{
  id: string;
  streamIndex: number;
  role: EditorAudioRole;
  title: string;
  gainPercent: number;
  muted: boolean;
  solo: boolean;
}>;

export type EditorAudioTrack = EditorTrackMix & Readonly<{
  availability: EditorAudioAvailability;
  errorMessage: string | null;
}>;

export type EditorMixerState = Readonly<{
  tracks: readonly EditorAudioTrack[];
}>;

export type EditorMediaSyncPlan = Readonly<{
  editedTimeUs: number;
  sourceTimeUs: number;
  segmentId: string;
  videoSourceTimeUs: number;
  audioSourceTimes: Readonly<Record<string, number>>;
}>;

function isCombinedTrack(track: ClipAudioTrack) {
  return track.role.trim().toLowerCase() === "combined"
    || track.title?.trim().toLowerCase() === "combined";
}

function roleForTrack(track: ClipAudioTrack): EditorAudioRole {
  if (isCombinedTrack(track)) return "CombinedFallback";
  switch (track.role) {
    case "Game":
    case "VoiceChat":
    case "Microphone":
    case "Other":
      return track.role;
    default:
      return "Unknown";
  }
}

function titleForTrack(track: ClipAudioTrack, role: EditorAudioRole) {
  switch (role) {
    case "VoiceChat": return "Voice Chat";
    case "Microphone": return "Microphone";
    case "CombinedFallback": return "Combined";
    case "Unknown": return track.title ?? track.handlerName ?? `Audio ${track.streamIndex}`;
    default: return role;
  }
}

function clampGainPercent(value: number) {
  if (!Number.isFinite(value)) return DEFAULT_EDITOR_GAIN_PERCENT;
  return Math.min(Math.max(Math.round(value), 0), MAX_EDITOR_GAIN_PERCENT);
}

function updateTrack(
  mixer: EditorMixerState,
  trackId: string,
  update: (track: EditorAudioTrack) => EditorAudioTrack,
): EditorMixerState {
  if (!mixer.tracks.some((track) => track.id === trackId)) return mixer;
  return {
    tracks: mixer.tracks.map((track) => track.id === trackId ? update(track) : track),
  };
}

export function createEditorMixer(audioTracks: readonly ClipAudioTrack[]): EditorMixerState {
  const independent = audioTracks.filter((track) => !isCombinedTrack(track));
  const selected = independent.length > 0
    ? independent
    : audioTracks.filter(isCombinedTrack).slice(0, 1);
  return {
    tracks: selected.map((track) => {
      const role = roleForTrack(track);
      return {
        id: `stream-${track.streamIndex}`,
        streamIndex: track.streamIndex,
        role,
        title: titleForTrack(track, role),
        gainPercent: DEFAULT_EDITOR_GAIN_PERCENT,
        muted: false,
        solo: false,
        availability: "preparing",
        errorMessage: null,
      };
    }),
  };
}

export function withEditorTrackGain(mixer: EditorMixerState, trackId: string, gainPercent: number) {
  return updateTrack(mixer, trackId, (track) => ({ ...track, gainPercent: clampGainPercent(gainPercent) }));
}

export function toggleEditorTrackMute(mixer: EditorMixerState, trackId: string) {
  return updateTrack(mixer, trackId, (track) => ({ ...track, muted: !track.muted }));
}

export function toggleEditorTrackSolo(mixer: EditorMixerState, trackId: string) {
  return updateTrack(mixer, trackId, (track) => ({ ...track, solo: !track.solo }));
}

export function withEditorTrackAvailability(
  mixer: EditorMixerState,
  trackId: string,
  availability: EditorAudioAvailability,
  errorMessage: string | null = null,
) {
  return updateTrack(mixer, trackId, (track) => ({ ...track, availability, errorMessage }));
}

export function resetEditorAudio(mixer: EditorMixerState): EditorMixerState {
  return {
    tracks: mixer.tracks.map((track) => ({
      ...track,
      gainPercent: DEFAULT_EDITOR_GAIN_PERCENT,
      muted: false,
      solo: false,
    })),
  };
}

export function isTrackAudible(track: EditorTrackMix, allTracks: readonly EditorTrackMix[]) {
  if (track.muted) return false;
  const anySolo = allTracks.some((candidate) => candidate.solo);
  return !anySolo || track.solo;
}

export function effectiveTrackGain(track: EditorTrackMix, allTracks: readonly EditorTrackMix[]) {
  return isTrackAudible(track, allTracks) ? track.gainPercent / 100 : 0;
}

export function isEditorMixerDirty(mixer: EditorMixerState) {
  return mixer.tracks.some((track) => track.gainPercent !== DEFAULT_EDITOR_GAIN_PERCENT || track.muted || track.solo);
}

export function isEditorDirty(edlDirty: boolean, mixer: EditorMixerState) {
  return edlDirty || isEditorMixerDirty(mixer);
}

export function editorMediaSyncPlan(
  segments: readonly EditorSegment[],
  editedTimeUs: number,
  audioTrackIds: readonly string[],
): EditorMediaSyncPlan | null {
  const mapping = editedTimeToSourceTime(segments, editedTimeUs);
  if (!mapping) return null;
  return {
    editedTimeUs: mapping.editedTimeUs,
    sourceTimeUs: mapping.sourceTimeUs,
    segmentId: mapping.segmentId,
    videoSourceTimeUs: mapping.sourceTimeUs,
    audioSourceTimes: Object.fromEntries(audioTrackIds.map((trackId) => [trackId, mapping.sourceTimeUs])),
  };
}

export function cutBoundarySyncPlan(nextSourceStartUs: number, audioTrackIds: readonly string[]) {
  const sourceTimeUs = Math.max(0, Math.round(Number.isFinite(nextSourceStartUs) ? nextSourceStartUs : 0));
  return {
    videoSourceTimeUs: sourceTimeUs,
    audioSourceTimes: Object.fromEntries(audioTrackIds.map((trackId) => [trackId, sourceTimeUs])),
  };
}

export function audioDriftCorrectionPlan(
  videoTimeSeconds: number,
  audioTimeSeconds: number,
  thresholdMs = AUDIO_DRIFT_THRESHOLD_MS,
) {
  if (!Number.isFinite(videoTimeSeconds) || !Number.isFinite(audioTimeSeconds)) {
    return { driftMs: 0, shouldCorrect: false, correctedTimeSeconds: null };
  }
  const driftMs = Math.abs(audioTimeSeconds - videoTimeSeconds) * 1_000;
  return {
    driftMs,
    shouldCorrect: driftMs > Math.max(0, thresholdMs),
    correctedTimeSeconds: driftMs > Math.max(0, thresholdMs) ? videoTimeSeconds : null,
  };
}

export function editorTransportPlan(
  action: "play" | "pause",
  sourceTimeSeconds: number,
  audioTrackIds: readonly string[],
) {
  const safeSourceTime = Number.isFinite(sourceTimeSeconds) ? Math.max(0, sourceTimeSeconds) : 0;
  return {
    action,
    videoSourceTimeSeconds: safeSourceTime,
    audio: Object.fromEntries(audioTrackIds.map((trackId) => [trackId, {
      action,
      sourceTimeSeconds: safeSourceTime,
    }])),
  };
}
