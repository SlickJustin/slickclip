import {
  useCallback,
  useEffect,
  useRef,
  useState,
  type CSSProperties,
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
} from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { ClipListItem, ClipPlaybackInfo, ClipPlaybackInfoResponse, PrepareClipMediaResponse } from "../types/clips";
import { audioLabel, errorMessage } from "../types/clips";
import { isEditableShortcutTarget, mediaTimeToPercent } from "../utils/playerControls";
import {
  canDeleteSelectedSegment,
  canSplitAtPlayhead,
  createEditorSession,
  deleteSelectedSegment,
  editedTimeToSourceTime,
  formatEditorTimeUs,
  microsecondsToSeconds,
  previewTrimmedSegments,
  redoEditorEdit,
  resetEditorEdits,
  resetEditorSession,
  secondsToMicroseconds,
  segmentDurationUs,
  segmentEditedOffsets,
  selectEditorSegment,
  sourceTimeToEditedTime,
  splitAtPlayhead,
  timelineTickTimes,
  totalEditedDurationUs,
  trimEditorSegment,
  undoEditorEdit,
  withEditorDuration,
  withEditorPlaybackState,
  withEditorPlayheadUs,
  type EditorPlaybackState,
  type EditorSegment,
  type EditorSession,
  type EditorTrimEdge,
} from "../utils/editorSession";

const BOUNDARY_TOLERANCE_US = 30_000;
const KEYBOARD_SEEK_US = 1_000_000;
const KEYBOARD_TRIM_US = 100_000;

type EditorMediaStatus = "loading" | "ready" | "preparingProxy" | "error";
type EditorMediaSource = { path: string; url: string; kind: "Master" | "H264 Proxy"; revision: number };
type Props = {
  clip: ClipListItem | null;
  onBackToClips: () => void;
  onDirtyChange: (dirty: boolean) => void;
};
type PendingSeek = { segmentId: string; editedTimeUs: number; resumePlaying: boolean };
type TrimDrag = {
  pointerId: number;
  segmentId: string;
  edge: EditorTrimEdge;
  startClientX: number;
  laneWidth: number;
  initialSourceUs: number;
  editedDurationUs: number;
  sourceTimeUs: number;
  previewSegments: readonly EditorSegment[];
};

export function EditorPage({ clip, onBackToClips, onDirtyChange }: Props) {
  if (!clip) {
    return (
      <div className="page editor-page">
        <header className="page-header">
          <div><h1>Editor</h1><p>Build focused edits from an immutable SlickClip source.</p></div>
          <button className="secondary-button" type="button" onClick={onBackToClips}>Browse Clips</button>
        </header>
        <section className="editor-empty-state" aria-labelledby="editor-empty-heading">
          <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m9 8 7 4-7 4Z" /><rect x="3" y="4" width="18" height="16" rx="2" /></svg>
          <h2 id="editor-empty-heading">Choose a source clip</h2>
          <p>Open Clips and select Edit on the recording you want to work with.</p>
          <button className="primary-button" type="button" onClick={onBackToClips}>Open Clips</button>
        </section>
      </div>
    );
  }

  return <ActiveEditor key={clip.id} clip={clip} onBackToClips={onBackToClips} onDirtyChange={onDirtyChange} />;
}

function ActiveEditor({ clip, onBackToClips, onDirtyChange }: { clip: ClipListItem; onBackToClips: () => void; onDirtyChange: (dirty: boolean) => void }) {
  const [session, setSession] = useState<EditorSession>(() => createEditorSession(clip));
  const [playbackInfo, setPlaybackInfo] = useState<ClipPlaybackInfo | null>(null);
  const [source, setSource] = useState<EditorMediaSource | null>(null);
  const [mediaStatus, setMediaStatus] = useState<EditorMediaStatus>("loading");
  const [mediaError, setMediaError] = useState<string | null>(null);
  const [trimDrag, setTrimDrag] = useState<TrimDrag | null>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const timelineLaneRef = useRef<HTMLDivElement>(null);
  const mountedRef = useRef(true);
  const operationTokenRef = useRef(0);
  const sourceRevisionRef = useRef(0);
  const fallbackStartedRef = useRef(false);
  const seekWatchdogRef = useRef<number | undefined>(undefined);
  const playbackFrameRef = useRef<number | undefined>(undefined);
  const pendingRestoreRef = useRef({ sourceTimeSeconds: 0, play: false });
  const pendingSeekRef = useRef<PendingSeek | null>(null);
  const activeSegmentIdRef = useRef<string | null>(session.selectedSegmentId);
  const editorEndedRef = useRef(false);
  const trimDragRef = useRef<TrimDrag | null>(null);
  const sessionRef = useRef(session);

  const replaceSession = useCallback((next: EditorSession) => {
    sessionRef.current = next;
    setSession(next);
  }, []);

  useEffect(() => { sessionRef.current = session; }, [session]);
  useEffect(() => { onDirtyChange(session.dirty); }, [onDirtyChange, session.dirty]);
  useEffect(() => () => onDirtyChange(false), [onDirtyChange]);

  const updatePlaybackState = useCallback((state: EditorPlaybackState) => {
    setSession((current) => {
      const next = withEditorPlaybackState(current, state);
      sessionRef.current = next;
      return next;
    });
  }, []);

  const failMedia = useCallback((message: string) => {
    setMediaError(message);
    setMediaStatus("error");
    updatePlaybackState("error");
  }, [updatePlaybackState]);

  const useMediaPath = useCallback((path: string, kind: EditorMediaSource["kind"]) => {
    try {
      const revision = ++sourceRevisionRef.current;
      setSource({ path, kind, revision, url: `${convertFileSrc(path)}?v=${revision}` });
      return true;
    } catch (cause) {
      failMedia(`SlickClip could not create a playable media URL: ${errorMessage(cause)}`);
      return false;
    }
  }, [failMedia]);

  useEffect(() => {
    mountedRef.current = true;
    fallbackStartedRef.current = false;
    const reset = resetEditorSession(clip);
    replaceSession(reset);
    activeSegmentIdRef.current = reset.selectedSegmentId;
    setPlaybackInfo(null);
    setSource(null);
    setMediaError(null);
    setMediaStatus("loading");
    const token = ++operationTokenRef.current;

    void invoke<ClipPlaybackInfoResponse>("get_clip_playback_info", { request: { clipId: clip.id } })
      .then((response) => {
        if (!mountedRef.current || operationTokenRef.current !== token) return;
        if (!response.success || !response.info) throw new Error(response.errorMessage ?? "The source clip is unavailable for editing.");
        setPlaybackInfo(response.info);
        setSession((current) => {
          const next = withEditorDuration(current, response.info!.duration100ns / 10_000_000);
          sessionRef.current = next;
          activeSegmentIdRef.current = next.selectedSegmentId;
          return next;
        });
        useMediaPath(response.info.masterPath, "Master");
      })
      .catch((cause) => {
        if (mountedRef.current && operationTokenRef.current === token) failMedia(errorMessage(cause));
      });

    return () => {
      mountedRef.current = false;
      operationTokenRef.current += 1;
      if (seekWatchdogRef.current !== undefined) window.clearTimeout(seekWatchdogRef.current);
      if (playbackFrameRef.current !== undefined) window.cancelAnimationFrame(playbackFrameRef.current);
      seekWatchdogRef.current = undefined;
      playbackFrameRef.current = undefined;
      videoRef.current?.pause();
    };
  }, [clip, failMedia, replaceSession, useMediaPath]);

  const preparePreview = useCallback(async (retry = false) => {
    if (seekWatchdogRef.current !== undefined) window.clearTimeout(seekWatchdogRef.current);
    seekWatchdogRef.current = undefined;
    const currentSession = sessionRef.current;
    const video = videoRef.current;
    const currentMapping = editedTimeToSourceTime(currentSession.segments, currentSession.playheadUs);
    const sourceTimeSeconds = video?.currentTime ?? microsecondsToSeconds(currentMapping?.sourceTimeUs ?? 0);
    const resumePlaying = video ? !video.paused && !video.ended : currentSession.playbackState === "playing";
    const token = ++operationTokenRef.current;
    setMediaError(null);
    setMediaStatus("preparingProxy");
    updatePlaybackState("loading");

    while (mountedRef.current && operationTokenRef.current === token) {
      try {
        const response = await invoke<PrepareClipMediaResponse>("prepare_clip_preview", {
          request: { clipId: clip.id, retry, currentTimeSeconds: sourceTimeSeconds, wasPlaying: resumePlaying },
        });
        retry = false;
        if (!mountedRef.current || operationTokenRef.current !== token) return;
        if (response.artifact.state === "ready" && response.artifact.filePath) {
          pendingRestoreRef.current = { sourceTimeSeconds: response.restoreAtSeconds, play: response.resumePlaying };
          if (useMediaPath(response.artifact.filePath, "H264 Proxy")) setMediaStatus("loading");
          return;
        }
        if (!response.success || response.artifact.state === "error") {
          failMedia(response.errorMessage ?? response.artifact.errorMessage ?? "The editor preview could not be prepared.");
          return;
        }
        await new Promise((resolve) => window.setTimeout(resolve, 800));
      } catch (cause) {
        if (mountedRef.current && operationTokenRef.current === token) failMedia(errorMessage(cause));
        return;
      }
    }
  }, [clip.id, failMedia, updatePlaybackState, useMediaPath]);

  const seekVideoToEditedTime = useCallback((targetSession: EditorSession, requestedTimeUs: number, resumePlaying = false) => {
    const mapping = editedTimeToSourceTime(targetSession.segments, requestedTimeUs);
    if (!mapping) return;
    const next = withEditorPlayheadUs(targetSession, mapping.editedTimeUs);
    replaceSession(next);
    activeSegmentIdRef.current = mapping.segmentId;
    editorEndedRef.current = false;
    const video = videoRef.current;
    if (!video) return;
    const sourceTimeSeconds = microsecondsToSeconds(mapping.sourceTimeUs);
    if (Math.abs(video.currentTime - sourceTimeSeconds) < 0.001 && !video.seeking) {
      pendingSeekRef.current = null;
      if (resumePlaying && video.paused) void video.play().catch((cause) => failMedia(errorMessage(cause)));
      return;
    }
    pendingSeekRef.current = { segmentId: mapping.segmentId, editedTimeUs: mapping.editedTimeUs, resumePlaying };
    video.currentTime = sourceTimeSeconds;
  }, [failMedia, replaceSession]);

  function restoreAfterLoad(video: HTMLVideoElement) {
    const mediaDurationSeconds = Number.isFinite(video.duration) && video.duration > 0
      ? video.duration
      : microsecondsToSeconds(sessionRef.current.source.durationUs);
    const nextSession = withEditorDuration(sessionRef.current, mediaDurationSeconds);
    replaceSession(nextSession);
    const restore = pendingRestoreRef.current;
    const requestedSourceUs = secondsToMicroseconds(restore.sourceTimeSeconds);
    const restoredEditedUs = sourceTimeToEditedTime(nextSession.segments, requestedSourceUs) ?? nextSession.playheadUs;
    const mapping = editedTimeToSourceTime(nextSession.segments, restoredEditedUs);
    if (mapping) {
      activeSegmentIdRef.current = mapping.segmentId;
      video.currentTime = microsecondsToSeconds(mapping.sourceTimeUs);
      replaceSession(withEditorPlayheadUs(nextSession, mapping.editedTimeUs));
    }
    pendingRestoreRef.current = { sourceTimeSeconds: 0, play: false };
    if (restore.play) void video.play().catch(() => updatePlaybackState("paused"));
    else updatePlaybackState("paused");
  }

  function playbackFailed() {
    if (mediaStatus === "preparingProxy") return;
    const media = videoRef.current?.error;
    const message = media
      ? `WebView playback failed (${media.code}): ${media.message || "unsupported media or decode failure"}`
      : "WebView playback failed for this source clip.";
    if (source?.kind === "Master" && !fallbackStartedRef.current) {
      fallbackStartedRef.current = true;
      void preparePreview();
      return;
    }
    failMedia(message);
  }

  function seekingStarted() {
    if (source?.kind !== "Master") return;
    if (seekWatchdogRef.current !== undefined) window.clearTimeout(seekWatchdogRef.current);
    seekWatchdogRef.current = window.setTimeout(() => {
      fallbackStartedRef.current = true;
      void preparePreview();
    }, 2_000);
  }

  function seekingFinished(video: HTMLVideoElement) {
    if (seekWatchdogRef.current !== undefined) window.clearTimeout(seekWatchdogRef.current);
    seekWatchdogRef.current = undefined;
    const pending = pendingSeekRef.current;
    pendingSeekRef.current = null;
    if (pending) {
      activeSegmentIdRef.current = pending.segmentId;
      replaceSession(withEditorPlayheadUs(sessionRef.current, pending.editedTimeUs));
      if (pending.resumePlaying && video.paused) void video.play().catch((cause) => failMedia(errorMessage(cause)));
      return;
    }

    const sourceTimeUs = secondsToMicroseconds(video.currentTime);
    const editedTimeUs = sourceTimeToEditedTime(sessionRef.current.segments, sourceTimeUs);
    if (editedTimeUs !== null) {
      const mapping = editedTimeToSourceTime(sessionRef.current.segments, editedTimeUs);
      activeSegmentIdRef.current = mapping?.segmentId ?? null;
      replaceSession(withEditorPlayheadUs(sessionRef.current, editedTimeUs));
      return;
    }

    let nearestEditedUs = 0;
    let nearestDistanceUs = Number.POSITIVE_INFINITY;
    for (const offset of segmentEditedOffsets(sessionRef.current.segments)) {
      const startDistanceUs = Math.abs(sourceTimeUs - offset.segment.sourceStartUs);
      if (startDistanceUs < nearestDistanceUs) {
        nearestDistanceUs = startDistanceUs;
        nearestEditedUs = offset.editedStartUs;
      }
      const endDistanceUs = Math.abs(sourceTimeUs - offset.segment.sourceEndUs);
      if (endDistanceUs < nearestDistanceUs) {
        nearestDistanceUs = endDistanceUs;
        nearestEditedUs = offset.editedEndUs;
      }
    }
    seekVideoToEditedTime(sessionRef.current, nearestEditedUs, !video.paused);
  }

  const synchronizePlaybackPosition = useCallback((video: HTMLVideoElement, sourceTimeSeconds: number) => {
    if (video.seeking || pendingSeekRef.current) return;
    const current = sessionRef.current;
    const offsets = segmentEditedOffsets(current.segments);
    if (offsets.length === 0) return;
    const sourceTimeUs = secondsToMicroseconds(sourceTimeSeconds);
    let active = offsets.find((offset) => offset.segment.id === activeSegmentIdRef.current);
    if (!active || sourceTimeUs < active.segment.sourceStartUs - BOUNDARY_TOLERANCE_US || sourceTimeUs > active.segment.sourceEndUs + BOUNDARY_TOLERANCE_US) {
      const editedTimeUs = sourceTimeToEditedTime(current.segments, sourceTimeUs);
      if (editedTimeUs === null) return;
      const mapping = editedTimeToSourceTime(current.segments, editedTimeUs);
      active = offsets[mapping?.segmentIndex ?? 0];
      activeSegmentIdRef.current = active.segment.id;
    }

    if (!video.paused && !video.ended && sourceTimeUs >= active.segment.sourceEndUs - BOUNDARY_TOLERANCE_US) {
      const next = offsets[active.index + 1];
      if (next) {
        const resumePlaying = !video.paused;
        activeSegmentIdRef.current = next.segment.id;
        pendingSeekRef.current = {
          segmentId: next.segment.id,
          editedTimeUs: next.editedStartUs,
          resumePlaying,
        };
        replaceSession(withEditorPlayheadUs(current, next.editedStartUs));
        video.currentTime = microsecondsToSeconds(next.segment.sourceStartUs);
        return;
      }
      editorEndedRef.current = true;
      video.pause();
      video.currentTime = microsecondsToSeconds(active.segment.sourceEndUs);
      replaceSession(withEditorPlaybackState(withEditorPlayheadUs(current, active.editedEndUs), "ended"));
      return;
    }

    const editedTimeUs = active.editedStartUs + Math.max(0, sourceTimeUs - active.segment.sourceStartUs);
    replaceSession(withEditorPlayheadUs(current, editedTimeUs));
  }, [replaceSession]);

  const stopPlaybackMonitor = useCallback(() => {
    if (playbackFrameRef.current !== undefined) window.cancelAnimationFrame(playbackFrameRef.current);
    playbackFrameRef.current = undefined;
  }, []);

  const startPlaybackMonitor = useCallback((video: HTMLVideoElement) => {
    stopPlaybackMonitor();
    const tick = () => {
      synchronizePlaybackPosition(video, video.currentTime);
      if (!video.paused && !video.ended) playbackFrameRef.current = window.requestAnimationFrame(tick);
      else playbackFrameRef.current = undefined;
    };
    playbackFrameRef.current = window.requestAnimationFrame(tick);
  }, [stopPlaybackMonitor, synchronizePlaybackPosition]);

  function togglePlayback() {
    const video = videoRef.current;
    if (!video || mediaStatus === "error" || mediaStatus === "preparingProxy") return;
    if (!video.paused && !video.ended) {
      video.pause();
      return;
    }
    const current = sessionRef.current;
    const editedDurationUs = totalEditedDurationUs(current.segments);
    const targetUs = current.playheadUs >= editedDurationUs ? 0 : current.playheadUs;
    seekVideoToEditedTime(current, targetUs, true);
  }

  function applyEdit(next: EditorSession) {
    const current = sessionRef.current;
    if (next === current) return;
    videoRef.current?.pause();
    editorEndedRef.current = false;
    replaceSession(next);
    seekVideoToEditedTime(next, next.playheadUs);
  }

  function seekEditedTimeline(requestedTimeUs: number) {
    seekVideoToEditedTime(sessionRef.current, requestedTimeUs, false);
  }

  function selectSegment(segmentId: string) {
    replaceSession(selectEditorSegment(sessionRef.current, segmentId));
  }

  function beginTrim(event: ReactPointerEvent<HTMLButtonElement>, segment: EditorSegment, edge: EditorTrimEdge) {
    const laneWidth = timelineLaneRef.current?.getBoundingClientRect().width ?? 0;
    if (laneWidth <= 0) return;
    const pointerId = event.pointerId;
    const startClientX = event.clientX;
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture(pointerId);
    videoRef.current?.pause();
    selectSegment(segment.id);
    const drag: TrimDrag = {
      pointerId,
      segmentId: segment.id,
      edge,
      startClientX,
      laneWidth,
      initialSourceUs: edge === "start" ? segment.sourceStartUs : segment.sourceEndUs,
      editedDurationUs: totalEditedDurationUs(sessionRef.current.segments),
      sourceTimeUs: edge === "start" ? segment.sourceStartUs : segment.sourceEndUs,
      previewSegments: sessionRef.current.segments,
    };
    trimDragRef.current = drag;
    setTrimDrag(drag);
  }

  function moveTrim(event: ReactPointerEvent<HTMLButtonElement>) {
    const pointerId = event.pointerId;
    const clientX = event.clientX;
    const drag = trimDragRef.current;
    if (!drag || drag.pointerId !== pointerId) return;
    event.preventDefault();
    event.stopPropagation();
    const deltaUs = Math.round((clientX - drag.startClientX) / drag.laneWidth * drag.editedDurationUs);
    const previewSegments = previewTrimmedSegments(
      sessionRef.current.segments,
      sessionRef.current.source.durationUs,
      drag.segmentId,
      drag.edge,
      drag.initialSourceUs + deltaUs,
    );
    const previewSegment = previewSegments.find((segment) => segment.id === drag.segmentId);
    if (!previewSegment) return;
    const sourceTimeUs = drag.edge === "start" ? previewSegment.sourceStartUs : previewSegment.sourceEndUs;
    const nextDrag = { ...drag, sourceTimeUs, previewSegments };
    trimDragRef.current = nextDrag;
    setTrimDrag(nextDrag);
    const video = videoRef.current;
    if (video) video.currentTime = microsecondsToSeconds(sourceTimeUs);
  }

  function finishTrim(event: ReactPointerEvent<HTMLButtonElement>, commit: boolean) {
    const pointerId = event.pointerId;
    const drag = trimDragRef.current;
    if (!drag || drag.pointerId !== pointerId) return;
    event.preventDefault();
    event.stopPropagation();
    if (event.currentTarget.hasPointerCapture(pointerId)) event.currentTarget.releasePointerCapture(pointerId);
    trimDragRef.current = null;
    setTrimDrag(null);
    if (commit) applyEdit(trimEditorSegment(sessionRef.current, drag.segmentId, drag.edge, drag.sourceTimeUs));
    else seekVideoToEditedTime(sessionRef.current, sessionRef.current.playheadUs);
  }

  function keyboardTrim(event: ReactKeyboardEvent<HTMLButtonElement>, segment: EditorSegment, edge: EditorTrimEdge) {
    const key = event.key;
    if (key !== "ArrowLeft" && key !== "ArrowRight") return;
    event.preventDefault();
    event.stopPropagation();
    const direction = key === "ArrowLeft" ? -1 : 1;
    const sourceTimeUs = (edge === "start" ? segment.sourceStartUs : segment.sourceEndUs) + direction * KEYBOARD_TRIM_US;
    applyEdit(trimEditorSegment(sessionRef.current, segment.id, edge, sourceTimeUs));
  }

  function confirmReset() {
    if (!sessionRef.current.dirty) return;
    if (window.confirm("Reset all timeline edits to the full source clip?")) applyEdit(resetEditorEdits(sessionRef.current));
  }

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const target = event.target as HTMLElement | null;
      if (isEditableShortcutTarget(target?.tagName, Boolean(target?.isContentEditable))) return;
      if (target?.tagName === "BUTTON") return;
      const key = event.key.toLowerCase();
      if (event.ctrlKey && key === "z") {
        event.preventDefault();
        applyEdit(event.shiftKey ? redoEditorEdit(sessionRef.current) : undoEditorEdit(sessionRef.current));
        return;
      }
      if (event.ctrlKey && key === "y") {
        event.preventDefault();
        applyEdit(redoEditorEdit(sessionRef.current));
        return;
      }
      if (event.code === "Space") {
        event.preventDefault();
        togglePlayback();
      } else if (event.key === "Delete") {
        event.preventDefault();
        applyEdit(deleteSelectedSegment(sessionRef.current));
      } else if (!event.ctrlKey && !event.altKey && key === "s") {
        event.preventDefault();
        applyEdit(splitAtPlayhead(sessionRef.current));
      } else if (event.key === "ArrowLeft" || event.key === "ArrowRight") {
        event.preventDefault();
        const direction = event.key === "ArrowLeft" ? -1 : 1;
        const distanceUs = event.shiftKey ? KEYBOARD_SEEK_US * 5 : KEYBOARD_SEEK_US;
        seekEditedTimeline(sessionRef.current.playheadUs + direction * distanceUs);
      }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  });

  const displaySegments = trimDrag?.previewSegments ?? session.segments;
  const editedDurationUs = totalEditedDurationUs(displaySegments);
  const authoritativeEditedDurationUs = totalEditedDurationUs(session.segments);
  const timelinePercent = mediaTimeToPercent(session.playheadUs, authoritativeEditedDurationUs);
  const timelineStyle = { "--editor-playhead": `${timelinePercent}%` } as CSSProperties;
  const audioTracks = playbackInfo?.audioTracks ?? session.source.audioTracks;
  const splitEnabled = canSplitAtPlayhead(session);
  const deleteEnabled = canDeleteSelectedSegment(session);
  const selectedSegment = displaySegments.find((segment) => segment.id === session.selectedSegmentId) ?? null;

  return (
    <div className="page editor-page">
      <header className="page-header editor-page-header">
        <div className="editor-title-block">
          <button className="editor-back-button" type="button" onClick={onBackToClips}>← Clips</button>
          <div><h1>Editor</h1><p>{session.source.displayName}</p></div>
        </div>
        <div className="editor-header-status">
          {session.dirty && <span className="editor-dirty-state">Unsaved edits</span>}
          <span className="editor-source-safety">Original protected</span>
        </div>
      </header>

      <div className="editor-workspace">
        <section className="editor-preview-panel" aria-label="Editor video preview">
          <div className="editor-preview-heading"><span>Preview</span><small>{source?.kind ?? "Resolving source"}</small></div>
          <div className="editor-preview-stage">
            {source && <video
              key={`${source.kind}:${source.path}:${source.revision}`}
              ref={videoRef}
              src={source.url}
              controls
              playsInline
              preload="metadata"
              aria-label={`Editor preview for ${session.source.displayName}`}
              onLoadStart={() => { setMediaStatus("loading"); updatePlaybackState("loading"); }}
              onLoadedMetadata={(event) => restoreAfterLoad(event.currentTarget)}
              onCanPlay={() => setMediaStatus("ready")}
              onPlay={(event) => {
                editorEndedRef.current = false;
                updatePlaybackState("playing");
                startPlaybackMonitor(event.currentTarget);
              }}
              onPause={() => {
                stopPlaybackMonitor();
                if (!editorEndedRef.current && !videoRef.current?.ended) updatePlaybackState("paused");
              }}
              onEnded={() => {
                stopPlaybackMonitor();
                const current = sessionRef.current;
                replaceSession(withEditorPlaybackState(withEditorPlayheadUs(current, totalEditedDurationUs(current.segments)), "ended"));
              }}
              onTimeUpdate={(event) => {
                const sourceTimeSeconds = event.currentTarget.currentTime;
                synchronizePlaybackPosition(event.currentTarget, sourceTimeSeconds);
              }}
              onDurationChange={(event) => {
                const mediaDuration = event.currentTarget.duration;
                if (Number.isFinite(mediaDuration) && mediaDuration > 0) {
                  setSession((current) => {
                    const next = withEditorDuration(current, mediaDuration);
                    sessionRef.current = next;
                    return next;
                  });
                }
              }}
              onSeeking={seekingStarted}
              onSeeked={(event) => seekingFinished(event.currentTarget)}
              onError={playbackFailed}
            />}
            {mediaStatus === "loading" && <div className="editor-media-message"><span className="player-spinner" />Loading source clip...</div>}
            {mediaStatus === "preparingProxy" && <div className="editor-media-message"><span className="player-spinner" />Preparing H.264 editor preview...</div>}
            {mediaStatus === "error" && <div className="editor-media-message editor-media-error" role="alert">
              <strong>Editor preview unavailable</strong><span>{mediaError}</span>
              <div><button type="button" onClick={() => void preparePreview(true)}>Retry Preview</button><button type="button" onClick={onBackToClips}>Return to Clips</button></div>
            </div>}
          </div>
          <div className="editor-transport">
            <button type="button" onClick={togglePlayback} disabled={!source || mediaStatus !== "ready"}>{session.playbackState === "playing" ? "Pause" : "Play"}</button>
            <code>{formatEditorTimeUs(session.playheadUs)} / {formatEditorTimeUs(authoritativeEditedDurationUs)}</code>
            <span>{session.playbackState}</span>
          </div>
        </section>

        <aside className="editor-source-panel" aria-labelledby="editor-source-heading">
          <div className="section-heading"><div><span className="eyebrow">SOURCE</span><h2 id="editor-source-heading">Clip details</h2></div></div>
          <dl className="editor-source-details">
            <div><dt>Name</dt><dd>{session.source.displayName}</dd></div>
            <div><dt>File</dt><dd title={session.source.filePath}>{session.source.filename}</dd></div>
            <div><dt>Resolution</dt><dd>{session.source.width}×{session.source.height}</dd></div>
            <div><dt>Video</dt><dd>{session.source.videoCodec.toUpperCase()}</dd></div>
            <div><dt>Original</dt><dd>{formatEditorTimeUs(session.source.durationUs)}</dd></div>
            <div><dt>Edited</dt><dd>{formatEditorTimeUs(authoritativeEditedDurationUs)}</dd></div>
          </dl>
          <div className="editor-source-audio"><span>Saved audio tracks</span><div>{audioTracks.length > 0 ? audioTracks.map((track) => <span key={track.streamIndex}>{audioLabel(track)}</span>) : <small>No audio tracks</small>}</div></div>
          <div className="editor-safety-note"><strong>Shared non-destructive cuts</strong><span>The source MP4 stays unchanged. This timeline will govern video and every saved audio track.</span></div>
        </aside>

        <section className="editor-timeline-panel" aria-labelledby="editor-timeline-heading">
          <div className="editor-timeline-heading">
            <div><span className="eyebrow">EDIT DECISION LIST</span><h2 id="editor-timeline-heading">Edited timeline</h2></div>
            <code>{formatEditorTimeUs(session.playheadUs)}</code>
          </div>

          <div className="editor-toolbar" aria-label="Timeline editing actions">
            <button type="button" onClick={() => applyEdit(undoEditorEdit(sessionRef.current))} disabled={session.undoStack.length === 0} title="Undo (Ctrl+Z)">Undo</button>
            <button type="button" onClick={() => applyEdit(redoEditorEdit(sessionRef.current))} disabled={session.redoStack.length === 0} title="Redo (Ctrl+Shift+Z or Ctrl+Y)">Redo</button>
            <span className="editor-toolbar-divider" aria-hidden="true" />
            <button type="button" onClick={() => applyEdit(splitAtPlayhead(sessionRef.current))} disabled={!splitEnabled} title="Split at playhead (S)">Split</button>
            <button className="editor-delete-segment" type="button" onClick={() => applyEdit(deleteSelectedSegment(sessionRef.current))} disabled={!deleteEnabled} title="Delete selected segment (Delete)">Delete Segment</button>
            <button type="button" onClick={confirmReset} disabled={!session.dirty}>Reset</button>
            <div className="editor-duration-status" aria-label="Timeline duration">
              <span>Original <strong>{formatEditorTimeUs(session.source.durationUs)}</strong></span>
              <span>Edited <strong>{formatEditorTimeUs(authoritativeEditedDurationUs)}</strong></span>
            </div>
          </div>

          {trimDrag && <div className="editor-trim-feedback" role="status">
            {trimDrag.edge === "start" ? "Trim start" : "Trim end"}: source {formatEditorTimeUs(trimDrag.sourceTimeUs)} · edited {formatEditorTimeUs(editedDurationUs)}
          </div>}

          <div className="editor-timeline-ruler" aria-hidden="true">
            {timelineTickTimes(microsecondsToSeconds(authoritativeEditedDurationUs)).map((time, index) => <span key={index}>{formatEditorTimeUs(secondsToMicroseconds(time))}</span>)}
          </div>
          <div className="editor-timeline-row">
            <span className="editor-track-label">RESULT</span>
            <div className="editor-timeline-lane" ref={timelineLaneRef} style={timelineStyle}>
              <div className="editor-segment-strip">
                {displaySegments.map((segment, index) => {
                  const selected = segment.id === session.selectedSegmentId;
                  const width = editedDurationUs > 0 ? segmentDurationUs(segment) / editedDurationUs * 100 : 0;
                  return (
                    <div
                      className={`editor-segment${selected ? " editor-segment-selected" : ""}${index > 0 ? " editor-segment-cut" : ""}`}
                      key={segment.id}
                      style={{ width: `${width}%` }}
                      role="button"
                      tabIndex={0}
                      aria-pressed={selected}
                      aria-label={`Select segment ${index + 1}, source ${formatEditorTimeUs(segment.sourceStartUs)} to ${formatEditorTimeUs(segment.sourceEndUs)}`}
                      title={`Source ${formatEditorTimeUs(segment.sourceStartUs)} → ${formatEditorTimeUs(segment.sourceEndUs)}`}
                      onClick={() => selectSegment(segment.id)}
                      onKeyDown={(event) => {
                        if (event.key === "Enter" || event.code === "Space") {
                          event.preventDefault();
                          event.stopPropagation();
                          selectSegment(segment.id);
                        }
                      }}
                    >
                      {selected && <button
                        className="editor-trim-handle editor-trim-handle-start"
                        type="button"
                        aria-label="Trim segment start"
                        title="Drag to trim segment start"
                        onPointerDown={(event) => beginTrim(event, segment, "start")}
                        onPointerMove={moveTrim}
                        onPointerUp={(event) => finishTrim(event, true)}
                        onPointerCancel={(event) => finishTrim(event, false)}
                        onKeyDown={(event) => keyboardTrim(event, segment, "start")}
                      />}
                      <span className="editor-segment-label">{index + 1}</span>
                      {selected && <button
                        className="editor-trim-handle editor-trim-handle-end"
                        type="button"
                        aria-label="Trim segment end"
                        title="Drag to trim segment end"
                        onPointerDown={(event) => beginTrim(event, segment, "end")}
                        onPointerMove={moveTrim}
                        onPointerUp={(event) => finishTrim(event, true)}
                        onPointerCancel={(event) => finishTrim(event, false)}
                        onKeyDown={(event) => keyboardTrim(event, segment, "end")}
                      />}
                    </div>
                  );
                })}
              </div>
              <span className="editor-timeline-progress" />
              <span className="editor-timeline-playhead" />
              <label className="editor-seek-strip">
                <span className="visually-hidden">Seek edited timeline</span>
                <input
                  type="range"
                  min="0"
                  max={Math.max(authoritativeEditedDurationUs, 1)}
                  step="1000"
                  value={Math.min(session.playheadUs, authoritativeEditedDurationUs)}
                  disabled={!source || authoritativeEditedDurationUs <= 0 || mediaStatus !== "ready" || Boolean(trimDrag)}
                  aria-label="Edited timeline playhead"
                  aria-valuetext={`${formatEditorTimeUs(session.playheadUs)} of ${formatEditorTimeUs(authoritativeEditedDurationUs)}`}
                  onChange={(event) => {
                    const editedTimeUs = Number(event.currentTarget.value);
                    seekEditedTimeline(editedTimeUs);
                  }}
                />
              </label>
            </div>
          </div>
          <div className="editor-timeline-footer">
            <span>{session.segments.length} segment{session.segments.length === 1 ? "" : "s"}{selectedSegment ? ` · Selected source ${formatEditorTimeUs(selectedSegment.sourceStartUs)}–${formatEditorTimeUs(selectedSegment.sourceEndUs)}` : ""}</span>
            <span>Select a segment to trim. Use the lower rail to seek.</span>
          </div>
        </section>
      </div>
    </div>
  );
}
