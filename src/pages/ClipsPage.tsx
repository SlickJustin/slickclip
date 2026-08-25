import { useCallback, useEffect, useLayoutEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent, type MouseEvent as ReactMouseEvent } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import { ClipPlayer } from "../components/ClipPlayer";
import { ClipThumbnail } from "../components/ClipThumbnail";
import type {
  BatchDeleteClipsResponse, ClipActionResponse, ClipListItem, ClipListResponse, ClipMutationResponse, ClipSortOrder,
  ClipsGridSize, ClipsView, CollectionMutationResponse, CollectionsResponse, CollectionSummary,
  LibrarySummary, LibraryTelemetry, ReconcileResponse, ReconciliationTelemetry, UiPreferences,
  UiPreferencesPatch, UiPreferencesResponse,
} from "../types/clips";
import {
  audioLabel, defaultUiPreferences, errorMessage, formatBytes, formatDuration100ns, formatFps,
  formatLastWatched,
} from "../types/clips";
import { resolvedCollectionSelection } from "../utils/libraryPreferences";
import {
  batchBooleanTarget, batchDeleteTargets, confirmBatchDelete,
  emptyClipSelection, manualDeleteProtectionWarning, reconcileClipSelection, selectAllVisible, selectClip, selectedVisibleItems,
} from "../utils/clipSelection";

type Toast = (title: string, message: string, success: boolean) => void;
type Props = {
  onEditClip: (clip: ClipListItem) => void;
  playClip: ClipListItem | null;
  onPlayClipConsumed: () => void;
  onToast: Toast;
};
type ClipMoreMenuState = {
  clipId: string;
  anchorTop: number;
  anchorBottom: number;
  anchorRight: number;
  top: number;
  left: number;
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
  const [moreMenu, setMoreMenu] = useState<ClipMoreMenuState | null>(null);
  const [selection, setSelection] = useState(emptyClipSelection);
  const [batchPending, setBatchPending] = useState(false);
  const listRequestToken = useRef(0);
  const moreMenuRef = useRef<HTMLDivElement>(null);
  const moreButtonRef = useRef<HTMLButtonElement>(null);
  const reconciliationActive = refreshing || telemetry?.reconciliationRunning === true;
  const moreMenuClip = moreMenu ? clips.find((clip) => clip.id === moreMenu.clipId) ?? null : null;
  const selectedClips = selectedVisibleItems(selection.selectedIds, clips);
  const visibleClipIds = clips.map((clip) => clip.id);

  const closeMoreMenu = useCallback((restoreFocus = false) => {
    setMoreMenu(null);
    if (restoreFocus) window.requestAnimationFrame(() => moreButtonRef.current?.focus());
  }, []);

  useLayoutEffect(() => {
    const menu = moreMenuRef.current;
    if (!moreMenu || !menu) return;
    const margin = 12;
    const gap = 8;
    const rect = menu.getBoundingClientRect();
    const availableBelow = window.innerHeight - moreMenu.anchorBottom - margin;
    const availableAbove = moreMenu.anchorTop - margin;
    const top = availableBelow >= rect.height || availableBelow >= availableAbove
      ? Math.min(moreMenu.anchorBottom + gap, window.innerHeight - rect.height - margin)
      : Math.max(margin, moreMenu.anchorTop - rect.height - gap);
    const left = Math.max(margin, Math.min(moreMenu.anchorRight - rect.width, window.innerWidth - rect.width - margin));
    if (Math.abs(top - moreMenu.top) > 0.5 || Math.abs(left - moreMenu.left) > 0.5) {
      setMoreMenu((current) => current ? { ...current, top, left } : current);
    }
  }, [moreMenu]);

  useEffect(() => {
    if (!moreMenu) return;
    const focusFrame = window.requestAnimationFrame(() => {
      moreMenuRef.current?.querySelector<HTMLElement>('button:not(:disabled), input:not(:disabled)')?.focus();
    });
    function onPointerDown(event: PointerEvent) {
      const target = event.target as Node | null;
      if (target && (moreMenuRef.current?.contains(target) || moreButtonRef.current?.contains(target))) return;
      closeMoreMenu();
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      closeMoreMenu(true);
    }
    function onViewportChange(event: Event) {
      const target = event.target;
      if (target instanceof Node && moreMenuRef.current?.contains(target)) return;
      closeMoreMenu();
    }
    document.addEventListener("pointerdown", onPointerDown, true);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", onViewportChange);
    window.addEventListener("scroll", onViewportChange, true);
    return () => {
      window.cancelAnimationFrame(focusFrame);
      document.removeEventListener("pointerdown", onPointerDown, true);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", onViewportChange);
      window.removeEventListener("scroll", onViewportChange, true);
    };
  }, [closeMoreMenu, moreMenu]);

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

  useEffect(() => {
    setSelection(emptyClipSelection());
  }, [
    preferences.clipsFavoritesOnly,
    preferences.clipsSearchQuery,
    preferences.clipsSort,
    preferences.clipsView,
    preferences.selectedCollectionId,
  ]);

  useEffect(() => {
    setSelection((current) => reconcileClipSelection(current, clips.map((clip) => clip.id)));
  }, [clips]);

  useEffect(() => {
    function onSelectionShortcut(event: KeyboardEvent) {
      if (playingClip || moreMenu) return;
      const target = event.target;
      const editing = target instanceof HTMLElement
        && (target.isContentEditable || ["INPUT", "TEXTAREA", "SELECT"].includes(target.tagName));
      if ((event.ctrlKey || event.metaKey) && !event.altKey && event.key.toLowerCase() === "a" && !editing) {
        event.preventDefault();
        setSelection(selectAllVisible(clips.map((clip) => clip.id)));
      } else if (event.key === "Escape" && selection.selectedIds.size > 0) {
        event.preventDefault();
        setSelection(emptyClipSelection());
      }
    }
    window.addEventListener("keydown", onSelectionShortcut);
    return () => window.removeEventListener("keydown", onSelectionShortcut);
  }, [clips, moreMenu, playingClip, selection.selectedIds.size]);

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

  async function setPinned(clip: ClipListItem) {
    const response = await invoke<ClipMutationResponse>("set_clip_pinned", { request: { clipId: clip.id, pinned: !clip.pinned } });
    if (!response.success || !response.clip) return setError(response.errorMessage ?? "Cleanup protection update failed.");
    replaceClip(response.clip);
    onToast(response.clip.pinned ? "Protected from Cleanup" : "Cleanup protection removed", response.clip.pinned ? "Automatic storage cleanup will skip this clip. You can still delete it manually." : "This clip may be included in a future automatic storage cleanup.", true);
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
    const protectionWarning = manualDeleteProtectionWarning(clip.pinned ? 1 : 0, 1);
    if (!window.confirm(`Permanently delete “${clip.displayName}”?\n\n${protectionWarning ? `${protectionWarning}\n\n` : ""}This deletes the MP4 from disk and cannot be undone.`)) return;
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

  async function mutateSelectedClips(
    command: "set_clip_favorite" | "set_clip_pinned" | "set_clip_collection_membership",
    requestFor: (clip: ClipListItem) => Record<string, unknown>,
    successTitle: string,
  ) {
    if (batchPending || selectedClips.length === 0) return;
    const snapshot = [...selectedClips];
    setBatchPending(true);
    setError(null);
    let updatedCount = 0;
    let firstError: string | null = null;
    for (const clip of snapshot) {
      try {
        const response = await invoke<ClipMutationResponse>(command, { request: requestFor(clip) });
        if (!response.success || !response.clip) {
          firstError ??= response.errorMessage ?? `Could not update ${clip.displayName}.`;
          continue;
        }
        updatedCount += 1;
        replaceClip(response.clip);
      } catch (cause) {
        firstError ??= errorMessage(cause);
      }
    }
    try {
      await Promise.all([loadClips(), loadCollections()]);
    } catch (cause) {
      firstError ??= errorMessage(cause);
    } finally {
      setSelection(emptyClipSelection());
      setBatchPending(false);
    }
    if (firstError) {
      onToast(
        `${successTitle} incomplete`,
        `${updatedCount} of ${snapshot.length} clips updated. ${firstError}`,
        false,
      );
    } else {
      onToast(successTitle, `${updatedCount} clip${updatedCount === 1 ? "" : "s"} updated.`, true);
    }
  }

  function setSelectedFavorite(favorite: boolean) {
    return mutateSelectedClips(
      "set_clip_favorite",
      (clip) => ({ clipId: clip.id, favorite }),
      favorite ? "Clips favorited" : "Favorites removed",
    );
  }

  function setSelectedPinned(pinned: boolean) {
    return mutateSelectedClips(
      "set_clip_pinned",
      (clip) => ({ clipId: clip.id, pinned }),
      pinned ? "Protected from Cleanup" : "Cleanup protection removed",
    );
  }

  function setSelectedCollection(collection: CollectionSummary, included: boolean) {
    return mutateSelectedClips(
      "set_clip_collection_membership",
      (clip) => ({ clipId: clip.id, collectionId: collection.id, included }),
      included ? `Added to ${collection.name}` : `Removed from ${collection.name}`,
    );
  }

  async function deleteSelectedClips() {
    if (batchPending) return;
    const targets = batchDeleteTargets(selectedClips);
    const protectedCount = selectedClips.filter((clip) => clip.pinned).length;
    if (!confirmBatchDelete(targets.length, protectedCount, window.confirm)) return;
    setBatchPending(true);
    setError(null);
    try {
      const response = await invoke<BatchDeleteClipsResponse>("delete_clips", { request: { targets } });
      await Promise.all([loadClips(), loadCollections()]);
      setSelection(emptyClipSelection());
      if (!response.success) {
        return onToast(
          "Batch deletion incomplete",
          `${response.deletedCount} of ${response.requestedCount} clips deleted. ${response.errorMessage ?? "The remaining clips were not deleted."}`,
          false,
        );
      }
      onToast("Clips deleted", `${response.deletedCount} clips permanently deleted.`, true);
    } catch (cause) {
      setSelection(emptyClipSelection());
      await Promise.allSettled([loadClips(), loadCollections()]);
      onToast("Could not complete batch deletion", errorMessage(cause), false);
    } finally {
      setBatchPending(false);
    }
  }

  function handleCardSelection(event: ReactMouseEvent<HTMLElement>, clipId: string) {
    if (isInteractiveTarget(event.target)) return;
    setSelection((current) => selectClip(current, visibleClipIds, clipId, {
      toggle: event.ctrlKey || event.metaKey,
      range: event.shiftKey,
    }));
  }

  function handleCardSelectionKey(event: ReactKeyboardEvent<HTMLElement>, clipId: string) {
    if (event.target !== event.currentTarget || !["Enter", " "].includes(event.key)) return;
    event.preventDefault();
    setSelection((current) => selectClip(current, visibleClipIds, clipId, {
      toggle: event.ctrlKey || event.metaKey,
      range: event.shiftKey,
    }));
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

  function toggleMoreMenu(clip: ClipListItem, button: HTMLButtonElement) {
    if (moreMenu?.clipId === clip.id) return closeMoreMenu();
    const rect = button.getBoundingClientRect();
    moreButtonRef.current = button;
    setMoreMenu({
      clipId: clip.id,
      anchorTop: rect.top,
      anchorBottom: rect.bottom,
      anchorRight: rect.right,
      top: rect.bottom + 8,
      left: Math.max(12, rect.right - 272),
    });
  }

  function navigateMoreMenu(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(event.key)) return;
    const items = Array.from(event.currentTarget.querySelectorAll<HTMLElement>('button:not(:disabled), input:not(:disabled)'));
    if (items.length === 0) return;
    event.preventDefault();
    const currentIndex = items.indexOf(document.activeElement as HTMLElement);
    const nextIndex = event.key === "Home"
      ? 0
      : event.key === "End"
        ? items.length - 1
        : event.key === "ArrowDown"
          ? (currentIndex + 1 + items.length) % items.length
          : (currentIndex - 1 + items.length) % items.length;
    items[nextIndex].focus();
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
      {selectedClips.length > 0 && <div className="clips-batch-bar" role="region" aria-label="Selected clip actions">
        <strong>{selectedClips.length} selected</strong>
        <div className="clips-batch-actions">
          <details className="clips-batch-menu">
            <summary aria-disabled={batchPending}>Add to Collection</summary>
            <div>
              {collections.length === 0
                ? <span>No collections yet.</span>
                : collections.map((collection) => {
                  const allIncluded = selectedClips.every((clip) => clip.collectionIds.includes(collection.id));
                  const anyIncluded = selectedClips.some((clip) => clip.collectionIds.includes(collection.id));
                  return <div className="clips-batch-collection" key={collection.id}>
                    <span title={collection.name}>{collection.name}</span>
                    <button type="button" disabled={batchPending || allIncluded} onClick={() => void setSelectedCollection(collection, true)}>Add</button>
                    <button type="button" disabled={batchPending || !anyIncluded} onClick={() => void setSelectedCollection(collection, false)}>Remove</button>
                  </div>;
                })}
            </div>
          </details>
          <button type="button" disabled={batchPending} onClick={() => void setSelectedFavorite(batchBooleanTarget(selectedClips, (clip) => clip.favorite))}>
            {batchBooleanTarget(selectedClips, (clip) => clip.favorite) ? "Favorite" : "Unfavorite"}
          </button>
          <button type="button" disabled={batchPending} onClick={() => void setSelectedPinned(batchBooleanTarget(selectedClips, (clip) => clip.pinned))}>
            {batchBooleanTarget(selectedClips, (clip) => clip.pinned) ? "Protect from Cleanup" : "Remove Cleanup Protection"}
          </button>
          <details className="clips-batch-menu">
            <summary aria-disabled={batchPending}>More</summary>
            <div>
              <button type="button" disabled={batchPending} onClick={() => void setSelectedFavorite(true)}>Favorite selected</button>
              <button type="button" disabled={batchPending} onClick={() => void setSelectedFavorite(false)}>Unfavorite selected</button>
              <button type="button" disabled={batchPending} onClick={() => void setSelectedPinned(true)}>Protect from Cleanup</button>
              <button type="button" disabled={batchPending} onClick={() => void setSelectedPinned(false)}>Remove Cleanup Protection</button>
              <button className="danger" type="button" disabled={batchPending} onClick={() => void deleteSelectedClips()}>Delete selected</button>
            </div>
          </details>
          <button className="clips-batch-clear" type="button" disabled={batchPending} onClick={() => setSelection(emptyClipSelection())}>Clear</button>
        </div>
      </div>}
      {error && <div className="clips-library-error" role="alert"><span>{error}</span><button type="button" onClick={refreshLibrary}>Retry</button></div>}
      {refreshResult && <div className="clips-refresh-result" role="status">Scanned {refreshResult.scannedFiles} • unchanged {refreshResult.unchanged} • added {refreshResult.added} • updated {refreshResult.updated} • removed {refreshResult.removed} • failed {refreshResult.failed} • {refreshResult.durationMs.toFixed(1)} ms</div>}
      {loading ? <LibraryState title="Loading clips..." detail="Reading the local Clips database." /> : clips.length === 0 && !error ? <LibraryState title="No matching clips" detail="Try another view, collection, or search." /> : <div className={`clips-library-grid grid-${preferences.clipsGridSize}`}>
        {clips.map((clip) => {
          const selected = selection.selectedIds.has(clip.id);
          return <article
            className={`clip-card${selected ? " selected" : ""}`}
            key={clip.id}
            tabIndex={0}
            aria-label={`${clip.displayName}${clip.pinned ? ", protected from cleanup" : ""}${selected ? ", selected" : ""}`}
            onClick={(event) => handleCardSelection(event, clip.id)}
            onKeyDown={(event) => handleCardSelectionKey(event, clip.id)}
          >
          <button
            className="clip-selection-toggle"
            type="button"
            aria-pressed={selected}
            aria-label={selected ? `Remove ${clip.displayName} from selection` : `Add ${clip.displayName} to selection`}
            onClick={(event) => {
              event.stopPropagation();
              setSelection((current) => selectClip(current, visibleClipIds, clip.id, { toggle: true }));
            }}
          ><span aria-hidden="true">{selected ? "✓" : ""}</span></button>
          <ClipThumbnail clip={clip} onPlay={() => setPlayingClip(clip)} /><div className="clip-card-body">
          <div className="clip-card-heading">
            <div>
              <button className="clip-title-button" type="button" onClick={() => setPlayingClip(clip)}>{clip.displayName}</button>
              <small className="clip-card-date">
                {new Date(clip.createdAtMs).toLocaleString()}
                {clip.lastWatchedAtMs && <span title={new Date(clip.lastWatchedAtMs).toLocaleString()}> • {formatLastWatched(clip.lastWatchedAtMs)}</span>}
              </small>
            </div>
            <button className={`favorite-button${clip.favorite ? " active" : ""}`} type="button" aria-label={clip.favorite ? "Remove favorite" : "Add favorite"} title={clip.favorite ? "Remove favorite" : "Add favorite"} onClick={() => void setFavorite(clip)}>{clip.favorite ? "★" : "☆"}</button>
          </div>
          <div className="clip-card-facts">
            <span>{formatDuration100ns(clip.duration100ns)}</span>
            <i aria-hidden="true">•</i>
            <span>{clip.width}×{clip.height} / {formatFps(clip.fpsNumerator, clip.fpsDenominator)} FPS</span>
            <i aria-hidden="true">•</i>
            <span>{formatBytes(clip.fileSizeBytes)}</span>
          </div>
          {(clip.pinned || clip.playCount > 0 || clip.captureTargetLabel) && <div className="clip-card-context">
            {clip.pinned && <span className="clip-protected-badge" title="Excluded from automatic storage cleanup; manual deletion is still allowed">Protected from Cleanup</span>}
            {clip.playCount > 0 && <span>{clip.playCount} {clip.playCount === 1 ? "play" : "plays"}</span>}
            {clip.captureTargetLabel && <span title={clip.captureTargetLabel}>{clip.captureTargetLabel}</span>}
          </div>}
          {clip.audioTracks.length > 0 && <div className="clip-audio-badges">{clip.audioTracks.map((track) => <span key={track.streamIndex}>{audioLabel(track)}</span>)}</div>}
          <div className="clip-card-actions">
            <button className="clip-play-button" type="button" onClick={() => setPlayingClip(clip)}>▶ Play</button>
            <button className="clip-edit-button" type="button" onClick={() => onEditClip(clip)}>Edit</button>
            <button
              className="clip-more-button"
              type="button"
              aria-haspopup="menu"
              aria-expanded={moreMenu?.clipId === clip.id}
              onClick={(event) => toggleMoreMenu(clip, event.currentTarget)}
            ><span aria-hidden="true">•••</span> More</button>
          </div>
        </div></article>;
        })}
      </div>}
      <footer className="clips-library-footer"><span>{totalCount} matching clip{totalCount === 1 ? "" : "s"}</span>{telemetry && <details><summary>Library diagnostics</summary><code>Schema v{telemetry.schemaVersion} • query {telemetry.lastListQueryDurationMs?.toFixed(2) ?? "n/a"} ms</code><code>{telemetry.databasePath}</code><code>Newest Save indexed {telemetry.newestSavedClipIndexed === null ? "n/a" : telemetry.newestSavedClipIndexed ? "yes" : "no"} • {telemetry.newestSavedClipInsertionMs?.toFixed(2) ?? "n/a"} ms</code></details>}</footer>
    </section>
    {moreMenu && moreMenuClip && createPortal(
      <div
        className="clip-more-menu"
        ref={moreMenuRef}
        role="menu"
        aria-label={`More actions for ${moreMenuClip.displayName}`}
        style={{ top: moreMenu.top, left: moreMenu.left }}
        onKeyDown={navigateMoreMenu}
      >
        <div className="clip-more-menu-section clip-more-menu-primary" role="group" aria-label="Clip actions">
          <button type="button" role="menuitem" onClick={() => { closeMoreMenu(); void copyClip(moreMenuClip); }}>Copy Clip</button>
        </div>
        <div className="clip-more-menu-section" role="group" aria-label="Collections">
          <span className="clip-more-menu-heading">Collections</span>
          {collections.length === 0
            ? <span className="clip-more-menu-empty">No collections yet.</span>
            : collections.map((collection) => <label key={collection.id}>
              <input
                type="checkbox"
                role="menuitemcheckbox"
                aria-checked={moreMenuClip.collectionIds.includes(collection.id)}
                checked={moreMenuClip.collectionIds.includes(collection.id)}
                onChange={(event) => void setCollectionMembership(moreMenuClip, collection, event.target.checked)}
              />
              <span>{collection.name}</span>
            </label>)}
          <button type="button" role="menuitem" onClick={() => void createCollection()}>+ New Collection</button>
        </div>
        <div className="clip-more-menu-section clip-more-menu-actions" role="group" aria-label="File actions">
          <button type="button" role="menuitem" onClick={() => { closeMoreMenu(); void clipAction("open_clip_file", moreMenuClip); }}>Open Externally</button>
          <button type="button" role="menuitem" onClick={() => { closeMoreMenu(); void clipAction("open_clip_folder", moreMenuClip); }}>Open Folder</button>
          <button type="button" role="menuitem" onClick={() => { closeMoreMenu(); void renameClip(moreMenuClip); }}>Rename</button>
          <button type="button" role="menuitem" onClick={() => { closeMoreMenu(); void setPinned(moreMenuClip); }}>{moreMenuClip.pinned ? "Remove Cleanup Protection" : "Protect from Cleanup"}</button>
          <button className="danger" type="button" role="menuitem" onClick={() => { closeMoreMenu(); void deleteClip(moreMenuClip); }}>Delete Clip</button>
        </div>
      </div>,
      document.body,
    )}
    {playingClip && preferencesLoaded && <ClipPlayer clip={playingClip} preferences={preferences} onPreferencesChange={persistPreferences} onClipUpdated={replaceClip} onCopy={() => void copyClip(playingClip)} onClose={() => setPlayingClip(null)} />}
  </div>;
}

function LibraryState({ title, detail }: { title: string; detail: string }) {
  return <div className="empty-state"><div className="empty-state-icon" aria-hidden="true"><svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2" /><path d="m10 9 5 3-5 3Z" /></svg></div><h2>{title}</h2><p>{detail}</p></div>;
}

function isInteractiveTarget(target: EventTarget | null) {
  return target instanceof Element
    && Boolean(target.closest("button, a, input, select, textarea, summary, [role='menuitem']"));
}
