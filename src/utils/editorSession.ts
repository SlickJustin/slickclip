import type { ClipAudioTrack, ClipListItem } from "../types/clips";
import { clampMediaTime } from "./playerControls.ts";

export type EditorPlaybackState = "loading" | "paused" | "playing" | "ended" | "error";

export type EditorSource = Readonly<{
  clipId: string;
  filePath: string;
  filename: string;
  displayName: string;
  durationSeconds: number;
  width: number;
  height: number;
  videoCodec: string;
  audioTracks: readonly ClipAudioTrack[];
}>;

export type EditorRange = Readonly<{
  startSeconds: number;
  endSeconds: number;
}>;

export type EditorSession = Readonly<{
  source: EditorSource;
  playheadSeconds: number;
  playbackState: EditorPlaybackState;
  editableRange: EditorRange;
}>;

function safeDuration(value: number) {
  return Number.isFinite(value) ? Math.max(0, value) : 0;
}

export function createEditorSession(clip: ClipListItem): EditorSession {
  const durationSeconds = safeDuration(clip.duration100ns / 10_000_000);
  return {
    source: {
      clipId: clip.id,
      filePath: clip.filePath,
      filename: clip.filename,
      displayName: clip.displayName,
      durationSeconds,
      width: clip.width,
      height: clip.height,
      videoCodec: clip.videoCodec,
      audioTracks: clip.audioTracks.map((track) => ({ ...track })),
    },
    playheadSeconds: 0,
    playbackState: "loading",
    editableRange: { startSeconds: 0, endSeconds: durationSeconds },
  };
}

export function resetEditorSession(clip: ClipListItem) {
  return createEditorSession(clip);
}

export function withEditorDuration(session: EditorSession, requestedDuration: number): EditorSession {
  const durationSeconds = safeDuration(requestedDuration);
  return {
    ...session,
    source: { ...session.source, durationSeconds },
    playheadSeconds: clampMediaTime(session.playheadSeconds, durationSeconds),
    editableRange: { startSeconds: 0, endSeconds: durationSeconds },
  };
}

export function withEditorPlayhead(session: EditorSession, requestedTime: number): EditorSession {
  return {
    ...session,
    playheadSeconds: clampMediaTime(requestedTime, session.source.durationSeconds),
  };
}

export function withEditorPlaybackState(session: EditorSession, playbackState: EditorPlaybackState): EditorSession {
  return { ...session, playbackState };
}

export function timelinePositionToSeconds(position: number, width: number, duration: number) {
  if (!Number.isFinite(position) || !Number.isFinite(width) || width <= 0) return 0;
  const fraction = Math.min(Math.max(position / width, 0), 1);
  return fraction * safeDuration(duration);
}

export function timelineTickTimes(duration: number, intervalCount = 4) {
  const safe = safeDuration(duration);
  const count = Number.isInteger(intervalCount) ? Math.max(1, intervalCount) : 4;
  return Array.from({ length: count + 1 }, (_, index) => safe * index / count);
}

export function formatEditorTime(seconds: number) {
  const safe = safeDuration(seconds);
  const minutes = Math.floor(safe / 60);
  const wholeSeconds = Math.floor(safe % 60);
  const hundredths = Math.floor((safe - Math.floor(safe)) * 100);
  return `${minutes}:${String(wholeSeconds).padStart(2, "0")}.${String(hundredths).padStart(2, "0")}`;
}
