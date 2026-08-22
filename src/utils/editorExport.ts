import type {
  EditorExportRequest,
  EditorExportStatus,
} from "../types/clips.ts";
import type { EditorMixerState } from "./editorMixer.ts";
import type { EditorSegment } from "./editorSession.ts";

export type EditorExportUiState = Readonly<{
  status: EditorExportStatus | null;
  cancellationRequested: boolean;
}>;

export function createEditorExportUiState(): EditorExportUiState {
  return { status: null, cancellationRequested: false };
}

export function snapshotEditorExport(
  clipId: string,
  segments: readonly EditorSegment[],
  mixer: EditorMixerState,
): EditorExportRequest {
  return {
    clipId,
    segments: segments.map((segment) => ({
      id: segment.id,
      sourceStartUs: segment.sourceStartUs,
      sourceEndUs: segment.sourceEndUs,
    })),
    mixer: mixer.tracks.map((track) => ({
      streamIndex: track.streamIndex,
      gainPercent: track.gainPercent,
      muted: track.muted,
      solo: track.solo,
    })),
  };
}

export function adoptEditorExportStatus(status: EditorExportStatus): EditorExportUiState {
  return { status, cancellationRequested: false };
}

export function applyEditorExportEvent(
  current: EditorExportUiState,
  incoming: EditorExportStatus,
): EditorExportUiState {
  const activeExportId = current.status?.exportId;
  if (activeExportId && incoming.exportId !== activeExportId) return current;
  return {
    status: incoming,
    cancellationRequested: isEditorExportActive(incoming) ? current.cancellationRequested : false,
  };
}

export function requestEditorExportCancellation(state: EditorExportUiState): EditorExportUiState {
  return isEditorExportActive(state.status)
    ? { ...state, cancellationRequested: true }
    : state;
}

export function isEditorExportActive(status: EditorExportStatus | null) {
  return status !== null && ["preparing", "rendering", "verifying", "finalizing"].includes(status.phase);
}

export function areEditorControlsLocked(state: EditorExportUiState) {
  return isEditorExportActive(state.status);
}
