export type ClipAudioTrack = {
  streamIndex: number;
  role: string;
  title: string | null;
  handlerName: string | null;
  codec: string;
  profile: string | null;
  sampleRate: number | null;
  channels: number | null;
  bitrateBps: number | null;
  isDefault: boolean;
};

export type ClipListItem = {
  id: string;
  filePath: string;
  filename: string;
  displayName: string;
  createdAtMs: number;
  fileModifiedAtMs: number;
  fileSizeBytes: number;
  duration100ns: number;
  requestedDurationSeconds: number | null;
  width: number;
  height: number;
  fpsNumerator: number;
  fpsDenominator: number;
  videoCodec: string;
  videoProfile: string | null;
  videoBitrateBps: number | null;
  totalBitrateBps: number | null;
  captureTargetLabel: string | null;
  captureTargetType: string | null;
  favorite: boolean;
  importedExistingFile: boolean;
  audioStreamCount: number;
  defaultAudioStreamTitle: string | null;
  audioTracks: ClipAudioTrack[];
};

export type ReconciliationTelemetry = {
  scannedFiles: number;
  unchanged: number;
  added: number;
  updated: number;
  removed: number;
  failed: number;
  durationMs: number;
  errors: string[];
};

export type LibraryTelemetry = {
  databasePath: string;
  schemaVersion: number;
  indexedClipCount: number;
  reconciliationRunning: boolean;
  lastReconciliation: ReconciliationTelemetry | null;
  lastListQueryDurationMs: number | null;
  newestSavedClipId: string | null;
  newestSavedClipIndexed: boolean | null;
  newestSavedClipInsertionMs: number | null;
};

export type ClipListResponse = {
  success: boolean;
  clips: ClipListItem[];
  totalCount: number;
  telemetry: LibraryTelemetry;
  errorMessage: string | null;
};

export type ReconcileResponse = {
  success: boolean;
  result: ReconciliationTelemetry | null;
  telemetry: LibraryTelemetry;
  errorMessage: string | null;
};

export type ClipMutationResponse = {
  success: boolean;
  clip: ClipListItem | null;
  errorMessage: string | null;
};

export type ClipActionResponse = {
  success: boolean;
  errorMessage: string | null;
};

export type CacheArtifactState = "missing" | "preparing" | "ready" | "error";

export type CacheArtifactStatus = {
  state: CacheArtifactState;
  filePath: string | null;
  generationDurationMs: number | null;
  fileSizeBytes: number | null;
  bitrateBps: number | null;
  errorMessage: string | null;
};

export type ClipPlaybackInfo = {
  clipId: string;
  displayName: string;
  masterPath: string;
  masterCodec: string;
  width: number;
  height: number;
  duration100ns: number;
  audioTracks: ClipAudioTrack[];
  cacheRoot: string;
  preview: CacheArtifactStatus;
  thumbnail: CacheArtifactStatus;
};

export type ClipPlaybackInfoResponse = {
  success: boolean;
  info: ClipPlaybackInfo | null;
  errorMessage: string | null;
};

export type PrepareClipMediaResponse = {
  success: boolean;
  artifact: CacheArtifactStatus;
  playbackSource: string | null;
  selectedAudioRole: string | null;
  restoreAtSeconds: number;
  resumePlaying: boolean;
  errorMessage: string | null;
};

export type EditorExportPhase = "idle" | "preparing" | "rendering" | "verifying" | "finalizing" | "complete" | "failed" | "cancelled";

export type EditorExportSegment = {
  id: string;
  sourceStartUs: number;
  sourceEndUs: number;
};

export type EditorExportTrackMix = {
  streamIndex: number;
  gainPercent: number;
  muted: boolean;
  solo: boolean;
};

export type EditorExportRequest = {
  clipId: string;
  segments: EditorExportSegment[];
  mixer: EditorExportTrackMix[];
};

export type EditorExportStatus = {
  exportId: string | null;
  sourceClipId: string | null;
  phase: EditorExportPhase;
  progressPercent: number;
  encodedTimeUs: number;
  totalTimeUs: number;
  encoder: string | null;
  encoderHardware: boolean | null;
  encoderSettings: string | null;
  attemptedEncoders: string[];
  filterPlan: string | null;
  plannedDurationUs: number | null;
  verifiedDurationUs: number | null;
  outputClip: ClipListItem | null;
  outputDisplayName: string | null;
  indexingWarning: string | null;
  errorMessage: string | null;
  diagnostics: string[];
};

export type EditorExportCommandResponse = {
  success: boolean;
  status: EditorExportStatus;
  errorMessage: string | null;
};

export type ClipSortOrder = "newestFirst" | "oldestFirst" | "nameAscending";

export function audioLabel(track: ClipAudioTrack) {
  if (track.role === "VoiceChat") return "Voice Chat";
  if (track.role === "Microphone") return "Mic";
  if (track.role !== "Unknown") return track.role;
  return track.title ?? track.handlerName ?? `Audio ${track.streamIndex}`;
}

export function formatDuration100ns(value: number) {
  return formatTime(Math.max(0, value / 10_000_000));
}

export function formatTime(seconds: number) {
  const safe = Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
  return `${Math.floor(safe / 60)}:${String(Math.floor(safe % 60)).padStart(2, "0")}`;
}

export function formatFps(numerator: number, denominator: number) {
  if (denominator <= 0) return "0";
  const value = numerator / denominator;
  return Number.isInteger(value) ? value.toFixed(0) : value.toFixed(2);
}

export function formatBytes(bytes: number) {
  if (bytes < 1024) return `${bytes} B`;
  if (bytes < 1024 ** 2) return `${(bytes / 1024).toFixed(1)} KB`;
  if (bytes < 1024 ** 3) return `${(bytes / 1024 ** 2).toFixed(1)} MB`;
  return `${(bytes / 1024 ** 3).toFixed(2)} GB`;
}

export function errorMessage(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause);
}
