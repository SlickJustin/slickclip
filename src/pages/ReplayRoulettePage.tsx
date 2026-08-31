import { useCallback, useEffect, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ClipPlayer } from "../components/ClipPlayer";
import { ClipThumbnail } from "../components/ClipThumbnail";
import type {
  ClipActionResponse, ClipListItem, ClipListResponse, CollectionsResponse, CollectionSummary,
  UiPreferences, UiPreferencesPatch, UiPreferencesResponse,
} from "../types/clips";
import {
  defaultUiPreferences, errorMessage, formatDuration100ns, formatLastWatched,
} from "../types/clips";
import { selectRouletteClip } from "../utils/replayRoulette";

type Toast = (title: string, message: string, success: boolean) => void;
type Props = { onToast: Toast };

export function ReplayRoulettePage({ onToast }: Props) {
  const [preferences, setPreferences] = useState<UiPreferences>(defaultUiPreferences);
  const [preferencesLoaded, setPreferencesLoaded] = useState(false);
  const [collections, setCollections] = useState<CollectionSummary[]>([]);
  const [clips, setClips] = useState<ClipListItem[]>([]);
  const [favoritesOnly, setFavoritesOnly] = useState(false);
  const [collectionId, setCollectionId] = useState<string | null>(null);
  const [selectedClip, setSelectedClip] = useState<ClipListItem | null>(null);
  const [playingClip, setPlayingClip] = useState<ClipListItem | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [selectionRevision, setSelectionRevision] = useState(0);
  const requestToken = useRef(0);
  const recentIds = useRef<string[]>([]);

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
    void Promise.all([
      invoke<UiPreferencesResponse>("get_ui_preferences"),
      invoke<CollectionsResponse>("list_collections_command"),
    ]).then(([preferenceResponse, collectionResponse]) => {
      setPreferences(preferenceResponse.preferences);
      if (!collectionResponse.success) throw new Error(collectionResponse.errorMessage ?? "Collections are unavailable.");
      setCollections(collectionResponse.collections);
    }).catch((cause) => setError(errorMessage(cause))).finally(() => setPreferencesLoaded(true));
  }, []);

  const loadClips = useCallback(async () => {
    const token = ++requestToken.current;
    setLoading(true);
    try {
      const response = await invoke<ClipListResponse>("list_clips", {
        request: {
          searchText: "",
          favoritesOnly,
          recentlyWatchedOnly: false,
          collectionId,
          sortOrder: "newestFirst",
          limit: 200,
          offset: 0,
        },
      });
      if (token !== requestToken.current) return;
      if (!response.success) throw new Error(response.errorMessage ?? "Replay Roulette could not load your clips.");
      setClips(response.clips);
      setSelectedClip((current) => current && response.clips.find((clip) => clip.id === current.id) || null);
      setError(null);
    } catch (cause) {
      if (token === requestToken.current) setError(errorMessage(cause));
    } finally {
      if (token === requestToken.current) setLoading(false);
    }
  }, [collectionId, favoritesOnly]);

  useEffect(() => { void loadClips(); }, [loadClips]);

  useEffect(() => {
    let unlisten: UnlistenFn | undefined;
    let disposed = false;
    void listen<string>("clip-library-changed", () => void loadClips()).then((cleanup) => {
      if (disposed) cleanup(); else unlisten = cleanup;
    });
    return () => { disposed = true; unlisten?.(); };
  }, [loadClips]);

  function chooseClip() {
    const next = selectRouletteClip(clips, recentIds.current);
    if (!next) return;
    recentIds.current = [next.id, ...recentIds.current.filter((id) => id !== next.id)].slice(0, Math.min(5, Math.max(0, clips.length - 1)));
    setSelectedClip(next);
    setSelectionRevision((current) => current + 1);
  }

  function replaceClip(updated: ClipListItem) {
    setClips((current) => current.map((clip) => clip.id === updated.id ? updated : clip));
    setSelectedClip((current) => current?.id === updated.id ? updated : current);
    setPlayingClip((current) => current?.id === updated.id ? updated : current);
  }

  async function copyClip(clip: ClipListItem) {
    try {
      const response = await invoke<ClipActionResponse>("copy_clip_to_clipboard", { request: { clipId: clip.id } });
      if (!response.success) throw new Error(response.errorMessage ?? "The Windows clipboard rejected the clip.");
      onToast("Clip copied", "Paste it into Discord with Ctrl+V.", true);
    } catch (cause) {
      onToast("Could not copy clip", errorMessage(cause), false);
    }
  }

  const selectedCollection = collections.find((collection) => collection.id === collectionId);
  const filterDescription = [favoritesOnly ? "favorites" : "all clips", selectedCollection ? `in ${selectedCollection.name}` : null].filter(Boolean).join(" ");

  return <div className="page roulette-page">
    <header className="page-header roulette-page-header">
      <div><span className="roulette-page-eyebrow">Library wildcard</span><h1>Replay Roulette</h1><p>Let SlickClip dig up a moment you forgot about.</p></div>
      <div className="roulette-header-facts"><span>Weighted picks</span><span>Recent repeats avoided</span></div>
    </header>
    <section className="roulette-panel" aria-label="Replay Roulette">
      <div className="roulette-toolbar">
        <div className="roulette-pool-summary"><span className="roulette-eyebrow">Current pool</span><strong>{loading ? "Loading your Library..." : `${clips.length} eligible clip${clips.length === 1 ? "" : "s"}`}</strong><small>{filterDescription}</small></div>
        <div className="roulette-filter-controls">
          <label className="roulette-collection-filter"><span>Collection</span><select value={collectionId ?? ""} onChange={(event) => { setCollectionId(event.target.value || null); setSelectedClip(null); }}><option value="">All Collections</option>{collections.map((collection) => <option value={collection.id} key={collection.id}>{collection.name} ({collection.clipCount})</option>)}</select></label>
          <label className={`roulette-favorite-filter${favoritesOnly ? " active" : ""}`}><input type="checkbox" checked={favoritesOnly} onChange={(event) => { setFavoritesOnly(event.target.checked); setSelectedClip(null); }} /><span aria-hidden="true">★</span><strong>Favorites only</strong></label>
        </div>
      </div>

      {error ? <div className="roulette-state roulette-error" role="alert"><strong>Roulette is unavailable</strong><span>{error}</span><button className="secondary-button" type="button" onClick={() => void loadClips()}>Try Again</button></div>
        : loading ? <div className="roulette-state" role="status"><span className="player-spinner" /><strong>Shuffling your Library...</strong></div>
          : clips.length === 0 ? <div className="roulette-state"><div className="roulette-mark" aria-hidden="true">↻</div><strong>No clips match these filters</strong><span>Try another collection or include non-favorites.</span></div>
            : selectedClip ? <article className="roulette-result" key={`${selectedClip.id}:${selectionRevision}`} aria-live="polite">
              <div className="roulette-result-visual"><span className="roulette-card-shadow roulette-card-shadow-one" aria-hidden="true" /><span className="roulette-card-shadow roulette-card-shadow-two" aria-hidden="true" /><ClipThumbnail clip={selectedClip} onPlay={() => setPlayingClip(selectedClip)} /></div>
              <div className="roulette-result-copy">
                <div className="roulette-result-kicker"><span className="roulette-eyebrow">Your Library picked</span>{selectedClip.favorite && <span className="roulette-picked-favorite">★ Favorite</span>}</div>
                <h2>{selectedClip.displayName}</h2>
                <p>{selectedClip.captureTargetLabel ?? "Saved replay"}</p>
                <div className="roulette-facts"><span>{formatDuration100ns(selectedClip.duration100ns)}</span><span>{selectedClip.width}×{selectedClip.height}</span><span>{selectedClip.playCount === 0 ? "Never watched" : `${selectedClip.playCount} play${selectedClip.playCount === 1 ? "" : "s"}`}</span>{selectedClip.lastWatchedAtMs !== null && <span>{formatLastWatched(selectedClip.lastWatchedAtMs)}</span>}</div>
                <div className="roulette-actions"><button className="primary-button" type="button" onClick={() => setPlayingClip(selectedClip)}>▶ Play Replay</button><button className="secondary-button" type="button" onClick={chooseClip}>↻ Pick Another</button><button className="roulette-copy-button" type="button" onClick={() => void copyClip(selectedClip)}>Copy Clip</button></div>
                <small className="roulette-selection-note">Picks favor clips you watch less and avoid the last few results.</small>
              </div>
            </article>
              : <div className="roulette-landing"><div className="roulette-mark" aria-hidden="true"><span>↻</span></div><span className="roulette-eyebrow">Ready when you are</span><h2>Let the Library choose.</h2><p>SlickClip favors forgotten moments and keeps recent picks out of the way.</p><div className="roulette-promise"><span>Less-watched first</span><span>No immediate repeats</span><span>Your filters respected</span></div><button className="primary-button roulette-spin-button" type="button" onClick={chooseClip}>Pick My Replay</button><small>Choosing from {filterDescription}.</small></div>}
    </section>
    {playingClip && preferencesLoaded && <ClipPlayer clip={playingClip} preferences={preferences} onPreferencesChange={persistPreferences} onClipUpdated={replaceClip} onCopy={() => void copyClip(playingClip)} onClose={() => setPlayingClip(null)} />}
  </div>;
}
