import type { UiPreferences } from "../types/clips";

export type DetectedReplayState = "detected" | "starting" | "replayReady" | "captureFailed" | "replayStopped";

export function detectedReplayLabel(state: DetectedReplayState): string {
  switch (state) {
    case "detected": return "Detected";
    case "starting": return "Starting Replay…";
    case "replayReady": return "Replay Ready";
    case "captureFailed": return "Capture failed";
    case "replayStopped": return "Replay stopped";
  }
}

export function showCandidateApprovalControls(mode: UiPreferences["gameDetectionMode"]): boolean {
  return mode === "approvedGamesOnly";
}
