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
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ClipActionResponse,
  ClipListItem,
  ClipPlaybackInfo,
  ClipPlaybackInfoResponse,
  EditorExportCommandResponse,
  EditorExportStatus,
  PrepareClipMediaResponse,
} from "../types/clips";
import { audioLabel, errorMessage } from "../types/clips";
import {
  adoptEditorExportStatus,
  applyEditorExportEvent,
  areEditorControlsLocked,
  createEditorExportUiState,
  isEditorExportActive,
  requestEditorExportCancellation,
  snapshotEditorExport,
} from "../utils/editorExport";
import { isEditableShortcutTarget, mediaTimeToPercent } from "../utils/playerControls";
import {
  AUDIO_DRIFT_THRESHOLD_MS,
  audioDriftCorrectionPlan,
  createEditorMixer,
  effectiveTrackGain,
  isEditorDirty,
  isEditorMixerDirty,
  resetEditorAudio,
  toggleEditorTrackMute,
  toggleEditorTrackSolo,
  withEditorTrackAvailability,
  withEditorTrackGain,
  type EditorAudioTrack,
  type EditorMixerState,
} from "../utils/editorMixer";
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
  onPlayExport: (clip: ClipListItem) => void;
  onDirtyChange: (dirty: boolean) => void;
};
type PendingSeek = { segmentId: string; editedTimeUs: number; resumePlaying: boolean };
type EditorAudioRuntime = {
  element: HTMLAudioElement;
  sourceNode: MediaElementAudioSourceNode;
  gainNode: GainNode;
};
type AudioContextStatus = "idle" | "suspended" | "running" | "blocked";
type AudioSyncTelemetry = { maxDriftMs: number; resyncCount: number };
type AudioSyncTelemetryRuntime = AudioSyncTelemetry & { lastPublishedAtMs: number };
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

export function EditorPage({ clip, onBackToClips, onPlayExport, onDirtyChange }: Props) {
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

  return <ActiveEditor key={clip.id} clip={clip} onBackToClips={onBackToClips} onPlayExport={onPlayExport} onDirtyChange={onDirtyChange} />;
}

function ActiveEditor({ clip, onBackToClips, onPlayExport, onDirtyChange }: { clip: ClipListItem; onBackToClips: () => void; onPlayExport: (clip: ClipListItem) => void; onDirtyChange: (dirty: boolean) => void }) {
  const [session, setSession] = useState<EditorSession>(() => createEditorSession(clip));
  const [playbackInfo, setPlaybackInfo] = useState<ClipPlaybackInfo | null>(null);
  const [source, setSource] = useState<EditorMediaSource | null>(null);
  const [mediaStatus, setMediaStatus] = useState<EditorMediaStatus>("loading");
  const [mediaError, setMediaError] = useState<string | null>(null);
  const [trimDrag, setTrimDrag] = useState<TrimDrag | null>(null);
  const [mixer, setMixer] = useState<EditorMixerState>(() => createEditorMixer(clip.audioTracks));
  const [audioContextStatus, setAudioContextStatus] = useState<AudioContextStatus>("idle");
  const [audioRuntimeMessage, setAudioRuntimeMessage] = useState<string | null>(null);
  const [audioTelemetry, setAudioTelemetry] = useState<AudioSyncTelemetry>({ maxDriftMs: 0, resyncCount: 0 });
  const [exportUi, setExportUi] = useState(createEditorExportUiState);
  const [exportCommandError, setExportCommandError] = useState<string | null>(null);
  const exportActive = areEditorControlsLocked(exportUi);
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
  const mixerRef = useRef(mixer);
  const audioContextRef = useRef<AudioContext | null>(null);
  const audioRuntimeRef = useRef(new Map<string, EditorAudioRuntime>());
  const audioPrepareGenerationRef = useRef(0);
  const audioTrackAttemptRef = useRef(new Map<string, number>());
  const audioTelemetryRef = useRef<AudioSyncTelemetryRuntime>({ maxDriftMs: 0, resyncCount: 0, lastPublishedAtMs: 0 });

  const replaceSession = useCallback((next: EditorSession) => {
    sessionRef.current = next;
    setSession(next);
  }, []);

  const replaceMixer = useCallback((next: EditorMixerState) => {
    mixerRef.current = next;
    setMixer(next);
  }, []);

  const updateMixer = useCallback((update: (current: EditorMixerState) => EditorMixerState) => {
    const next = update(mixerRef.current);
    mixerRef.current = next;
    setMixer(next);
  }, []);

  useEffect(() => { sessionRef.current = session; }, [session]);
  useEffect(() => { mixerRef.current = mixer; }, [mixer]);
  const editorDirty = isEditorDirty(session.dirty, mixer);
  useEffect(() => { onDirtyChange(editorDirty); }, [editorDirty, onDirtyChange]);
  useEffect(() => () => onDirtyChange(false), [onDirtyChange]);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    const acceptStatus = (status: EditorExportStatus) => {
      if (status.sourceClipId !== clip.id) return;
      setExportUi((current) => {
        if (current.status?.exportId) return applyEditorExportEvent(current, status);
        return isEditorExportActive(status) ? adoptEditorExportStatus(status) : current;
      });
    };
    void listen<EditorExportStatus>("editor-export-status", (event) => acceptStatus(event.payload))
      .then((cleanup) => {
        if (disposed) cleanup();
        else unlisten = cleanup;
      });
    void invoke<EditorExportStatus>("get_editor_export_status")
      .then((status) => {
        if (!disposed) acceptStatus(status);
      })
      .catch(() => undefined);
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [clip.id]);

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

  const pauseAudioFollowers = useCallback(() => {
    for (const runtime of audioRuntimeRef.current.values()) runtime.element.pause();
  }, []);

  async function startEditorExport() {
    if (exportActive) return;
    videoRef.current?.pause();
    pauseAudioFollowers();
    setExportCommandError(null);
    const request = snapshotEditorExport(clip.id, sessionRef.current.segments, mixerRef.current);
    try {
      const response = await invoke<EditorExportCommandResponse>("start_editor_export", { request });
      if (response.status.sourceClipId === clip.id) {
        setExportUi(adoptEditorExportStatus(response.status));
      }
      if (!response.success) {
        setExportCommandError(response.errorMessage ?? "Could not start the Editor export.");
      }
    } catch (cause) {
      setExportCommandError(`Could not start the Editor export: ${errorMessage(cause)}`);
    }
  }

  async function cancelEditorExport() {
    const exportId = exportUi.status?.exportId;
    if (!exportId || !exportActive) return;
    setExportUi(requestEditorExportCancellation);
    setExportCommandError(null);
    try {
      const response = await invoke<EditorExportCommandResponse>("cancel_editor_export", { exportId });
      if (!response.success) {
        setExportCommandError(response.errorMessage ?? "Could not cancel the Editor export.");
      }
    } catch (cause) {
      setExportCommandError(`Could not cancel the Editor export: ${errorMessage(cause)}`);
    }
  }

  async function openExportFolder() {
    const exportedClip = exportUi.status?.outputClip;
    if (!exportedClip) return;
    const response = await invoke<ClipActionResponse>("open_clip_folder", {
      request: { clipId: exportedClip.id },
    });
    if (!response.success) setExportCommandError(response.errorMessage ?? "Could not open the export folder.");
  }

  const seekAudioFollowers = useCallback((sourceTimeSeconds: number) => {
    const safeTimeSeconds = Number.isFinite(sourceTimeSeconds) ? Math.max(0, sourceTimeSeconds) : 0;
    for (const runtime of audioRuntimeRef.current.values()) {
      if (Math.abs(runtime.element.currentTime - safeTimeSeconds) >= 0.001) {
        runtime.element.currentTime = safeTimeSeconds;
      }
    }
  }, []);

  const disposeAllEditorAudio = useCallback(() => {
    for (const runtime of audioRuntimeRef.current.values()) {
      runtime.element.onerror = null;
      runtime.element.pause();
      runtime.element.removeAttribute("src");
      runtime.element.load();
      runtime.sourceNode.disconnect();
      runtime.gainNode.disconnect();
    }
    audioRuntimeRef.current.clear();
    const context = audioContextRef.current;
    audioContextRef.current = null;
    if (context) {
      context.onstatechange = null;
      if (context.state !== "closed") void context.close().catch(() => undefined);
    }
  }, []);

  const ensureAudioContext = useCallback(() => {
    let context = audioContextRef.current;
    if (!context || context.state === "closed") {
      const newContext = new AudioContext();
      context = newContext;
      audioContextRef.current = newContext;
      newContext.onstatechange = () => {
        if (audioContextRef.current !== newContext) return;
        setAudioContextStatus(newContext.state === "running" ? "running" : "suspended");
      };
    }
    setAudioContextStatus(context.state === "running" ? "running" : "suspended");
    return context;
  }, []);

  const installEditorAudioRuntime = useCallback((track: EditorAudioTrack, filePath: string, generation: number) => {
    if (audioPrepareGenerationRef.current !== generation) return;
    try {
      const context = ensureAudioContext();
      const element = new Audio();
      element.crossOrigin = "anonymous";
      element.preload = "auto";
      element.src = `${convertFileSrc(filePath)}?editor-audio=${track.streamIndex}`;
      const sourceNode = context.createMediaElementSource(element);
      const gainNode = context.createGain();
      sourceNode.connect(gainNode);
      gainNode.connect(context.destination);
      const currentTrack = mixerRef.current.tracks.find((candidate) => candidate.id === track.id) ?? track;
      gainNode.gain.value = effectiveTrackGain(currentTrack, mixerRef.current.tracks);
      const runtime = { element, sourceNode, gainNode };
      audioRuntimeRef.current.set(track.id, runtime);
      element.onerror = () => {
        if (audioRuntimeRef.current.get(track.id) !== runtime) return;
        audioRuntimeRef.current.delete(track.id);
        element.pause();
        sourceNode.disconnect();
        gainNode.disconnect();
        const code = element.error?.code;
        updateMixer((current) => withEditorTrackAvailability(
          current,
          track.id,
          "error",
          `WebView2 could not decode this prepared audio track${code ? ` (media error ${code})` : ""}.`,
        ));
      };
      updateMixer((current) => withEditorTrackAvailability(current, track.id, "ready"));
    } catch (cause) {
      updateMixer((current) => withEditorTrackAvailability(
        current,
        track.id,
        "error",
        `SlickClip could not initialize this Editor audio track: ${errorMessage(cause)}`,
      ));
    }
  }, [ensureAudioContext, updateMixer]);

  const prepareEditorAudioTrack = useCallback(async (track: EditorAudioTrack, retry = false) => {
    const generation = audioPrepareGenerationRef.current;
    const attempt = (audioTrackAttemptRef.current.get(track.id) ?? 0) + 1;
    audioTrackAttemptRef.current.set(track.id, attempt);
    updateMixer((current) => withEditorTrackAvailability(current, track.id, "preparing"));

    while (audioPrepareGenerationRef.current === generation && audioTrackAttemptRef.current.get(track.id) === attempt) {
      try {
        const response = await invoke<PrepareClipMediaResponse>("prepare_editor_audio_preview", {
          request: {
            clipId: clip.id,
            streamIndex: track.streamIndex,
            retry,
            currentTimeSeconds: 0,
            wasPlaying: false,
          },
        });
        retry = false;
        if (audioPrepareGenerationRef.current !== generation || audioTrackAttemptRef.current.get(track.id) !== attempt) return;
        if (response.artifact.state === "ready" && response.artifact.filePath) {
          installEditorAudioRuntime(track, response.artifact.filePath, generation);
          return;
        }
        if (!response.success || response.artifact.state === "error") {
          updateMixer((current) => withEditorTrackAvailability(
            current,
            track.id,
            "error",
            response.errorMessage ?? response.artifact.errorMessage ?? "This Editor audio track could not be prepared.",
          ));
          return;
        }
        await new Promise((resolve) => window.setTimeout(resolve, 800));
      } catch (cause) {
        if (audioPrepareGenerationRef.current === generation && audioTrackAttemptRef.current.get(track.id) === attempt) {
          updateMixer((current) => withEditorTrackAvailability(
            current,
            track.id,
            "error",
            `Editor audio preparation failed: ${errorMessage(cause)}`,
          ));
        }
        return;
      }
    }
  }, [clip.id, installEditorAudioRuntime, updateMixer]);

  useEffect(() => {
    const generation = ++audioPrepareGenerationRef.current;
    disposeAllEditorAudio();
    audioTrackAttemptRef.current.clear();
    const nextMixer = createEditorMixer(clip.audioTracks);
    replaceMixer(nextMixer);
    setAudioContextStatus("idle");
    setAudioRuntimeMessage(null);
    const initialTelemetry = { maxDriftMs: 0, resyncCount: 0, lastPublishedAtMs: 0 };
    audioTelemetryRef.current = initialTelemetry;
    setAudioTelemetry({ maxDriftMs: 0, resyncCount: 0 });
    for (const track of nextMixer.tracks) void prepareEditorAudioTrack(track);

    return () => {
      if (audioPrepareGenerationRef.current === generation) audioPrepareGenerationRef.current += 1;
      audioTrackAttemptRef.current.clear();
      disposeAllEditorAudio();
    };
  }, [clip.audioTracks, disposeAllEditorAudio, prepareEditorAudioTrack, replaceMixer]);

  useEffect(() => {
    const context = audioContextRef.current;
    for (const track of mixer.tracks) {
      const runtime = audioRuntimeRef.current.get(track.id);
      if (!runtime) continue;
      const gain = effectiveTrackGain(track, mixer.tracks);
      if (context && context.state !== "closed") runtime.gainNode.gain.setValueAtTime(gain, context.currentTime);
      else runtime.gainNode.gain.value = gain;
    }
  }, [mixer]);

  const playAudioFollowersAt = useCallback((sourceTimeSeconds: number) => {
    const generation = audioPrepareGenerationRef.current;
    seekAudioFollowers(sourceTimeSeconds);
    const context = audioContextRef.current;
    let resumePromise: Promise<void> | null = null;
    try {
      if (context && context.state !== "running") resumePromise = context.resume();
    } catch (cause) {
      setAudioContextStatus("blocked");
      setAudioRuntimeMessage(`Editor audio could not start: ${errorMessage(cause)}`);
    }

    const playAttempts = [...audioRuntimeRef.current.values()].map((runtime) => {
      try {
        return runtime.element.play();
      } catch (cause) {
        return Promise.reject(cause);
      }
    });
    void Promise.allSettled([
      ...(resumePromise ? [resumePromise] : []),
      ...playAttempts,
    ]).then((results) => {
      if (audioPrepareGenerationRef.current !== generation) return;
      const failures = results.filter((result) => result.status === "rejected");
      if (failures.length > 0 || (context && context.state !== "running")) {
        setAudioContextStatus("blocked");
        setAudioRuntimeMessage("Editor audio is blocked or suspended. Press Play again to allow audio playback.");
        return;
      }
      if (context) setAudioContextStatus("running");
      setAudioRuntimeMessage(null);
    });
  }, [seekAudioFollowers]);

  const startSynchronizedPlayback = useCallback((video: HTMLVideoElement, sourceTimeSeconds: number) => {
    playAudioFollowersAt(sourceTimeSeconds);
    if (video.paused || video.ended) void video.play().catch((cause) => failMedia(errorMessage(cause)));
  }, [failMedia, playAudioFollowersAt]);

  const readyAudioRuntimeKey = mixer.tracks
    .filter((track) => track.availability === "ready")
    .map((track) => track.id)
    .join("|");

  useEffect(() => {
    const video = videoRef.current;
    if (readyAudioRuntimeKey && video && !video.paused && !video.ended) {
      playAudioFollowersAt(video.currentTime);
    }
  }, [playAudioFollowersAt, readyAudioRuntimeKey]);

  const monitorAudioDrift = useCallback((videoTimeSeconds: number) => {
    const telemetry = audioTelemetryRef.current;
    let corrected = false;
    for (const runtime of audioRuntimeRef.current.values()) {
      const audio = runtime.element;
      if (audio.paused || audio.ended || audio.seeking) continue;
      const plan = audioDriftCorrectionPlan(videoTimeSeconds, audio.currentTime, AUDIO_DRIFT_THRESHOLD_MS);
      telemetry.maxDriftMs = Math.max(telemetry.maxDriftMs, plan.driftMs);
      if (plan.shouldCorrect && plan.correctedTimeSeconds !== null) {
        audio.currentTime = plan.correctedTimeSeconds;
        telemetry.resyncCount += 1;
        corrected = true;
      }
    }
    const now = performance.now();
    if (corrected || now - telemetry.lastPublishedAtMs >= 500) {
      telemetry.lastPublishedAtMs = now;
      setAudioTelemetry({ maxDriftMs: telemetry.maxDriftMs, resyncCount: telemetry.resyncCount });
    }
  }, []);

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
    const shouldPlay = resumePlaying || (!video.paused && !video.ended);
    seekAudioFollowers(sourceTimeSeconds);
    if (!shouldPlay) pauseAudioFollowers();
    if (Math.abs(video.currentTime - sourceTimeSeconds) < 0.001 && !video.seeking) {
      pendingSeekRef.current = null;
      if (shouldPlay) startSynchronizedPlayback(video, sourceTimeSeconds);
      return;
    }
    pendingSeekRef.current = { segmentId: mapping.segmentId, editedTimeUs: mapping.editedTimeUs, resumePlaying: shouldPlay };
    video.currentTime = sourceTimeSeconds;
    if (shouldPlay) startSynchronizedPlayback(video, sourceTimeSeconds);
  }, [pauseAudioFollowers, replaceSession, seekAudioFollowers, startSynchronizedPlayback]);

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
      const sourceTimeSeconds = microsecondsToSeconds(mapping.sourceTimeUs);
      video.currentTime = sourceTimeSeconds;
      seekAudioFollowers(sourceTimeSeconds);
      replaceSession(withEditorPlayheadUs(nextSession, mapping.editedTimeUs));
    }
    pendingRestoreRef.current = { sourceTimeSeconds: 0, play: false };
    if (restore.play) startSynchronizedPlayback(video, video.currentTime);
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
    const video = videoRef.current;
    if (video) seekAudioFollowers(video.currentTime);
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
      seekAudioFollowers(video.currentTime);
      if (pending.resumePlaying) startSynchronizedPlayback(video, video.currentTime);
      else pauseAudioFollowers();
      return;
    }

    seekAudioFollowers(video.currentTime);
    if (!video.paused && !video.ended) playAudioFollowersAt(video.currentTime);
    else pauseAudioFollowers();

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
        const nextSourceTimeSeconds = microsecondsToSeconds(next.segment.sourceStartUs);
        seekAudioFollowers(nextSourceTimeSeconds);
        video.currentTime = nextSourceTimeSeconds;
        return;
      }
      editorEndedRef.current = true;
      video.pause();
      pauseAudioFollowers();
      video.currentTime = microsecondsToSeconds(active.segment.sourceEndUs);
      seekAudioFollowers(video.currentTime);
      replaceSession(withEditorPlaybackState(withEditorPlayheadUs(current, active.editedEndUs), "ended"));
      return;
    }

    if (!video.paused && !video.ended) monitorAudioDrift(sourceTimeSeconds);
    const editedTimeUs = active.editedStartUs + Math.max(0, sourceTimeUs - active.segment.sourceStartUs);
    replaceSession(withEditorPlayheadUs(current, editedTimeUs));
  }, [monitorAudioDrift, pauseAudioFollowers, replaceSession, seekAudioFollowers]);

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
    if (exportActive) return;
    const video = videoRef.current;
    if (!video || mediaStatus === "error" || mediaStatus === "preparingProxy") return;
    if (!video.paused && !video.ended) {
      video.pause();
      pauseAudioFollowers();
      return;
    }
    const current = sessionRef.current;
    const editedDurationUs = totalEditedDurationUs(current.segments);
    const targetUs = current.playheadUs >= editedDurationUs ? 0 : current.playheadUs;
    seekVideoToEditedTime(current, targetUs, true);
  }

  function applyEdit(next: EditorSession) {
    if (exportActive) return;
    const current = sessionRef.current;
    if (next === current) return;
    videoRef.current?.pause();
    pauseAudioFollowers();
    editorEndedRef.current = false;
    replaceSession(next);
    seekVideoToEditedTime(next, next.playheadUs);
  }

  function seekEditedTimeline(requestedTimeUs: number) {
    if (exportActive) return;
    seekVideoToEditedTime(sessionRef.current, requestedTimeUs, false);
  }

  function selectSegment(segmentId: string) {
    if (exportActive) return;
    replaceSession(selectEditorSegment(sessionRef.current, segmentId));
  }

  function beginTrim(event: ReactPointerEvent<HTMLButtonElement>, segment: EditorSegment, edge: EditorTrimEdge) {
    if (exportActive) return;
    const laneWidth = timelineLaneRef.current?.getBoundingClientRect().width ?? 0;
    if (laneWidth <= 0) return;
    const pointerId = event.pointerId;
    const startClientX = event.clientX;
    event.preventDefault();
    event.stopPropagation();
    event.currentTarget.setPointerCapture(pointerId);
    videoRef.current?.pause();
    pauseAudioFollowers();
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
    if (exportActive) return;
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
    if (video) {
      const sourceTimeSeconds = microsecondsToSeconds(sourceTimeUs);
      video.currentTime = sourceTimeSeconds;
      seekAudioFollowers(sourceTimeSeconds);
    }
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
    if (exportActive) return;
    if (!sessionRef.current.dirty) return;
    if (window.confirm("Reset all timeline edits to the full source clip?")) applyEdit(resetEditorEdits(sessionRef.current));
  }

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (exportActive) return;
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
  const readyAudioTrackCount = mixer.tracks.filter((track) => track.availability === "ready").length;
  const preparingAudioTrackCount = mixer.tracks.filter((track) => track.availability === "preparing").length;
  const failedAudioTrackCount = mixer.tracks.filter((track) => track.availability === "error" || track.availability === "unavailable").length;
  const allAudioTracksFailed = mixer.tracks.length > 0 && failedAudioTrackCount === mixer.tracks.length;
  const mixerStatus = mixer.tracks.length === 0
    ? "No Editor audio tracks"
    : allAudioTracksFailed
      ? "Audio unavailable"
      : preparingAudioTrackCount > 0
        ? `Preparing audio tracks (${readyAudioTrackCount}/${mixer.tracks.length} ready)`
        : audioContextStatus === "blocked"
          ? "Audio blocked — press Play again"
          : audioContextStatus === "running"
            ? "Ready · Audio active"
            : "Ready · Audio starts when you press Play";
  const exportStatus = exportUi.status;
  const exportPhaseLabel = exportStatus ? ({
    idle: "Ready to export",
    preparing: "Preparing export",
    rendering: "Rendering",
    verifying: "Verifying output",
    finalizing: "Adding to Clips",
    complete: "Export complete",
    failed: "Export failed",
    cancelled: "Export cancelled",
  } as const)[exportStatus.phase] : null;
  const exportProgress = Math.max(0, Math.min(100, exportStatus?.progressPercent ?? 0));

  return (
    <div className="page editor-page">
      <header className="page-header editor-page-header">
        <div className="editor-title-block">
          <button className="editor-back-button" type="button" onClick={onBackToClips}>← Clips</button>
          <div><h1>Editor</h1><p>{session.source.displayName}</p></div>
        </div>
        <div className="editor-header-status">
          {editorDirty && <span className="editor-dirty-state">Unsaved edits</span>}
          <span className="editor-source-safety">Original protected</span>
          <button
            className="primary-button editor-export-button"
            type="button"
            disabled={exportActive || session.segments.length === 0}
            onClick={() => void startEditorExport()}
          >{exportActive ? "Exporting..." : "Export Clip"}</button>
        </div>
      </header>

      {(exportStatus || exportCommandError) && <section className={`editor-export-status editor-export-status-${exportStatus?.phase ?? "failed"}`} aria-live="polite">
        <div className="editor-export-status-heading">
          <div>
            <strong>{exportPhaseLabel ?? "Could not export clip"}</strong>
            {exportActive && <span>{exportUi.cancellationRequested ? "Stopping the owned FFmpeg process..." : `${exportProgress.toFixed(0)}%`}</span>}
            {exportStatus?.phase === "complete" && <span>{exportStatus.outputDisplayName}</span>}
          </div>
          {exportActive && <button className="secondary-button" type="button" disabled={exportUi.cancellationRequested} onClick={() => void cancelEditorExport()}>{exportUi.cancellationRequested ? "Cancelling..." : "Cancel Export"}</button>}
        </div>
        {exportActive && <div className="editor-export-progress" role="progressbar" aria-label="Editor export progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(exportProgress)}><span style={{ width: `${exportProgress}%` }} /></div>}
        {(exportStatus?.phase === "failed" || exportCommandError) && <p className="editor-export-error" role="alert">{exportCommandError ?? exportStatus?.errorMessage ?? "Could not export clip."}</p>}
        {exportStatus?.phase === "cancelled" && <p>The partial export was removed. The source clip and editable session are unchanged.</p>}
        {exportStatus?.phase === "complete" && <div className="editor-export-success">
          <p>{exportStatus.indexingWarning ?? "The verified H.264 clip is now in your Clips Library."}</p>
          <div>
            {exportStatus.outputClip && <><button className="primary-button" type="button" onClick={() => onPlayExport(exportStatus.outputClip!)}>Play Export</button>
            <button className="secondary-button" type="button" onClick={() => void openExportFolder()}>Open Folder</button></>}
            <button className="secondary-button" type="button" onClick={onBackToClips}>Back to Clips</button>
          </div>
        </div>}
        {exportStatus && <details className="editor-export-diagnostics">
          <summary>Export diagnostics</summary>
          <code>Phase {exportStatus.phase} · planned {exportStatus.plannedDurationUs ? formatEditorTimeUs(exportStatus.plannedDurationUs) : "pending"} · verified {exportStatus.verifiedDurationUs ? formatEditorTimeUs(exportStatus.verifiedDurationUs) : "pending"}</code>
          <code>Encoder {exportStatus.encoder ?? "probing"}{exportStatus.encoderHardware === null ? "" : exportStatus.encoderHardware ? " · hardware" : " · software"}{exportStatus.encoderSettings ? ` · ${exportStatus.encoderSettings}` : ""}</code>
          <code>Attempts {exportStatus.attemptedEncoders.join(" → ") || "pending"}</code>
          {exportStatus.filterPlan && <code className="editor-export-filter-plan">{exportStatus.filterPlan}</code>}
          {exportStatus.diagnostics.map((diagnostic, index) => <code key={index}>{diagnostic}</code>)}
        </details>}
      </section>}

      <div className="editor-workspace">
        <section className="editor-preview-panel" aria-label="Editor video preview">
          <div className="editor-preview-heading"><span>Preview</span><small>{source?.kind ?? "Resolving source"}</small></div>
          <div className="editor-preview-stage">
            {source && <video
              key={`${source.kind}:${source.path}:${source.revision}`}
              ref={videoRef}
              src={source.url}
              controls={!exportActive}
              muted
              playsInline
              preload="metadata"
              aria-label={`Editor preview for ${session.source.displayName}`}
              onLoadStart={() => { pauseAudioFollowers(); setMediaStatus("loading"); updatePlaybackState("loading"); }}
              onLoadedMetadata={(event) => restoreAfterLoad(event.currentTarget)}
              onCanPlay={() => setMediaStatus("ready")}
              onPlay={(event) => {
                const sourceTimeSeconds = event.currentTarget.currentTime;
                editorEndedRef.current = false;
                updatePlaybackState("playing");
                playAudioFollowersAt(sourceTimeSeconds);
                startPlaybackMonitor(event.currentTarget);
              }}
              onPause={() => {
                stopPlaybackMonitor();
                pauseAudioFollowers();
                if (!editorEndedRef.current && !videoRef.current?.ended) updatePlaybackState("paused");
              }}
              onEnded={() => {
                stopPlaybackMonitor();
                pauseAudioFollowers();
                const current = sessionRef.current;
                replaceSession(withEditorPlaybackState(withEditorPlayheadUs(current, totalEditedDurationUs(current.segments)), "ended"));
              }}
              onVolumeChange={(event) => {
                const video = event.currentTarget;
                if (!video.muted) video.muted = true;
              }}
              onTimeUpdate={(event) => {
                const sourceTimeSeconds = event.currentTarget.currentTime;
                synchronizePlaybackPosition(event.currentTarget, sourceTimeSeconds);
              }}
              onSeeking={seekingStarted}
              onSeeked={(event) => seekingFinished(event.currentTarget)}
              onError={playbackFailed}
            />}
            {mediaStatus === "loading" && <div className="editor-media-message"><span className="player-spinner" />Loading source clip...</div>}
            {mediaStatus === "preparingProxy" && <div className="editor-media-message"><span className="player-spinner" />Preparing H.264 editor preview...</div>}
            {mediaStatus === "error" && <div className="editor-media-message editor-media-error" role="alert">
              <strong>Editor preview unavailable</strong><span>{mediaError}</span>
              <div><button type="button" disabled={exportActive} onClick={() => void preparePreview(true)}>Retry Preview</button><button type="button" onClick={onBackToClips}>Return to Clips</button></div>
            </div>}
          </div>
          <div className="editor-transport">
            <button type="button" onClick={togglePlayback} disabled={exportActive || !source || mediaStatus !== "ready"}>{session.playbackState === "playing" ? "Pause" : "Play"}</button>
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

        <section className="editor-mixer-panel" aria-labelledby="editor-mixer-heading">
          <div className="editor-mixer-heading">
            <div><span className="eyebrow">AUDIO MIXER</span><h2 id="editor-mixer-heading">Editor stems</h2></div>
            <div className="editor-mixer-actions">
              <span className={allAudioTracksFailed ? "editor-mixer-status editor-mixer-status-error" : "editor-mixer-status"}>{mixerStatus}</span>
              <button
                type="button"
                onClick={() => updateMixer(resetEditorAudio)}
                disabled={exportActive || !isEditorMixerDirty(mixer)}
                title="Restore all Editor audio tracks to 100%, unmuted, and unsoloed"
              >Reset Audio</button>
            </div>
          </div>

          {mixer.tracks.length > 0 ? <div className="editor-mixer-tracks">
            {mixer.tracks.map((track) => <div className="editor-mixer-track" key={track.id}>
              <div className="editor-mixer-track-name">
                <strong>{track.title}</strong>
                {track.role === "CombinedFallback" && <small>Combined fallback</small>}
                <span className={`editor-audio-availability editor-audio-availability-${track.availability}`}>{track.availability}</span>
              </div>
              <div className="editor-mixer-toggles">
                <button
                  type="button"
                  className={track.muted ? "editor-mixer-toggle active" : "editor-mixer-toggle"}
                  aria-label={`Mute ${track.title}`}
                  aria-pressed={track.muted}
                  title={`Mute ${track.title}`}
                  disabled={exportActive}
                  onClick={() => updateMixer((current) => toggleEditorTrackMute(current, track.id))}
                >M</button>
                <button
                  type="button"
                  className={track.solo ? "editor-mixer-toggle active solo" : "editor-mixer-toggle"}
                  aria-label={`Solo ${track.title}`}
                  aria-pressed={track.solo}
                  title={`Solo ${track.title}`}
                  disabled={exportActive}
                  onClick={() => updateMixer((current) => toggleEditorTrackSolo(current, track.id))}
                >S</button>
              </div>
              <label className={track.gainPercent > 100 ? "editor-mixer-gain amplified" : "editor-mixer-gain"}>
                <span className="visually-hidden">{track.title} volume</span>
                <input
                  type="range"
                  min="0"
                  max="300"
                  step="1"
                  value={track.gainPercent}
                  disabled={exportActive}
                  aria-label={`${track.title} volume`}
                  aria-valuetext={`${track.gainPercent}%`}
                  onChange={(event) => {
                    const gainPercent = Number(event.currentTarget.value);
                    updateMixer((current) => withEditorTrackGain(current, track.id, gainPercent));
                  }}
                />
                <output>{track.gainPercent}%</output>
              </label>
              {track.availability === "error" && <div className="editor-mixer-track-error" role="alert">
                <span>{track.errorMessage ?? "This track is unavailable."}</span>
                <button type="button" disabled={exportActive} onClick={() => void prepareEditorAudioTrack(track, true)}>Retry</button>
              </div>}
            </div>)}
          </div> : <p className="editor-mixer-empty">This clip has no saved audio streams available to the Editor.</p>}

          {allAudioTracksFailed && <p className="editor-mixer-error" role="alert">No Editor audio track could be prepared. Video remains muted so SlickClip does not pretend the stem mix is active or silently substitute Combined audio.</p>}
          {!allAudioTracksFailed && failedAudioTrackCount > 0 && <p className="editor-mixer-warning">{failedAudioTrackCount} track{failedAudioTrackCount === 1 ? " is" : "s are"} unavailable. Ready tracks can still be previewed.</p>}
          {audioRuntimeMessage && <p className="editor-mixer-warning" role="status">{audioRuntimeMessage}</p>}
          <div className="editor-audio-diagnostics" aria-label="Editor audio synchronization diagnostics">
            <span>Video clock authoritative</span>
            <span>Correction threshold {AUDIO_DRIFT_THRESHOLD_MS} ms</span>
            <span>Max drift {audioTelemetry.maxDriftMs.toFixed(1)} ms</span>
            <span>Resyncs {audioTelemetry.resyncCount}</span>
          </div>
        </section>

        <section className="editor-timeline-panel" aria-labelledby="editor-timeline-heading">
          <div className="editor-timeline-heading">
            <div><span className="eyebrow">EDIT DECISION LIST</span><h2 id="editor-timeline-heading">Edited timeline</h2></div>
            <code>{formatEditorTimeUs(session.playheadUs)}</code>
          </div>

          <div className="editor-toolbar" aria-label="Timeline editing actions">
            <button type="button" onClick={() => applyEdit(undoEditorEdit(sessionRef.current))} disabled={exportActive || session.undoStack.length === 0} title="Undo (Ctrl+Z)">Undo</button>
            <button type="button" onClick={() => applyEdit(redoEditorEdit(sessionRef.current))} disabled={exportActive || session.redoStack.length === 0} title="Redo (Ctrl+Shift+Z or Ctrl+Y)">Redo</button>
            <span className="editor-toolbar-divider" aria-hidden="true" />
            <button type="button" onClick={() => applyEdit(splitAtPlayhead(sessionRef.current))} disabled={exportActive || !splitEnabled} title="Split at playhead (S)">Split</button>
            <button className="editor-delete-segment" type="button" onClick={() => applyEdit(deleteSelectedSegment(sessionRef.current))} disabled={exportActive || !deleteEnabled} title="Delete selected segment (Delete)">Delete Segment</button>
            <button type="button" onClick={confirmReset} disabled={exportActive || !session.dirty}>Reset</button>
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
                      aria-disabled={exportActive}
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
                        disabled={exportActive}
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
                        disabled={exportActive}
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
                  disabled={exportActive || !source || authoritativeEditedDurationUs <= 0 || mediaStatus !== "ready" || Boolean(trimDrag)}
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
