import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ClipPlayer } from "../components/ClipPlayer";
import { ClipThumbnail } from "../components/ClipThumbnail";
import type {
  ClipActionResponse, ClipListItem, ClipListResponse, ClipMutationResponse, ClipSortOrder,
  ClipsGridSize, ClipsView, CollectionMutationResponse, CollectionsResponse, CollectionSummary,
  LibrarySummary, LibraryTelemetry, ReconcileResponse, ReconciliationTelemetry, UiPreferences,
  UiPreferencesPatch, UiPreferencesResponse,
} from "../types/clips";
import {
  audioLabel, defaultUiPreferences, errorMessage, formatBytes, formatDuration100ns, formatFps,
  formatLastWatched,
} from "../types/clips";
import { resolvedCollectionSelection } from "../utils/libraryPreferences";

type Toast = (title: string, message: string, success: boolean) => void;
type Props = {
  onEditClip: (clip: ClipListItem) => void;
  playClip: ClipListItem | null;
  onPlayClipConsumed: () => void;
  onToast: Toast;
};

export function ClipsPage({ onEditClip, playClip, onPlayClipConsumed, onToast }: Props) {
  const [preferences, setPreferences] = useState<UiPreferences>(defaultUiPreferences);
  const [preferencesLoaded, setPreferencesLoaded] = useState(false);
  const [clips, setClips] = useState<ClipListItem[]>([]);
  const [collections, setCollections] = useState<CollectionSummary[]>([]);
  const [collectionsLoaded, setCollectionsLoaded] = useState(false);
  const [totalCount, setTotalCount] = useState(0);
  const [summary, setSummary] = useState<LibrarySummary | null>(null);
  const [telemetry, setTelemetry] = useState<LibraryTelemetry | null>(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [refreshResult, setRefreshResult] = useState<ReconciliationTelemetry | null>(null);
  const [playingClip, setPlayingClip] = useState<ClipListItem | null>(null);
  const listRequestToken = useRef(0);
  const reconciliationActive = refreshing || telemetry?.reconciliationRunning === true;

  const persistPreferences = useCallback(async (patch: UiPreferencesPatch) => {
    setPreferences((current) => ({ ...current, ...patch }));
    try {
      const response = await invoke<UiPreferencesResponse>("update_ui_preferences", { patch });
      if (!response.success) console.warn("SlickClip preference save failed:", response.errorMessage);
    } catch (cause) {
      console.warn("SlickClip preference save failed:", cause);
    }
  }, []);

  useEffect(() => {
    void invoke<UiPreferencesResponse>("get_ui_preferences")
      .then((response) => setPreferences(response.preferences))
      .catch((cause) => console.warn("SlickClip preference load failed:", cause))
      .finally(() => setPreferencesLoaded(true));
  }, []);

  const loadCollections = useCallback(async () => {
    const response = await invoke<CollectionsResponse>("list_collections_command");
    if (!response.success) throw new Error(response.errorMessage ?? "Collections are unavailable.");
    setCollections(response.collections);
    setCollectionsLoaded(true);
  }, []);

  useEffect(() => {
    if (!preferencesLoaded) return;
    void loadCollections().catch((cause) => setError(errorMessage(cause)));
  }, [loadCollections, preferencesLoaded]);

  useEffect(() => {
    if (!collectionsLoaded || !preferences.selectedCollectionId) return;
    if (resolvedCollectionSelection(preferences.selectedCollectionId, collections.map((item) => item.id)) === null) {
      void persistPreferences({ selectedCollectionId: null });
    }
  }, [collections, collectionsLoaded, persistPreferences, preferences.selectedCollectionId]);

  useEffect(() => {
    if (!playClip) return;
    setPlayingClip(playClip);
    onPlayClipConsumed();
  }, [onPlayClipConsumed, playClip]);

  const loadClips = useCallback(async () => {
    if (!preferencesLoaded) return;
    const token = ++listRequestToken.current;
    try {
      const response = await invoke<ClipListResponse>("list_clips", {
        request: {
          searchText: preferences.clipsSearchQuery,
          favoritesOnly: preferences.clipsView === "favorites" || preferences.clipsFavoritesOnly,
          recentlyWatchedOnly: preferences.clipsView === "recentlyWatched",
          collectionId: preferences.selectedCollectionId,
          sortOrder: preferences.clipsSort,
          limit: 200,
          offset: 0,
        },
      });
      if (token !== listRequestToken.current) return;
      setTelemetry(response.telemetry);
      if (!response.success) throw new Error(response.errorMessage ?? "The Clips library is unavailable.");
      setClips(response.clips);
      setTotalCount(response.totalCount);
      setSummary(response.summary);
      setError(null);
    } catch (cause) {
      if (token === listRequestToken.current) setError(errorMessage(cause));
    } finally {
      if (token === listRequestToken.current) setLoading(false);
    }
  }, [
    preferences.clipsFavoritesOnly,
    preferences.clipsSearchQuery,
    preferences.clipsSort,
    preferences.clipsView,
    preferences.selectedCollectionId,
    preferencesLoaded,
  ]);

  useEffect(() => {
    const timer = window.setTimeout(() => void loadClips(), 180);
    return () => window.clearTimeout(timer);
  }, [loadClips]);

  useEffect(() => {
    if (!preferencesLoaded) return;
    const timer = window.setTimeout(() => {
      void invoke<UiPreferencesResponse>("update_ui_preferences", { patch: { clipsSearchQuery: preferences.clipsSearchQuery } })
        .then((response) => { if (!response.success) console.warn("SlickClip search preference save failed:", response.errorMessage); })
        .catch((cause) => console.warn("SlickClip search preference save failed:", cause));
    }, 450);
    return () => window.clearTimeout(timer);
  }, [preferences.clipsSearchQuery, preferencesLoaded]);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    void listen<string>("clip-library-changed", () => void loadClips()).then((cleanup) => {
      if (disposed) cleanup(); else unlisten = cleanup;
    });
    return () => { disposed = true; unlisten?.(); };
  }, [loadClips]);

  async function refreshLibrary() {
    if (refreshing) return;
    setRefreshing(true); setError(null);
    try {
      const response = await invoke<ReconcileResponse>("refresh_clip_library");
      setTelemetry(response.telemetry);
      if (!response.success) throw new Error(response.errorMessage ?? "Clip reconciliation failed.");
      setRefreshResult(response.result);
      await Promise.all([loadClips(), loadCollections()]);
    } catch (cause) { setError(errorMessage(cause)); }
    finally { setRefreshing(false); }
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

  async function copyClip(clip: ClipListItem) {
    try {
      const response = await invoke<ClipActionResponse>("copy_clip_to_clipboard", { request: { clipId: clip.id } });
      if (!response.success) throw new Error(response.errorMessage ?? "The Windows clipboard rejected the clip.");
      onToast("Clip copied", "Paste it into Discord with Ctrl+V.", true);
    } catch (cause) { onToast("Could not copy clip", errorMessage(cause), false); }
  }

  async function deleteClip(clip: ClipListItem) {
    if (!window.confirm(`Permanently delete “${clip.displayName}”?\n\nThis deletes the MP4 from disk and cannot be undone.`)) return;
    const response = await invoke<ClipActionResponse>("delete_clip", { request: { clipId: clip.id } });
    if (!response.success) return setError(response.errorMessage ?? "Clip deletion failed.");
    await Promise.all([loadClips(), loadCollections()]);
  }

  async function createCollection() {
    const name = window.prompt("New collection name:");
    if (name === null) return;
    try {
      const response = await invoke<CollectionMutationResponse>("create_collection_command", { request: { name } });
      if (!response.success || !response.collection) throw new Error(response.errorMessage ?? "Collection creation failed.");
      await loadCollections(); onToast("Collection created", response.collection.name, true);
    } catch (cause) { onToast("Could not create collection", errorMessage(cause), false); }
  }

  async function renameSelectedCollection() {
    const collection = collections.find((item) => item.id === preferences.selectedCollectionId);
    if (!collection) return;
    const name = window.prompt("Rename collection:", collection.name);
    if (name === null) return;
    const response = await invoke<CollectionMutationResponse>("rename_collection_command", { request: { collectionId: collection.id, name } });
    if (!response.success || !response.collection) return onToast("Could not rename collection", response.errorMessage ?? "Collection rename failed.", false);
    await loadCollections(); onToast("Collection renamed", response.collection.name, true);
  }

  async function deleteSelectedCollection() {
    const collection = collections.find((item) => item.id === preferences.selectedCollectionId);
    if (!collection) return;
    if (!window.confirm(`Delete collection “${collection.name}”?\n\nThe clips inside it will remain in your Library.`)) return;
    const response = await invoke<ClipActionResponse>("delete_collection_command", { request: { collectionId: collection.id } });
    if (!response.success) return onToast("Could not delete collection", response.errorMessage ?? "Collection deletion failed.", false);
    await persistPreferences({ selectedCollectionId: null });
    await Promise.all([loadCollections(), loadClips()]);
    onToast("Collection deleted", `${collection.name}. Its clips remain in your Library.`, true);
  }

  async function setCollectionMembership(clip: ClipListItem, collection: CollectionSummary, included: boolean) {
    const response = await invoke<ClipMutationResponse>("set_clip_collection_membership", { request: { clipId: clip.id, collectionId: collection.id, included } });
    if (!response.success || !response.clip) return onToast("Could not update collection", response.errorMessage ?? "Collection assignment failed.", false);
    replaceClip(response.clip); await loadCollections();
    onToast(included ? "Added to collection" : "Removed from collection", collection.name, true);
  }

  function replaceClip(updated: ClipListItem) {
    setClips((current) => current.map((clip) => clip.id === updated.id ? updated : clip));
    setPlayingClip((current) => current?.id === updated.id ? updated : current);
    setError(null);
  }

  function selectView(view: ClipsView) {
    const patch: UiPreferencesPatch = { clipsView: view, clipsFavoritesOnly: view === "favorites" };
    if (view === "recentlyWatched") patch.clipsSort = "recentlyWatched";
    void persistPreferences(patch);
  }

  return <div className="page">
    <header className="page-header clips-page-header"><div><h1>Clips</h1><p>Your persistent local replay library.</p></div><button className="secondary-button" type="button" disabled={reconciliationActive} onClick={refreshLibrary}>{reconciliationActive ? "Refreshing..." : "Refresh Clips"}</button></header>
    <section className="clips-panel" aria-label="Clips library">
      <div className="clips-toolbar">
        <label className="search-field"><span className="visually-hidden">Search clips</span><svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m20 20-4-4" /></svg><input value={preferences.clipsSearchQuery} onChange={(event) => setPreferences((current) => ({ ...current, clipsSearchQuery: event.target.value }))} placeholder="Search names, files, or capture targets..." /></label>
        <label><span className="visually-hidden">Library view</span><select value={preferences.clipsView} onChange={(event) => selectView(event.target.value as ClipsView)}><option value="all">All Clips</option><option value="favorites">Favorites</option><option value="recentlyWatched">Recently Watched</option></select></label>
        <label><span className="visually-hidden">Collection</span><select value={preferences.selectedCollectionId ?? ""} onChange={(event) => void persistPreferences({ selectedCollectionId: event.target.value || null })}><option value="">All Collections</option>{collections.map((collection) => <option value={collection.id} key={collection.id}>{collection.name} ({collection.clipCount})</option>)}</select></label>
        <label><span className="visually-hidden">Sort clips</span><select value={preferences.clipsSort} onChange={(event) => void persistPreferences({ clipsSort: event.target.value as ClipSortOrder })}><option value="newestFirst">Newest</option><option value="oldestFirst">Oldest</option><option value="nameAscending">Name A–Z</option><option value="nameDescending">Name Z–A</option><option value="longestFirst">Longest</option><option value="shortestFirst">Shortest</option><option value="largestFirst">Largest</option><option value="smallestFirst">Smallest</option><option value="mostPlayed">Most Played</option><option value="recentlyWatched">Recently Watched</option></select></label>
        <label><span className="visually-hidden">Grid size</span><select value={preferences.clipsGridSize} onChange={(event) => void persistPreferences({ clipsGridSize: event.target.value as ClipsGridSize })}><option value="compact">Compact</option><option value="comfortable">Comfortable</option><option value="large">Large</option></select></label>
      </div>
      <div className="clips-collection-toolbar"><div><button type="button" onClick={() => void createCollection()}>+ New Collection</button>{preferences.selectedCollectionId && <><button type="button" onClick={() => void renameSelectedCollection()}>Rename Collection</button><button className="danger" type="button" onClick={() => void deleteSelectedCollection()}>Delete Collection</button></>}</div>{summary && <span className="library-storage-summary">{summary.clipCount} clip{summary.clipCount === 1 ? "" : "s"} • {formatBytes(summary.totalSizeBytes)} <small>Library size</small></span>}</div>
      {error && <div className="clips-library-error" role="alert"><span>{error}</span><button type="button" onClick={refreshLibrary}>Retry</button></div>}
      {refreshResult && <div className="clips-refresh-result" role="status">Scanned {refreshResult.scannedFiles} • unchanged {refreshResult.unchanged} • added {refreshResult.added} • updated {refreshResult.updated} • removed {refreshResult.removed} • failed {refreshResult.failed} • {refreshResult.durationMs.toFixed(1)} ms</div>}
      {loading ? <LibraryState title="Loading clips..." detail="Reading the local Clips database." /> : clips.length === 0 && !error ? <LibraryState title="No matching clips" detail="Try another view, collection, or search." /> : <div className={`clips-library-grid grid-${preferences.clipsGridSize}`}>
        {clips.map((clip) => <article className="clip-card" key={clip.id}><ClipThumbnail clip={clip} onPlay={() => setPlayingClip(clip)} /><div className="clip-card-body">
          <div className="clip-card-heading"><div><button className="clip-title-button" type="button" onClick={() => setPlayingClip(clip)}>{clip.displayName}</button><small>{new Date(clip.createdAtMs).toLocaleString()}</small>{clip.lastWatchedAtMs && <small title={new Date(clip.lastWatchedAtMs).toLocaleString()}>{formatLastWatched(clip.lastWatchedAtMs)}</small>}</div><button className={`favorite-button${clip.favorite ? " active" : ""}`} type="button" aria-label={clip.favorite ? "Remove favorite" : "Add favorite"} onClick={() => void setFavorite(clip)}>{clip.favorite ? "★" : "☆"}</button></div>
          <div className="clip-card-facts"><span>{formatDuration100ns(clip.duration100ns)}</span><span>{formatBytes(clip.fileSizeBytes)}</span><span>{formatFps(clip.fpsNumerator, clip.fpsDenominator)} FPS</span>{clip.playCount > 0 && <span>▶ {clip.playCount} {clip.playCount === 1 ? "play" : "plays"}</span>}{clip.captureTargetLabel && <span>{clip.captureTargetLabel}</span>}</div>
          {clip.audioTracks.length > 0 && <div className="clip-audio-badges">{clip.audioTracks.map((track) => <span key={track.streamIndex}>{audioLabel(track)}</span>)}</div>}
          <div className="clip-card-actions"><button className="clip-play-button" type="button" onClick={() => setPlayingClip(clip)}>▶ Play</button><button className="clip-edit-button" type="button" onClick={() => onEditClip(clip)}>Edit</button><button type="button" onClick={() => void copyClip(clip)}>Copy Clip</button>
            <details className="clip-collections-menu"><summary>Collections</summary><div>{collections.length === 0 ? <span>No collections yet.</span> : collections.map((collection) => <label key={collection.id}><input type="checkbox" checked={clip.collectionIds.includes(collection.id)} onChange={(event) => void setCollectionMembership(clip, collection, event.target.checked)} />{collection.name}</label>)}<button type="button" onClick={() => void createCollection()}>+ New Collection</button></div></details>
            <button type="button" onClick={() => void clipAction("open_clip_file", clip)}>Open Externally</button><button type="button" onClick={() => void clipAction("open_clip_folder", clip)}>Folder</button><button type="button" onClick={() => void renameClip(clip)}>Rename</button><button className="danger" type="button" onClick={() => void deleteClip(clip)}>Delete</button></div>
        </div></article>)}
      </div>}
      <footer className="clips-library-footer"><span>{totalCount} matching clip{totalCount === 1 ? "" : "s"}</span>{telemetry && <details><summary>Library diagnostics</summary><code>Schema v{telemetry.schemaVersion} • query {telemetry.lastListQueryDurationMs?.toFixed(2) ?? "n/a"} ms</code><code>{telemetry.databasePath}</code><code>Newest Save indexed {telemetry.newestSavedClipIndexed === null ? "n/a" : telemetry.newestSavedClipIndexed ? "yes" : "no"} • {telemetry.newestSavedClipInsertionMs?.toFixed(2) ?? "n/a"} ms</code></details>}</footer>
    </section>
    {playingClip && preferencesLoaded && <ClipPlayer clip={playingClip} preferences={preferences} onPreferencesChange={persistPreferences} onClipUpdated={replaceClip} onCopy={() => void copyClip(playingClip)} onClose={() => setPlayingClip(null)} />}
  </div>;
}

function LibraryState({ title, detail }: { title: string; detail: string }) {
  return <div className="empty-state"><div className="empty-state-icon" aria-hidden="true"><svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2" /><path d="m10 9 5 3-5 3Z" /></svg></div><h2>{title}</h2><p>{detail}</p></div>;
}
