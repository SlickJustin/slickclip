export type ClipSelectionState = {
  selectedIds: ReadonlySet<string>;
  anchorId: string | null;
};

export type ClipSelectionModifiers = {
  toggle?: boolean;
  range?: boolean;
};

export type BatchDeleteTarget = {
  clipId: string;
  fileSizeBytes: number;
  fileModifiedAtMs: number;
};

type Identified = { id: string };
type DeleteFingerprint = Identified & { fileSizeBytes: number; fileModifiedAtMs: number };

export function emptyClipSelection(): ClipSelectionState {
  return { selectedIds: new Set<string>(), anchorId: null };
}

export function selectClip(
  current: ClipSelectionState,
  visibleIds: readonly string[],
  clipId: string,
  modifiers: ClipSelectionModifiers = {},
): ClipSelectionState {
  const targetIndex = visibleIds.indexOf(clipId);
  if (targetIndex < 0) return reconcileClipSelection(current, visibleIds);

  if (modifiers.range && current.anchorId) {
    const anchorIndex = visibleIds.indexOf(current.anchorId);
    if (anchorIndex >= 0) {
      const start = Math.min(anchorIndex, targetIndex);
      const end = Math.max(anchorIndex, targetIndex);
      const selectedIds = modifiers.toggle ? new Set(current.selectedIds) : new Set<string>();
      visibleIds.slice(start, end + 1).forEach((id) => selectedIds.add(id));
      return { selectedIds, anchorId: current.anchorId };
    }
  }

  if (modifiers.toggle) {
    const selectedIds = new Set(current.selectedIds);
    if (selectedIds.has(clipId)) selectedIds.delete(clipId);
    else selectedIds.add(clipId);
    return { selectedIds, anchorId: clipId };
  }

  return { selectedIds: new Set([clipId]), anchorId: clipId };
}

export function selectAllVisible(visibleIds: readonly string[]): ClipSelectionState {
  return {
    selectedIds: new Set(visibleIds),
    anchorId: visibleIds[0] ?? null,
  };
}

export function reconcileClipSelection(
  current: ClipSelectionState,
  visibleIds: readonly string[],
): ClipSelectionState {
  const visible = new Set(visibleIds);
  const selectedIds = new Set(Array.from(current.selectedIds).filter((id) => visible.has(id)));
  const anchorId = current.anchorId && visible.has(current.anchorId) ? current.anchorId : null;
  if (
    anchorId === current.anchorId
    && selectedIds.size === current.selectedIds.size
    && Array.from(selectedIds).every((id) => current.selectedIds.has(id))
  ) return current;
  return { selectedIds, anchorId };
}

export function selectedVisibleItems<T extends Identified>(
  selectedIds: ReadonlySet<string>,
  visibleItems: readonly T[],
): T[] {
  return visibleItems.filter((item) => selectedIds.has(item.id));
}

export function batchBooleanTarget<T>(items: readonly T[], predicate: (item: T) => boolean): boolean {
  return !items.every(predicate);
}

export function batchCollectionTarget(items: readonly { collectionIds: readonly string[] }[], collectionId: string): boolean {
  return !items.every((item) => item.collectionIds.includes(collectionId));
}

export function batchDeleteTargets(items: readonly DeleteFingerprint[]): BatchDeleteTarget[] {
  return items.map((item) => ({
    clipId: item.id,
    fileSizeBytes: item.fileSizeBytes,
    fileModifiedAtMs: item.fileModifiedAtMs,
  }));
}

export function manualDeleteProtectionWarning(protectedCount: number, totalCount: number): string {
  if (protectedCount < 1 || totalCount < 1) return "";
  if (totalCount === 1) {
    return "This clip is protected from automatic cleanup. Manual deletion overrides that protection and will still permanently remove it.";
  }
  return `${protectedCount} of the selected clips ${protectedCount === 1 ? "is" : "are"} protected from automatic cleanup. Manual deletion overrides that protection and will still permanently remove ${protectedCount === 1 ? "it" : "them"}.`;
}

export function confirmBatchDelete(count: number, protectedCount: number, confirm: (message: string) => boolean): boolean {
  if (count < 1) return false;
  const protectionWarning = manualDeleteProtectionWarning(protectedCount, count);
  return confirm(
    `Permanently delete ${count} selected clip${count === 1 ? "" : "s"}?\n\n`
    + (protectionWarning ? `${protectionWarning}\n\n` : "")
    + "This deletes the MP4 files from disk and cannot be undone.",
  );
}
