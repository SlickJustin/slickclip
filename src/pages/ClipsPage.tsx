import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ClipPlayer } from "../components/ClipPlayer";
import { ClipThumbnail } from "../components/ClipThumbnail";
import type {
  ClipActionResponse,
  ClipListItem,
  ClipListResponse,
  ClipMutationResponse,
  ClipSortOrder,
  LibraryTelemetry,
  ReconcileResponse,
  ReconciliationTelemetry,
} from "../types/clips";
import { audioLabel, errorMessage, formatBytes, formatDuration100ns, formatFps } from "../types/clips";

export function ClipsPage() {
  const [search, setSearch] = useState("");
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [sortOrder, setSortOrder] = useState<ClipSortOrder>("newestFirst");
  const [clips, setClips] = useState<ClipListItem[]>([]);
  const [totalCount, setTotalCount] = useState(0);
  const [telemetry, setTelemetry] = useState<LibraryTelemetry | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshResult, setRefreshResult] = useState<ReconciliationTelemetry | null>(null);
  const [playingClip, setPlayingClip] = useState<ClipListItem | null>(null);
  const reconciliationActive = refreshing || telemetry?.reconciliationRunning === true;

  const loadClips = useCallback(async () => {
    try {
      const response = await invoke<ClipListResponse>("list_clips", {
        request: { searchText: search, favoritesOnly, sortOrder, limit: 100, offset: 0 },
      });
      setTelemetry(response.telemetry);
      if (!response.success) throw new Error(response.errorMessage ?? "The Clips library is unavailable.");
      setClips(response.clips);
      setTotalCount(response.totalCount);
      setError(null);
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setLoading(false);
    }
  }, [favoritesOnly, search, sortOrder]);

  useEffect(() => {
    const timer = window.setTimeout(() => void loadClips(), 180);
    return () => window.clearTimeout(timer);
  }, [loadClips]);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    void listen<string>("clip-library-changed", () => void loadClips()).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [loadClips]);

  async function refreshLibrary() {
    if (refreshing) return;
    setRefreshing(true);
    setError(null);
    try {
      const response = await invoke<ReconcileResponse>("refresh_clip_library");
      setTelemetry(response.telemetry);
      if (!response.success) throw new Error(response.errorMessage ?? "Clip reconciliation failed.");
      setRefreshResult(response.result);
      await loadClips();
    } catch (cause) {
      setError(errorMessage(cause));
    } finally {
      setRefreshing(false);
    }
  }

  async function setFavorite(clip: ClipListItem) {
    const response = await invoke<ClipMutationResponse>("set_clip_favorite", { request: { clipId: clip.id, favorite: !clip.favorite } });
    if (!response.success || !response.clip) return setError(response.errorMessage ?? "Favorite update failed.");
    replaceClip(response.clip);
  }

  async function renameClip(clip: ClipListItem) {
    const name = window.prompt("Library display name (leave empty to restore the filename-derived name):", clip.displayName);
    if (name === null) return;
    const response = await invoke<ClipMutationResponse>("rename_clip_display_name", { request: { clipId: clip.id, displayName: name } });
    if (!response.success || !response.clip) return setError(response.errorMessage ?? "Clip rename failed.");
    replaceClip(response.clip);
  }

  async function clipAction(command: "open_clip_file" | "open_clip_folder", clip: ClipListItem) {
    const response = await invoke<ClipActionResponse>(command, { request: { clipId: clip.id } });
    if (!response.success) setError(response.errorMessage ?? "The clip action failed.");
  }

  async function deleteClip(clip: ClipListItem) {
    if (!window.confirm(`Permanently delete “${clip.displayName}”?\n\nThis deletes the MP4 from disk and cannot be undone.`)) return;
    const response = await invoke<ClipActionResponse>("delete_clip", { request: { clipId: clip.id } });
    if (!response.success) return setError(response.errorMessage ?? "Clip deletion failed.");
    setClips((current) => current.filter((value) => value.id !== clip.id));
    setTotalCount((current) => Math.max(0, current - 1));
  }

  function replaceClip(updated: ClipListItem) {
    setClips((current) => current.map((clip) => clip.id === updated.id ? updated : clip));
    setError(null);
  }

  return (
    <div className="page">
      <header className="page-header clips-page-header">
        <div><h1>Clips</h1><p>Your persistent local replay library.</p></div>
        <button className="secondary-button" type="button" disabled={reconciliationActive} onClick={refreshLibrary}>{reconciliationActive ? "Refreshing..." : "Refresh Clips"}</button>
      </header>

      <section className="clips-panel" aria-label="Clips library">
        <div className="clips-toolbar">
          <label className="search-field">
            <span className="visually-hidden">Search clips</span>
            <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m20 20-4-4" /></svg>
            <input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search names, files, or capture targets..." />
          </label>
          <label className="favorites-filter"><input type="checkbox" checked={favoritesOnly} onChange={(event) => setFavoritesOnly(event.target.checked)} />Favorites</label>
          <label>
            <span className="visually-hidden">Sort clips</span>
            <select value={sortOrder} onChange={(event) => setSortOrder(event.target.value as ClipSortOrder)}>
              <option value="newestFirst">Newest First</option><option value="oldestFirst">Oldest First</option><option value="nameAscending">Name A–Z</option>
            </select>
          </label>
        </div>

        {error && <div className="clips-library-error" role="alert"><span>{error}</span><button type="button" onClick={refreshLibrary}>Retry</button></div>}
        {refreshResult && <div className="clips-refresh-result" role="status">Scanned {refreshResult.scannedFiles} · unchanged {refreshResult.unchanged} · added {refreshResult.added} · updated {refreshResult.updated} · removed {refreshResult.removed} · failed {refreshResult.failed} · {refreshResult.durationMs.toFixed(1)} ms</div>}

        {loading ? <LibraryState title="Loading clips..." detail="Reading the local Clips database." /> : clips.length === 0 && !error ? <LibraryState title="No clips yet" detail="Save a replay and it will appear here." /> : (
          <div className="clips-library-grid">
            {clips.map((clip) => (
              <article className="clip-card" key={clip.id}>
                <ClipThumbnail clip={clip} onPlay={() => setPlayingClip(clip)} />
                <div className="clip-card-body">
                  <div className="clip-card-heading">
                    <div><button className="clip-title-button" type="button" onClick={() => setPlayingClip(clip)}>{clip.displayName}</button><small>{new Date(clip.createdAtMs).toLocaleString()}</small></div>
                    <button className={`favorite-button${clip.favorite ? " active" : ""}`} type="button" aria-label={clip.favorite ? "Remove favorite" : "Add favorite"} onClick={() => void setFavorite(clip)}>{clip.favorite ? "\u2605" : "\u2606"}</button>
                  </div>
                  <div className="clip-card-facts"><span>{formatDuration100ns(clip.duration100ns)}</span><span>{formatBytes(clip.fileSizeBytes)}</span><span>{formatFps(clip.fpsNumerator, clip.fpsDenominator)} FPS</span>{clip.captureTargetLabel && <span>{clip.captureTargetLabel}</span>}</div>
                  {clip.audioTracks.length > 0 && <div className="clip-audio-badges">{clip.audioTracks.map((track) => <span key={track.streamIndex}>{audioLabel(track)}</span>)}</div>}
                  <div className="clip-card-actions">
                    <button className="clip-play-button" type="button" onClick={() => setPlayingClip(clip)}>▶ Play</button><button type="button" onClick={() => void clipAction("open_clip_file", clip)}>Open Externally</button><button type="button" onClick={() => void clipAction("open_clip_folder", clip)}>Folder</button><button type="button" onClick={() => void renameClip(clip)}>Rename</button><button className="danger" type="button" onClick={() => void deleteClip(clip)}>Delete</button>
                  </div>
                </div>
              </article>
            ))}
          </div>
        )}

        <footer className="clips-library-footer">
          <span>{totalCount} indexed clip{totalCount === 1 ? "" : "s"}</span>
          {telemetry && <details><summary>Library diagnostics</summary><code>Schema v{telemetry.schemaVersion} · query {telemetry.lastListQueryDurationMs?.toFixed(2) ?? "n/a"} ms</code><code>{telemetry.databasePath}</code><code>Newest Save indexed {telemetry.newestSavedClipIndexed === null ? "n/a" : telemetry.newestSavedClipIndexed ? "yes" : "no"} · {telemetry.newestSavedClipInsertionMs?.toFixed(2) ?? "n/a"} ms</code></details>}
        </footer>
      </section>
      {playingClip && <ClipPlayer clip={playingClip} onClose={() => setPlayingClip(null)} />}
    </div>
  );
}

function LibraryState({ title, detail }: { title: string; detail: string }) {
  return <div className="empty-state"><div className="empty-state-icon" aria-hidden="true"><svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2" /><path d="m10 9 5 3-5 3Z" /></svg></div><h2>{title}</h2><p>{detail}</p></div>;
}
