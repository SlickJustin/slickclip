import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { ClipActionResponse, ClipListItem, ClipMutationResponse, ClipPlaybackInfo, ClipPlaybackInfoResponse, PrepareClipMediaResponse, UiPreferences, UiPreferencesPatch } from "../types/clips";
import { errorMessage, formatBytes, formatTime } from "../types/clips";
import { clampMediaTime, mediaTimeToPercent, planPlaybackSourceSwitch, playbackIntent, playerShortcut, toggledMuteState, volumePlan } from "../utils/playerControls";
import { addPlayedTime } from "../utils/watchProgress";

type PlayerState = "idle" | "loading" | "playing" | "paused" | "preparingProxy" | "error";
type PlaybackSource = "Master" | "H264 Proxy";
type PlayerIconName = "play" | "pause" | "volume" | "muted" | "fullscreen" | "exitFullscreen";
type Props = {
  clip: ClipListItem;
  preferences: UiPreferences;
  onPreferencesChange: (patch: UiPreferencesPatch) => Promise<void>;
  onClipUpdated: (clip: ClipListItem) => void;
  onCopy: () => void;
  onClose: () => void;
};
type Source = { path: string; kind: PlaybackSource; revision: number };

function PlayerIcon({ name }: { name: PlayerIconName }) {
  if (name === "play") return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m8 5 11 7-11 7z" /></svg>;
  if (name === "pause") return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M7 5h4v14H7zm6 0h4v14h-4z" /></svg>;
  if (name === "muted") return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 9v6h4l5 4V5L8 9zm12.2.2 1.8 1.8 1.8-1.8 1.4 1.4-1.8 1.8 1.8 1.8-1.4 1.4-1.8-1.8-1.8 1.8-1.4-1.4 1.8-1.8-1.8-1.8z" /></svg>;
  if (name === "volume") return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 9v6h4l5 4V5L8 9zm11.5-.5a5 5 0 0 1 0 7l1.4 1.4a7 7 0 0 0 0-9.8z" /></svg>;
  if (name === "exitFullscreen") return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M9 4v5H4v2h7V4zm6 0h-2v7h7V9h-5zm-4 9H4v2h5v5h2zm9 0h-7v7h2v-5h5z" /></svg>;
  return <svg viewBox="0 0 24 24" aria-hidden="true"><path d="M4 4h7v2H6v5H4zm9 0h7v7h-2V6h-5zM4 13h2v5h5v2H4zm14 0h2v7h-7v-2h5z" /></svg>;
}

export function ClipPlayer({ clip, preferences, onPreferencesChange, onClipUpdated, onCopy, onClose }: Props) {
  const [info, setInfo] = useState<ClipPlaybackInfo | null>(null);
  const [source, setSource] = useState<Source | null>(null);
  const [playerState, setPlayerState] = useState<PlayerState>("idle");
  const [currentTime, setCurrentTime] = useState(0);
  const [duration, setDuration] = useState(clip.duration100ns / 10_000_000);
  const [volume, setVolume] = useState(preferences.playerVolume);
  const [muted, setMuted] = useState(preferences.playerMuted);
  const [bufferedPercent, setBufferedPercent] = useState(0);
  const [controlsVisible, setControlsVisible] = useState(true);
  const [isFullscreen, setIsFullscreen] = useState(false);
  const [directAttempt, setDirectAttempt] = useState<"pending" | "success" | "error">("pending");
  const [directError, setDirectError] = useState<string | null>(null);
  const [generation, setGeneration] = useState<PrepareClipMediaResponse | null>(null);
  const [playerError, setPlayerError] = useState<string | null>(null);
  const videoRef = useRef<HTMLVideoElement>(null);
  const modalRef = useRef<HTMLDivElement>(null);
  const mountedRef = useRef(true);
  const generationToken = useRef(0);
  const fallbackStarted = useRef(false);
  const pendingRestore = useRef({ time: 0, play: false });
  const sourceRevision = useRef(0);
  const seekWatchdog = useRef<number | undefined>(undefined);
  const controlsTimer = useRef<number | undefined>(undefined);
  const currentTimeRef = useRef(0);
  const durationRef = useRef(duration);
  const volumeRef = useRef(preferences.playerVolume);
  const mutedRef = useRef(preferences.playerMuted);
  const lastAudibleVolume = useRef(preferences.playerLastAudibleVolume);
  const volumePreferenceTimer = useRef<number | undefined>(undefined);
  const watchedSeconds = useRef(0);
  const watchCounted = useRef(false);
  const lastPlaybackTime = useRef<number | null>(null);

  const persistPlayerState = useCallback((delayMs = 0) => {
    if (volumePreferenceTimer.current !== undefined) window.clearTimeout(volumePreferenceTimer.current);
    volumePreferenceTimer.current = window.setTimeout(() => {
      volumePreferenceTimer.current = undefined;
      void onPreferencesChange({
        playerVolume: volumeRef.current,
        playerMuted: mutedRef.current,
        playerLastAudibleVolume: lastAudibleVolume.current,
      });
    }, delayMs);
  }, [onPreferencesChange]);

  const clearControlsTimer = useCallback(() => {
    if (controlsTimer.current !== undefined) window.clearTimeout(controlsTimer.current);
    controlsTimer.current = undefined;
  }, []);

  const showControls = useCallback(() => {
    clearControlsTimer();
    setControlsVisible(true);
    if (videoRef.current && !videoRef.current.paused) controlsTimer.current = window.setTimeout(() => setControlsVisible(false), 2_500);
  }, [clearControlsTimer]);

  const updateBuffered = useCallback(() => {
    const video = videoRef.current;
    if (!video || !Number.isFinite(video.duration) || video.duration <= 0 || video.buffered.length === 0) return setBufferedPercent(0);
    setBufferedPercent(mediaTimeToPercent(video.buffered.end(video.buffered.length - 1), video.duration));
  }, []);

  const seekTo = useCallback((requestedTime: number) => {
    const video = videoRef.current;
    const nextTime = clampMediaTime(requestedTime, video?.duration || durationRef.current);
    if (video) video.currentTime = nextTime;
    currentTimeRef.current = nextTime;
    setCurrentTime(nextTime);
    showControls();
  }, [showControls]);

  const togglePlayback = useCallback(() => {
    const video = videoRef.current;
    if (!video) return;
    if (playbackIntent(video.paused) === "play") void video.play().catch(() => setPlayerState("paused"));
    else video.pause();
    showControls();
  }, [showControls]);

  const setPlayerVolume = useCallback((requestedVolume: number) => {
    const next = volumePlan(requestedVolume);
    volumeRef.current = next.volume;
    mutedRef.current = next.muted;
    if (next.volume > 0) lastAudibleVolume.current = next.volume;
    setVolume(next.volume);
    setMuted(next.muted);
    if (videoRef.current) { videoRef.current.volume = next.volume; videoRef.current.muted = next.muted; }
    persistPlayerState(220);
    showControls();
  }, [persistPlayerState, showControls]);

  const toggleMute = useCallback(() => {
    let nextMuted = toggledMuteState(mutedRef.current, volumeRef.current);
    if (volumeRef.current <= 0 && mutedRef.current) {
      const restoredVolume = lastAudibleVolume.current || 0.65;
      volumeRef.current = restoredVolume;
      setVolume(restoredVolume);
      nextMuted = false;
      if (videoRef.current) videoRef.current.volume = restoredVolume;
    }
    mutedRef.current = nextMuted;
    setMuted(nextMuted);
    if (videoRef.current) videoRef.current.muted = nextMuted;
    persistPlayerState();
    showControls();
  }, [persistPlayerState, showControls]);

  const toggleFullscreen = useCallback(() => {
    if (document.fullscreenElement) void document.exitFullscreen();
    else void modalRef.current?.requestFullscreen();
    showControls();
  }, [showControls]);

  useEffect(() => {
    mountedRef.current = true;
    setPlayerState("loading");
    void invoke<ClipPlaybackInfoResponse>("get_clip_playback_info", { request: { clipId: clip.id } })
      .then((response) => {
        if (!mountedRef.current) return;
        if (!response.success || !response.info) throw new Error(response.errorMessage ?? "The clip is unavailable for playback.");
        setInfo(response.info);
        const mediaDuration = response.info.duration100ns / 10_000_000;
        durationRef.current = mediaDuration;
        setDuration(mediaDuration);
        setSource({ path: response.info.masterPath, kind: "Master", revision: ++sourceRevision.current });
      })
      .catch((cause) => { if (mountedRef.current) { setPlayerError(errorMessage(cause)); setPlayerState("error"); } });
    return () => {
      mountedRef.current = false;
      generationToken.current += 1;
      if (seekWatchdog.current !== undefined) window.clearTimeout(seekWatchdog.current);
      if (volumePreferenceTimer.current !== undefined) window.clearTimeout(volumePreferenceTimer.current);
      void onPreferencesChange({
        playerVolume: volumeRef.current,
        playerMuted: mutedRef.current,
        playerLastAudibleVolume: lastAudibleVolume.current,
      });
      clearControlsTimer();
    };
  }, [clearControlsTimer, clip.id, onPreferencesChange]);

  const prepareMedia = useCallback(async (retry = false) => {
    if (seekWatchdog.current !== undefined) window.clearTimeout(seekWatchdog.current);
    seekWatchdog.current = undefined;
    const video = videoRef.current;
    const switchPlan = planPlaybackSourceSwitch(video?.currentTime ?? currentTimeRef.current, video?.duration || durationRef.current, video?.paused ?? true, volumeRef.current, mutedRef.current);
    const token = ++generationToken.current;
    setPlayerState("preparingProxy");
    setPlayerError(null);
    showControls();
    while (mountedRef.current && generationToken.current === token) {
      try {
        const request = { clipId: clip.id, retry, currentTimeSeconds: switchPlan.restoreAtSeconds, wasPlaying: switchPlan.resumePlaying };
        const response = await invoke<PrepareClipMediaResponse>("prepare_clip_preview", { request });
        retry = false;
        if (!mountedRef.current || generationToken.current !== token) return;
        setGeneration(response);
        if (response.artifact.state === "ready" && response.artifact.filePath) {
          pendingRestore.current = { time: response.restoreAtSeconds, play: response.resumePlaying };
          setSource({ path: response.artifact.filePath, kind: "H264 Proxy", revision: ++sourceRevision.current });
          setPlayerState("loading");
          return;
        }
        if (response.artifact.state === "error" || !response.success) {
          setPlayerError(response.errorMessage ?? response.artifact.errorMessage ?? "Preview generation failed.");
          setPlayerState("error");
          return;
        }
        await new Promise((resolve) => window.setTimeout(resolve, 800));
      } catch (cause) {
        if (!mountedRef.current || generationToken.current !== token) return;
        setPlayerError(errorMessage(cause));
        setPlayerState("error");
        return;
      }
    }
  }, [clip.id, showControls]);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const target = event.target as HTMLElement | null;
      const shortcut = playerShortcut(event.key, event.code, target?.tagName, target?.isContentEditable ?? false);
      if (!shortcut) return;
      showControls();
      if (shortcut === "togglePlayback") { event.preventDefault(); togglePlayback(); }
      else if (shortcut === "seekBackward") { event.preventDefault(); seekTo((videoRef.current?.currentTime ?? currentTimeRef.current) - 5); }
      else if (shortcut === "seekForward") { event.preventDefault(); seekTo((videoRef.current?.currentTime ?? currentTimeRef.current) + 5); }
      else if (shortcut === "toggleMute") toggleMute();
      else if (shortcut === "toggleFullscreen") toggleFullscreen();
      else if (shortcut === "escape") { if (document.fullscreenElement) void document.exitFullscreen(); else onClose(); }
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, [onClose, seekTo, showControls, toggleFullscreen, toggleMute, togglePlayback]);

  useEffect(() => {
    function onFullscreenChange() { setIsFullscreen(document.fullscreenElement === modalRef.current); showControls(); }
    document.addEventListener("fullscreenchange", onFullscreenChange);
    return () => document.removeEventListener("fullscreenchange", onFullscreenChange);
  }, [showControls]);

  useEffect(() => {
    if (playerState === "playing") showControls();
    else { clearControlsTimer(); setControlsVisible(true); }
  }, [clearControlsTimer, playerState, showControls]);

  function restoreAfterLoad() {
    const video = videoRef.current;
    if (!video) return;
    video.volume = volumeRef.current;
    video.muted = mutedRef.current;
    const restore = pendingRestore.current;
    if (restore.time > 0) video.currentTime = Math.min(restore.time, video.duration || restore.time);
    if (restore.play) void video.play().catch(() => setPlayerState("paused"));
    else setPlayerState("paused");
    pendingRestore.current = { time: 0, play: false };
    updateBuffered();
  }

  function directPlaybackReady() {
    if (source?.kind === "Master") { setDirectAttempt("success"); setDirectError(null); }
    if (videoRef.current?.paused) setPlayerState("paused");
    updateBuffered();
  }

  function playbackFailed() {
    const mediaError = videoRef.current?.error;
    const message = mediaError ? `WebView playback failed (${mediaError.code}): ${mediaError.message || "unsupported media or decode failure"}` : "WebView playback failed for this media source.";
    if (source?.kind === "Master" && !fallbackStarted.current) {
      fallbackStarted.current = true; setDirectAttempt("error"); setDirectError(message); void prepareMedia(); return;
    }
    setPlayerError(message); setPlayerState("error");
  }

  function seekingStarted() {
    lastPlaybackTime.current = null;
    if (source?.kind !== "Master") return;
    if (seekWatchdog.current !== undefined) window.clearTimeout(seekWatchdog.current);
    seekWatchdog.current = window.setTimeout(() => {
      fallbackStarted.current = true;
      setDirectAttempt("error");
      setDirectError("Direct master seeking did not complete promptly; switching to an H.264 preview.");
      void prepareMedia();
    }, 2_000);
  }

  function seekingFinished() {
    if (seekWatchdog.current !== undefined) window.clearTimeout(seekWatchdog.current);
    seekWatchdog.current = undefined;
    lastPlaybackTime.current = videoRef.current?.currentTime ?? null;
  }

  function trackMeaningfulPlayback(video: HTMLVideoElement, allowEnded = false) {
    const current = video.currentTime;
    const previous = lastPlaybackTime.current;
    lastPlaybackTime.current = current;
    if (watchCounted.current || (!allowEnded && video.paused) || video.seeking || previous === null) return;
    const elapsed = current - previous;
    if (elapsed <= 0) return;
    const progress = addPlayedTime(watchedSeconds.current, elapsed, video.duration || durationRef.current, false);
    watchedSeconds.current = progress.accumulatedSeconds;
    if (!progress.reachedThreshold) return;
    watchCounted.current = true;
    void invoke<ClipMutationResponse>("record_clip_watch_command", { request: { clipId: clip.id } })
      .then((response) => {
        if (response.success && response.clip) onClipUpdated(response.clip);
        else console.warn("SlickClip watch metadata update failed:", response.errorMessage);
      })
      .catch((cause) => console.warn("SlickClip watch metadata update failed:", cause));
  }

  async function trustedAction(command: "open_clip_file" | "open_clip_folder") {
    const response = await invoke<ClipActionResponse>(command, { request: { clipId: clip.id } });
    if (!response.success) { setPlayerError(response.errorMessage ?? "The requested clip action failed."); setPlayerState("error"); }
  }

  function retryPreparation() {
    fallbackStarted.current = true;
    void prepareMedia(true);
  }

  const seekPercent = mediaTimeToPercent(currentTime, duration);
  const seekStyle = { "--played": `${seekPercent}%`, "--buffered": `${Math.max(bufferedPercent, seekPercent)}%` } as CSSProperties;
  const volumeStyle = { "--played": `${muted ? 0 : volume * 100}%` } as CSSProperties;

  return (
    <div className="clip-player-backdrop" role="presentation" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
      <div className="clip-player-modal" ref={modalRef} role="dialog" aria-modal="true" aria-label={`Playing ${clip.displayName}`}>
        <header className="clip-player-header">
          <div><span>SlickClip / Clip Player</span><h2>{clip.displayName}</h2></div>
          <button type="button" onClick={onClose} aria-label="Close player">×</button>
        </header>

        <div className="clip-player-stage" onMouseMove={showControls} onMouseEnter={showControls} onClick={(event) => { if (event.target === event.currentTarget || event.target === videoRef.current) togglePlayback(); }}>
          {source && <video
            key={`${source.kind}:${source.path}:${source.revision}`}
            ref={videoRef}
            src={`${convertFileSrc(source.path)}?v=${source.revision}`}
            controls={false}
            playsInline
            preload="metadata"
            onLoadStart={() => setPlayerState("loading")}
            onLoadedMetadata={(event) => { const mediaDuration = event.currentTarget.duration || durationRef.current; durationRef.current = mediaDuration; setDuration(mediaDuration); restoreAfterLoad(); }}
            onCanPlay={directPlaybackReady}
            onPlay={(event) => { lastPlaybackTime.current = event.currentTarget.currentTime; setPlayerState("playing"); }}
            onPause={(event) => { trackMeaningfulPlayback(event.currentTarget, true); lastPlaybackTime.current = null; setPlayerState("paused"); }}
            onEnded={(event) => { trackMeaningfulPlayback(event.currentTarget, true); lastPlaybackTime.current = null; setPlayerState("paused"); }}
            onTimeUpdate={(event) => { currentTimeRef.current = event.currentTarget.currentTime; setCurrentTime(event.currentTarget.currentTime); trackMeaningfulPlayback(event.currentTarget); }}
            onDurationChange={(event) => { if (Number.isFinite(event.currentTarget.duration)) { durationRef.current = event.currentTarget.duration; setDuration(event.currentTarget.duration); } }}
            onProgress={updateBuffered}
            onSeeking={seekingStarted}
            onSeeked={seekingFinished}
            onVolumeChange={(event) => { volumeRef.current = event.currentTarget.volume; mutedRef.current = event.currentTarget.muted; setVolume(event.currentTarget.volume); setMuted(event.currentTarget.muted); }}
            onError={playbackFailed}
          />}
          {!source && playerState !== "error" && <div className="clip-player-message">Loading SlickClip media...</div>}
          {playerState === "preparingProxy" && <div className="clip-player-message"><span className="player-spinner" />Preparing H.264 Preview...</div>}
          {playerState === "error" && <div className="clip-player-message clip-player-error">
            <strong>Playback unavailable</strong><span>{playerError}</span>
            <div><button type="button" onClick={retryPreparation}>Retry Preview</button><button type="button" onClick={() => void trustedAction("open_clip_file")}>Open Externally</button></div>
          </div>}

          {source && playerState !== "error" && <div className={`slick-player-controls${playerState === "playing" && !controlsVisible ? " controls-hidden" : ""}`} onClick={(event) => event.stopPropagation()}>
            <label className="player-seek-control">
              <span className="visually-hidden">Seek through clip</span>
              <input className="player-range player-seek-range" type="range" min="0" max={duration || 0} step="0.01" value={Math.min(currentTime, duration || 0)} style={seekStyle} aria-label="Seek through clip" aria-valuetext={`${formatTime(currentTime)} of ${formatTime(duration)}`} onChange={(event) => seekTo(Number(event.target.value))} />
            </label>
            <div className="player-control-row">
              <button className="player-icon-button player-play-button" type="button" onClick={togglePlayback} aria-label={playerState === "playing" ? "Pause" : "Play"}><PlayerIcon name={playerState === "playing" ? "pause" : "play"} /></button>
              <div className="player-time"><span>{formatTime(currentTime)}</span><span>/</span><span>{formatTime(duration)}</span></div>
              <div className="player-volume-control">
                <button className="player-icon-button" type="button" onClick={toggleMute} aria-label={muted ? "Unmute" : "Mute"} aria-pressed={muted}><PlayerIcon name={muted || volume === 0 ? "muted" : "volume"} /></button>
                <input className="player-range player-volume-range" type="range" min="0" max="1" step="0.01" value={volume} style={volumeStyle} aria-label="Volume" onChange={(event) => setPlayerVolume(Number(event.target.value))} />
              </div>
              <span className="player-control-spacer" />
              <button className="player-icon-button" type="button" onClick={toggleFullscreen} aria-label={isFullscreen ? "Exit fullscreen" : "Enter fullscreen"}><PlayerIcon name={isFullscreen ? "exitFullscreen" : "fullscreen"} /></button>
            </div>
          </div>}
        </div>

        <div className="clip-player-secondary-actions">
          <button type="button" onClick={onCopy}>Copy Clip</button>
          <button type="button" onClick={() => void prepareMedia()}>Use H.264 Preview</button>
          <button type="button" onClick={() => void trustedAction("open_clip_file")}>Open Externally</button>
          <button type="button" onClick={() => void trustedAction("open_clip_folder")}>Open Folder</button>
        </div>

        <details className="clip-player-diagnostics">
          <summary>Playback diagnostics</summary>
          <div><span>Clip ID</span><code>{clip.id}</code></div>
          <div><span>Master</span><code>{info?.masterPath ?? "Loading..."}</code></div>
          <div><span>Master codec</span><code>{info?.masterCodec ?? clip.videoCodec}</code></div>
          <div><span>Playback source</span><code>{source?.kind ?? "None"}</code></div>
          <div><span>Resolution</span><code>{source?.kind === "Master" ? (info ? `${info.width}×${info.height}` : `${clip.width}×${clip.height}`) : "Up to 1920×1080"}</code></div>
          <div><span>Duration</span><code>{formatTime(duration)}</code></div>
          <div><span>Direct attempt</span><code>{directAttempt}{directError ? ` — ${directError}` : ""}</code></div>
          <div><span>Proxy</span><code>{generation?.artifact.state ?? info?.preview.state ?? "unknown"}{(generation?.artifact.fileSizeBytes ?? info?.preview.fileSizeBytes) ? ` · ${formatBytes((generation?.artifact.fileSizeBytes ?? info?.preview.fileSizeBytes)!)}` : ""}{(generation?.artifact.bitrateBps ?? info?.preview.bitrateBps) ? ` · ${((generation?.artifact.bitrateBps ?? info?.preview.bitrateBps)! / 1_000_000).toFixed(2)} Mbps` : ""}{(generation?.artifact.generationDurationMs ?? info?.preview.generationDurationMs) ? ` · ${(generation?.artifact.generationDurationMs ?? info?.preview.generationDurationMs)!.toFixed(0)} ms` : ""}</code></div>
          <div><span>Thumbnail</span><code>{info?.thumbnail.state ?? "unknown"}{info?.thumbnail.fileSizeBytes ? ` · ${formatBytes(info.thumbnail.fileSizeBytes)}` : ""}{info?.thumbnail.generationDurationMs ? ` · ${info.thumbnail.generationDurationMs.toFixed(0)} ms` : ""}</code></div>
          <div><span>Cache root</span><code>{info?.cacheRoot ?? "Loading..."}</code></div>
          <div><span>Player state</span><code>{playerState}</code></div>
        </details>
      </div>
    </div>
  );
}
