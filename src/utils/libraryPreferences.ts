export function resolvedCollectionSelection(selectedId: string | null, availableIds: readonly string[]) {
  if (!selectedId) return null;
  return availableIds.includes(selectedId) ? selectedId : null;
}
