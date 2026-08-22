import type { ClipAudioTrack, ClipListItem } from "../types/clips";

export const MICROSECONDS_PER_SECOND = 1_000_000;
export const MIN_SEGMENT_DURATION_US = 100_000;
export const EDITOR_HISTORY_LIMIT = 50;

export type EditorPlaybackState = "loading" | "paused" | "playing" | "ended" | "error";
export type EditorTrimEdge = "start" | "end";

export type EditorSegment = Readonly<{
  id: string;
  sourceStartUs: number;
  sourceEndUs: number;
}>;

export type EditorSource = Readonly<{
  clipId: string;
  filePath: string;
  filename: string;
  displayName: string;
  durationUs: number;
  width: number;
  height: number;
  videoCodec: string;
  audioTracks: readonly ClipAudioTrack[];
}>;

export type EditorHistoryState = Readonly<{
  segments: readonly EditorSegment[];
}>;

export type EditorSession = Readonly<{
  source: EditorSource;
  segments: readonly EditorSegment[];
  selectedSegmentId: string | null;
  playheadUs: number;
  playbackState: EditorPlaybackState;
  undoStack: readonly EditorHistoryState[];
  redoStack: readonly EditorHistoryState[];
  nextSegmentOrdinal: number;
  dirty: boolean;
}>;

export type EditorSegmentOffset = Readonly<{
  segment: EditorSegment;
  index: number;
  editedStartUs: number;
  editedEndUs: number;
}>;

export type EditorTimeMapping = Readonly<{
  segmentId: string;
  segmentIndex: number;
  editedTimeUs: number;
  sourceTimeUs: number;
  segmentEditedStartUs: number;
  segmentEditedEndUs: number;
}>;

function safeIntegerUs(value: number) {
  if (!Number.isFinite(value)) return 0;
  return Math.max(0, Math.round(value));
}

function cloneSegments(segments: readonly EditorSegment[]) {
  return segments.map((segment) => ({ ...segment }));
}

function initialSegmentId(clipId: string) {
  return `${clipId}:segment:0`;
}

function initialSegments(clipId: string, durationUs: number): readonly EditorSegment[] {
  return durationUs > 0
    ? [{ id: initialSegmentId(clipId), sourceStartUs: 0, sourceEndUs: durationUs }]
    : [];
}

function reconcileSegmentsToDuration(
  segments: readonly EditorSegment[],
  previousDurationUs: number,
  durationUs: number,
) {
  const reconciled = segments.map((segment) => ({
    ...segment,
    sourceEndUs: segment.sourceEndUs === previousDurationUs || segment.sourceEndUs > durationUs
      ? durationUs
      : segment.sourceEndUs,
  }));
  return validateEditorSegments(reconciled, durationUs)
    && reconciled.every((segment) => segmentDurationUs(segment) >= MIN_SEGMENT_DURATION_US)
    ? reconciled
    : null;
}

function reconcileHistoryToDuration(
  history: readonly EditorHistoryState[],
  previousDurationUs: number,
  durationUs: number,
) {
  return history.flatMap((state) => {
    const segments = reconcileSegmentsToDuration(state.segments, previousDurationUs, durationUs);
    return segments ? [{ segments }] : [];
  });
}

function historyState(segments: readonly EditorSegment[]): EditorHistoryState {
  return { segments: cloneSegments(segments) };
}

function boundedHistory(history: readonly EditorHistoryState[]) {
  return history.slice(-EDITOR_HISTORY_LIMIT);
}

function segmentsEqual(left: readonly EditorSegment[], right: readonly EditorSegment[]) {
  return left.length === right.length && left.every((segment, index) => {
    const other = right[index];
    return segment.id === other.id
      && segment.sourceStartUs === other.sourceStartUs
      && segment.sourceEndUs === other.sourceEndUs;
  });
}

function isOriginalTimeline(segments: readonly EditorSegment[], sourceDurationUs: number) {
  return segments.length === 1
    && segments[0].sourceStartUs === 0
    && segments[0].sourceEndUs === sourceDurationUs;
}

function selectedSegmentAfterRestore(
  segments: readonly EditorSegment[],
  preferredId: string | null,
  playheadUs: number,
) {
  if (preferredId && segments.some((segment) => segment.id === preferredId)) return preferredId;
  return editedTimeToSourceTime(segments, playheadUs)?.segmentId ?? segments[0]?.id ?? null;
}

function nearestEditedTimeForSource(segments: readonly EditorSegment[], sourceTimeUs: number) {
  const exact = sourceTimeToEditedTime(segments, sourceTimeUs);
  if (exact !== null) return exact;

  let nearestTime = 0;
  let nearestDistance = Number.POSITIVE_INFINITY;
  for (const offset of segmentEditedOffsets(segments)) {
    const startDistance = Math.abs(sourceTimeUs - offset.segment.sourceStartUs);
    if (startDistance < nearestDistance) {
      nearestDistance = startDistance;
      nearestTime = offset.editedStartUs;
    }
    const endDistance = Math.abs(sourceTimeUs - offset.segment.sourceEndUs);
    if (endDistance < nearestDistance) {
      nearestDistance = endDistance;
      nearestTime = offset.editedEndUs;
    }
  }
  return nearestTime;
}

function remapPlayhead(
  previousSegments: readonly EditorSegment[],
  nextSegments: readonly EditorSegment[],
  previousPlayheadUs: number,
) {
  const previous = editedTimeToSourceTime(previousSegments, previousPlayheadUs);
  if (!previous) return 0;
  return nearestEditedTimeForSource(nextSegments, previous.sourceTimeUs);
}

function restoreTimeline(
  session: EditorSession,
  segments: readonly EditorSegment[],
  undoStack: readonly EditorHistoryState[],
  redoStack: readonly EditorHistoryState[],
): EditorSession {
  if (!validateEditorSegments(segments, session.source.durationUs)) return session;
  const playheadUs = remapPlayhead(session.segments, segments, session.playheadUs);
  return {
    ...session,
    segments: cloneSegments(segments),
    selectedSegmentId: selectedSegmentAfterRestore(segments, session.selectedSegmentId, playheadUs),
    playheadUs,
    playbackState: session.playbackState === "playing" ? "paused" : session.playbackState,
    undoStack,
    redoStack,
    dirty: !isOriginalTimeline(segments, session.source.durationUs),
  };
}

function commitTimeline(
  session: EditorSession,
  segments: readonly EditorSegment[],
  selectedSegmentId: string | null,
  nextSegmentOrdinal = session.nextSegmentOrdinal,
): EditorSession {
  if (!validateEditorSegments(segments, session.source.durationUs) || segmentsEqual(session.segments, segments)) return session;
  const playheadUs = remapPlayhead(session.segments, segments, session.playheadUs);
  return {
    ...session,
    segments: cloneSegments(segments),
    selectedSegmentId: selectedSegmentAfterRestore(segments, selectedSegmentId, playheadUs),
    playheadUs,
    playbackState: session.playbackState === "playing" ? "paused" : session.playbackState,
    undoStack: boundedHistory([...session.undoStack, historyState(session.segments)]),
    redoStack: [],
    nextSegmentOrdinal,
    dirty: !isOriginalTimeline(segments, session.source.durationUs),
  };
}

export function secondsToMicroseconds(seconds: number) {
  return safeIntegerUs(seconds * MICROSECONDS_PER_SECOND);
}

export function microsecondsToSeconds(microseconds: number) {
  return safeIntegerUs(microseconds) / MICROSECONDS_PER_SECOND;
}

export function segmentDurationUs(segment: EditorSegment) {
  return Math.max(0, segment.sourceEndUs - segment.sourceStartUs);
}

export function totalEditedDurationUs(segments: readonly EditorSegment[]) {
  return segments.reduce((total, segment) => total + segmentDurationUs(segment), 0);
}

export function segmentEditedOffsets(segments: readonly EditorSegment[]): readonly EditorSegmentOffset[] {
  let cursorUs = 0;
  return segments.map((segment, index) => {
    const editedStartUs = cursorUs;
    cursorUs += segmentDurationUs(segment);
    return { segment, index, editedStartUs, editedEndUs: cursorUs };
  });
}

export function validateEditorSegments(segments: readonly EditorSegment[], sourceDurationUs: number) {
  const durationUs = safeIntegerUs(sourceDurationUs);
  if (segments.length === 0 || durationUs <= 0) return false;
  const ids = new Set<string>();
  let previousEndUs = 0;
  for (const segment of segments) {
    if (!segment.id || ids.has(segment.id)) return false;
    if (!Number.isSafeInteger(segment.sourceStartUs) || !Number.isSafeInteger(segment.sourceEndUs)) return false;
    if (segment.sourceStartUs < 0 || segment.sourceEndUs <= segment.sourceStartUs || segment.sourceEndUs > durationUs) return false;
    if (segment.sourceStartUs < previousEndUs) return false;
    ids.add(segment.id);
    previousEndUs = segment.sourceEndUs;
  }
  return true;
}

export function editedTimeToSourceTime(
  segments: readonly EditorSegment[],
  requestedEditedTimeUs: number,
): EditorTimeMapping | null {
  const offsets = segmentEditedOffsets(segments);
  const editedDurationUs = totalEditedDurationUs(segments);
  if (offsets.length === 0 || editedDurationUs <= 0) return null;
  const editedTimeUs = Math.min(safeIntegerUs(requestedEditedTimeUs), editedDurationUs);
  const lastIndex = offsets.length - 1;
  const offset = offsets.find((candidate) => editedTimeUs < candidate.editedEndUs || candidate.index === lastIndex) ?? offsets[lastIndex];
  const withinSegmentUs = Math.min(
    Math.max(editedTimeUs - offset.editedStartUs, 0),
    segmentDurationUs(offset.segment),
  );
  return {
    segmentId: offset.segment.id,
    segmentIndex: offset.index,
    editedTimeUs,
    sourceTimeUs: offset.segment.sourceStartUs + withinSegmentUs,
    segmentEditedStartUs: offset.editedStartUs,
    segmentEditedEndUs: offset.editedEndUs,
  };
}

export function segmentSourcePositionToEditedTime(
  segments: readonly EditorSegment[],
  segmentId: string,
  requestedSourceTimeUs: number,
) {
  const offset = segmentEditedOffsets(segments).find((candidate) => candidate.segment.id === segmentId);
  const sourceTimeUs = safeIntegerUs(requestedSourceTimeUs);
  if (!offset || sourceTimeUs < offset.segment.sourceStartUs || sourceTimeUs > offset.segment.sourceEndUs) return null;
  return offset.editedStartUs + sourceTimeUs - offset.segment.sourceStartUs;
}

export function sourceTimeToEditedTime(segments: readonly EditorSegment[], requestedSourceTimeUs: number) {
  const sourceTimeUs = safeIntegerUs(requestedSourceTimeUs);
  const offsets = segmentEditedOffsets(segments);
  for (const offset of offsets) {
    if (sourceTimeUs >= offset.segment.sourceStartUs && sourceTimeUs < offset.segment.sourceEndUs) {
      return offset.editedStartUs + sourceTimeUs - offset.segment.sourceStartUs;
    }
  }
  const last = offsets[offsets.length - 1];
  return last && sourceTimeUs === last.segment.sourceEndUs ? last.editedEndUs : null;
}

export function createEditorSession(clip: ClipListItem): EditorSession {
  const durationUs = safeIntegerUs(clip.duration100ns / 10);
  const segments = initialSegments(clip.id, durationUs);
  return {
    source: {
      clipId: clip.id,
      filePath: clip.filePath,
      filename: clip.filename,
      displayName: clip.displayName,
      durationUs,
      width: clip.width,
      height: clip.height,
      videoCodec: clip.videoCodec,
      audioTracks: clip.audioTracks.map((track) => ({ ...track })),
    },
    segments,
    selectedSegmentId: segments[0]?.id ?? null,
    playheadUs: 0,
    playbackState: "loading",
    undoStack: [],
    redoStack: [],
    nextSegmentOrdinal: 1,
    dirty: false,
  };
}

export function resetEditorSession(clip: ClipListItem) {
  return createEditorSession(clip);
}

export function withEditorDuration(session: EditorSession, requestedDurationSeconds: number): EditorSession {
  const durationUs = secondsToMicroseconds(requestedDurationSeconds);
  if (durationUs <= 0 || durationUs === session.source.durationUs) return session;
  if (!session.dirty && session.undoStack.length === 0 && session.redoStack.length === 0) {
    const segments = initialSegments(session.source.clipId, durationUs);
    return {
      ...session,
      source: { ...session.source, durationUs },
      segments,
      selectedSegmentId: segments[0]?.id ?? null,
      playheadUs: Math.min(session.playheadUs, durationUs),
      dirty: false,
    };
  }

  const segments = reconcileSegmentsToDuration(session.segments, session.source.durationUs, durationUs);
  if (!segments) return session;
  const playheadUs = remapPlayhead(session.segments, segments, session.playheadUs);
  return {
    ...session,
    source: { ...session.source, durationUs },
    segments,
    selectedSegmentId: selectedSegmentAfterRestore(segments, session.selectedSegmentId, playheadUs),
    playheadUs,
    playbackState: session.playbackState === "playing" ? "paused" : session.playbackState,
    undoStack: reconcileHistoryToDuration(session.undoStack, session.source.durationUs, durationUs),
    redoStack: reconcileHistoryToDuration(session.redoStack, session.source.durationUs, durationUs),
    dirty: !isOriginalTimeline(segments, durationUs),
  };
}

export function withEditorPlayhead(session: EditorSession, requestedEditedTimeSeconds: number): EditorSession {
  return withEditorPlayheadUs(session, secondsToMicroseconds(requestedEditedTimeSeconds));
}

export function withEditorPlayheadUs(session: EditorSession, requestedEditedTimeUs: number): EditorSession {
  return {
    ...session,
    playheadUs: Math.min(safeIntegerUs(requestedEditedTimeUs), totalEditedDurationUs(session.segments)),
  };
}

export function withEditorPlaybackState(session: EditorSession, playbackState: EditorPlaybackState): EditorSession {
  return { ...session, playbackState };
}

export function selectEditorSegment(session: EditorSession, segmentId: string): EditorSession {
  return session.segments.some((segment) => segment.id === segmentId)
    ? { ...session, selectedSegmentId: segmentId }
    : session;
}

export function previewTrimmedSegments(
  segments: readonly EditorSegment[],
  sourceDurationUs: number,
  segmentId: string,
  edge: EditorTrimEdge,
  requestedSourceTimeUs: number,
) {
  const index = segments.findIndex((segment) => segment.id === segmentId);
  if (index < 0) return segments;
  const segment = segments[index];
  if (segmentDurationUs(segment) < MIN_SEGMENT_DURATION_US) return segments;
  const requestedUs = safeIntegerUs(requestedSourceTimeUs);
  const nextSegments = cloneSegments(segments);
  if (edge === "start") {
    const minimumStartUs = index > 0 ? segments[index - 1].sourceEndUs : 0;
    const maximumStartUs = segment.sourceEndUs - MIN_SEGMENT_DURATION_US;
    nextSegments[index] = {
      ...segment,
      sourceStartUs: Math.min(Math.max(requestedUs, minimumStartUs), maximumStartUs),
    };
  } else {
    const minimumEndUs = segment.sourceStartUs + MIN_SEGMENT_DURATION_US;
    const maximumEndUs = index < segments.length - 1
      ? segments[index + 1].sourceStartUs
      : safeIntegerUs(sourceDurationUs);
    nextSegments[index] = {
      ...segment,
      sourceEndUs: Math.min(Math.max(requestedUs, minimumEndUs), maximumEndUs),
    };
  }
  return validateEditorSegments(nextSegments, sourceDurationUs) ? nextSegments : segments;
}

export function trimEditorSegment(
  session: EditorSession,
  segmentId: string,
  edge: EditorTrimEdge,
  requestedSourceTimeUs: number,
) {
  const segments = previewTrimmedSegments(
    session.segments,
    session.source.durationUs,
    segmentId,
    edge,
    requestedSourceTimeUs,
  );
  return commitTimeline(session, segments, segmentId);
}

export function canSplitAtPlayhead(session: EditorSession) {
  const mapping = editedTimeToSourceTime(session.segments, session.playheadUs);
  if (!mapping) return false;
  const segment = session.segments[mapping.segmentIndex];
  return mapping.sourceTimeUs - segment.sourceStartUs >= MIN_SEGMENT_DURATION_US
    && segment.sourceEndUs - mapping.sourceTimeUs >= MIN_SEGMENT_DURATION_US;
}

export function splitAtPlayhead(session: EditorSession): EditorSession {
  const mapping = editedTimeToSourceTime(session.segments, session.playheadUs);
  if (!mapping || !canSplitAtPlayhead(session)) return session;
  const segment = session.segments[mapping.segmentIndex];
  const rightId = `${session.source.clipId}:segment:${session.nextSegmentOrdinal}`;
  const segments = [
    ...session.segments.slice(0, mapping.segmentIndex),
    { ...segment, sourceEndUs: mapping.sourceTimeUs },
    { id: rightId, sourceStartUs: mapping.sourceTimeUs, sourceEndUs: segment.sourceEndUs },
    ...session.segments.slice(mapping.segmentIndex + 1),
  ];
  return commitTimeline(session, segments, rightId, session.nextSegmentOrdinal + 1);
}

export function canDeleteSelectedSegment(session: EditorSession) {
  return session.segments.length > 1
    && session.selectedSegmentId !== null
    && session.segments.some((segment) => segment.id === session.selectedSegmentId);
}

export function deleteSelectedSegment(session: EditorSession): EditorSession {
  if (!canDeleteSelectedSegment(session)) return session;
  const deletedIndex = session.segments.findIndex((segment) => segment.id === session.selectedSegmentId);
  const segments = session.segments.filter((segment) => segment.id !== session.selectedSegmentId);
  const nextSelected = segments[Math.min(deletedIndex, segments.length - 1)]?.id ?? null;
  return commitTimeline(session, segments, nextSelected);
}

export function resetEditorEdits(session: EditorSession): EditorSession {
  if (!session.dirty) return session;
  const segments = initialSegments(session.source.clipId, session.source.durationUs);
  return commitTimeline(session, segments, segments[0]?.id ?? null);
}

export function undoEditorEdit(session: EditorSession): EditorSession {
  const previous = session.undoStack[session.undoStack.length - 1];
  if (!previous) return session;
  return restoreTimeline(
    session,
    previous.segments,
    session.undoStack.slice(0, -1),
    boundedHistory([...session.redoStack, historyState(session.segments)]),
  );
}

export function redoEditorEdit(session: EditorSession): EditorSession {
  const next = session.redoStack[session.redoStack.length - 1];
  if (!next) return session;
  return restoreTimeline(
    session,
    next.segments,
    boundedHistory([...session.undoStack, historyState(session.segments)]),
    session.redoStack.slice(0, -1),
  );
}

export function timelinePositionToSeconds(position: number, width: number, duration: number) {
  if (!Number.isFinite(position) || !Number.isFinite(width) || width <= 0) return 0;
  const fraction = Math.min(Math.max(position / width, 0), 1);
  return fraction * Math.max(Number.isFinite(duration) ? duration : 0, 0);
}

export function timelineTickTimes(duration: number, intervalCount = 4) {
  const safeDuration = Number.isFinite(duration) ? Math.max(0, duration) : 0;
  const count = Number.isInteger(intervalCount) ? Math.max(1, intervalCount) : 4;
  return Array.from({ length: count + 1 }, (_, index) => safeDuration * index / count);
}

export function formatEditorTime(seconds: number) {
  const safeSeconds = Number.isFinite(seconds) ? Math.max(0, seconds) : 0;
  const minutes = Math.floor(safeSeconds / 60);
  const wholeSeconds = Math.floor(safeSeconds % 60);
  const hundredths = Math.floor((safeSeconds - Math.floor(safeSeconds)) * 100);
  return `${minutes}:${String(wholeSeconds).padStart(2, "0")}.${String(hundredths).padStart(2, "0")}`;
}

export function formatEditorTimeUs(microseconds: number) {
  return formatEditorTime(microsecondsToSeconds(microseconds));
}
