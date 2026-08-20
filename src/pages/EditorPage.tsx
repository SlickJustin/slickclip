import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { ClipListItem, ClipPlaybackInfo, ClipPlaybackInfoResponse, PrepareClipMediaResponse } from "../types/clips";
import { audioLabel, errorMessage } from "../types/clips";
import { clampMediaTime, mediaTimeToPercent } from "../utils/playerControls";
import {
  createEditorSession,
  formatEditorTime,
  resetEditorSession,
  timelinePositionToSeconds,
  timelineTickTimes,
  withEditorDuration,
  withEditorPlaybackState,
  withEditorPlayhead,
  type EditorPlaybackState,
  type EditorSession,
} from "../utils/editorSession";

type EditorMediaStatus = "loading" | "ready" | "preparingProxy" | "error";
type EditorMediaSource = { path: string; url: string; kind: "Master" | "H264 Proxy"; revision: number };
type Props = { clip: ClipListItem | null; onBackToClips: () => void };

export function EditorPage({ clip, onBackToClips }: Props) {
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

  return <ActiveEditor key={clip.id} clip={clip} onBackToClips={onBackToClips} />;
}

function ActiveEditor({ clip, onBackToClips }: { clip: ClipListItem; onBackToClips: () => void }) {
  const [session, setSession] = useState<EditorSession>(() => createEditorSession(clip));
  const [playbackInfo, setPlaybackInfo] = useState<ClipPlaybackInfo | null>(null);
  const [source, setSource] = useState<EditorMediaSource | null>(null);
  const [mediaStatus, setMediaStatus] = useState<EditorMediaStatus>("loading");
  const [mediaError, setMediaError] = useState<string | null>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const mountedRef = useRef(true);
  const operationTokenRef = useRef(0);
  const sourceRevisionRef = useRef(0);
  const fallbackStartedRef = useRef(false);
  const seekWatchdogRef = useRef<number | undefined>(undefined);
  const pendingRestoreRef = useRef({ time: 0, play: false });
  const sessionRef = useRef(session);

  useEffect(() => { sessionRef.current = session; }, [session]);

  const updatePlaybackState = useCallback((state: EditorPlaybackState) => {
    setSession((current) => withEditorPlaybackState(current, state));
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
    setSession(resetEditorSession(clip));
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
        setSession((current) => withEditorDuration(current, response.info!.duration100ns / 10_000_000));
        useMediaPath(response.info.masterPath, "Master");
      })
      .catch((cause) => {
        if (mountedRef.current && operationTokenRef.current === token) failMedia(errorMessage(cause));
      });

    return () => {
      mountedRef.current = false;
      operationTokenRef.current += 1;
      if (seekWatchdogRef.current !== undefined) window.clearTimeout(seekWatchdogRef.current);
      seekWatchdogRef.current = undefined;
      videoRef.current?.pause();
    };
  }, [clip, failMedia, useMediaPath]);

  const preparePreview = useCallback(async (retry = false) => {
    if (seekWatchdogRef.current !== undefined) window.clearTimeout(seekWatchdogRef.current);
    seekWatchdogRef.current = undefined;
    const video = videoRef.current;
    const duration = sessionRef.current.source.durationSeconds;
    const restoreAtSeconds = clampMediaTime(video?.currentTime ?? sessionRef.current.playheadSeconds, duration);
    const resumePlaying = video ? !video.paused && !video.ended : sessionRef.current.playbackState === "playing";
    const token = ++operationTokenRef.current;
    setMediaError(null);
    setMediaStatus("preparingProxy");
    updatePlaybackState("loading");

    while (mountedRef.current && operationTokenRef.current === token) {
      try {
        const response = await invoke<PrepareClipMediaResponse>("prepare_clip_preview", {
          request: { clipId: clip.id, retry, currentTimeSeconds: restoreAtSeconds, wasPlaying: resumePlaying },
        });
        retry = false;
        if (!mountedRef.current || operationTokenRef.current !== token) return;
        if (response.artifact.state === "ready" && response.artifact.filePath) {
          pendingRestoreRef.current = { time: response.restoreAtSeconds, play: response.resumePlaying };
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

  function restoreAfterLoad(video: HTMLVideoElement) {
    const duration = Number.isFinite(video.duration) && video.duration > 0
      ? video.duration
      : sessionRef.current.source.durationSeconds;
    setSession((current) => withEditorDuration(current, duration));
    const restore = pendingRestoreRef.current;
    const nextTime = clampMediaTime(restore.time, duration);
    video.currentTime = nextTime;
    setSession((current) => withEditorPlayhead(current, nextTime));
    pendingRestoreRef.current = { time: 0, play: false };
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

  function seekingStarted(video: HTMLVideoElement) {
    setSession((current) => withEditorPlayhead(current, video.currentTime));
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
    setSession((current) => withEditorPlayhead(current, video.currentTime));
  }

  function seekFromTimeline(position: number) {
    const nextTime = timelinePositionToSeconds(position, 1, session.source.durationSeconds);
    if (videoRef.current) videoRef.current.currentTime = nextTime;
    setSession((current) => withEditorPlayhead(current, nextTime));
  }

  function togglePlayback() {
    const video = videoRef.current;
    if (!video || mediaStatus === "error" || mediaStatus === "preparingProxy") return;
    if (video.paused || video.ended) void video.play().catch((cause) => failMedia(errorMessage(cause)));
    else video.pause();
  }

  const duration = session.source.durationSeconds;
  const timelineFraction = duration > 0 ? session.playheadSeconds / duration : 0;
  const timelinePercent = mediaTimeToPercent(session.playheadSeconds, duration);
  const timelineStyle = { "--editor-playhead": `${timelinePercent}%` } as CSSProperties;
  const audioTracks = playbackInfo?.audioTracks ?? session.source.audioTracks;

  return (
    <div className="page editor-page">
      <header className="page-header editor-page-header">
        <div className="editor-title-block">
          <button className="editor-back-button" type="button" onClick={onBackToClips}>← Clips</button>
          <div><h1>Editor</h1><p>{session.source.displayName}</p></div>
        </div>
        <span className="editor-source-safety">Original protected</span>
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
              onPlay={() => updatePlaybackState("playing")}
              onPause={() => { if (!videoRef.current?.ended) updatePlaybackState("paused"); }}
              onEnded={() => updatePlaybackState("ended")}
              onTimeUpdate={(event) => {
                const currentTime = event.currentTarget.currentTime;
                setSession((current) => withEditorPlayhead(current, currentTime));
              }}
              onDurationChange={(event) => {
                const mediaDuration = event.currentTarget.duration;
                if (Number.isFinite(mediaDuration) && mediaDuration > 0) {
                  setSession((current) => withEditorDuration(current, mediaDuration));
                }
              }}
              onSeeking={(event) => seekingStarted(event.currentTarget)}
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
            <code>{formatEditorTime(session.playheadSeconds)} / {formatEditorTime(duration)}</code>
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
            <div><dt>Duration</dt><dd>{formatEditorTime(duration)}</dd></div>
          </dl>
          <div className="editor-source-audio"><span>Saved audio tracks</span><div>{audioTracks.length > 0 ? audioTracks.map((track) => <span key={track.streamIndex}>{audioLabel(track)}</span>) : <small>No audio tracks</small>}</div></div>
          <div className="editor-safety-note"><strong>Non-destructive session</strong><span>The source MP4 and library metadata remain unchanged.</span></div>
        </aside>

        <section className="editor-timeline-panel" aria-labelledby="editor-timeline-heading">
          <div className="editor-timeline-heading">
            <div><span className="eyebrow">WORK AREA</span><h2 id="editor-timeline-heading">Source timeline</h2></div>
            <code>{formatEditorTime(session.playheadSeconds)}</code>
          </div>
          <div className="editor-timeline-ruler" aria-hidden="true">
            {timelineTickTimes(duration).map((time, index) => <span key={index}>{formatEditorTime(time)}</span>)}
          </div>
          <div className="editor-timeline-row">
            <span className="editor-track-label">SOURCE</span>
            <label className="editor-timeline-lane" style={timelineStyle}>
              <span className="visually-hidden">Seek editor timeline</span>
              <span className="editor-source-clip"><span>{session.source.displayName}</span></span>
              <span className="editor-timeline-progress" />
              <span className="editor-timeline-playhead" />
              <input type="range" min="0" max="1" step="0.0001" value={timelineFraction} disabled={!source || duration <= 0 || mediaStatus !== "ready"} aria-label="Editor timeline playhead" aria-valuetext={`${formatEditorTime(session.playheadSeconds)} of ${formatEditorTime(duration)}`} onChange={(event) => seekFromTimeline(Number(event.target.value))} />
            </label>
          </div>
          <div className="editor-timeline-footer"><span>Editable range: {formatEditorTime(session.editableRange.startSeconds)} – {formatEditorTime(session.editableRange.endSeconds)}</span><span>Click or drag the timeline to seek.</span></div>
        </section>
      </div>
    </div>
  );
}
