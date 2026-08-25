import assert from "node:assert/strict";
import test from "node:test";
import {
  batchBooleanTarget,
  batchCollectionTarget,
  batchDeleteTargets,
  confirmBatchDelete,
  emptyClipSelection,
  manualDeleteProtectionWarning,
  reconcileClipSelection,
  selectAllVisible,
  selectClip,
  selectedVisibleItems,
} from "../src/utils/clipSelection.ts";

const visible = ["alpha", "bravo", "charlie", "delta"];

function ids(selection: ReturnType<typeof emptyClipSelection>) {
  return Array.from(selection.selectedIds);
}

test("plain click replaces selection and Ctrl+Click toggles stable clip IDs", () => {
  let selection = selectClip(emptyClipSelection(), visible, "bravo");
  assert.deepEqual(ids(selection), ["bravo"]);
  assert.equal(selection.anchorId, "bravo");

  selection = selectClip(selection, visible, "delta", { toggle: true });
  assert.deepEqual(ids(selection), ["bravo", "delta"]);
  selection = selectClip(selection, visible, "bravo", { toggle: true });
  assert.deepEqual(ids(selection), ["delta"]);

  selection = selectClip(selection, visible, "charlie");
  assert.deepEqual(ids(selection), ["charlie"]);
});

test("Shift+Click follows the current visible order and Ctrl+Shift adds a range", () => {
  const sorted = ["delta", "bravo", "alpha", "charlie"];
  let selection = selectClip(emptyClipSelection(), sorted, "bravo");
  selection = selectClip(selection, sorted, "charlie", { range: true });
  assert.deepEqual(ids(selection), ["bravo", "alpha", "charlie"]);

  selection = selectClip(selection, sorted, "delta", { toggle: true });
  selection = selectClip(selection, sorted, "alpha", { toggle: true, range: true });
  assert.deepEqual(ids(selection), ["bravo", "alpha", "charlie", "delta"]);
});

test("Ctrl+A selects only visible clips and Escape-style clearing is complete", () => {
  const selection = selectAllVisible(["filtered-a", "filtered-b"]);
  assert.deepEqual(ids(selection), ["filtered-a", "filtered-b"]);
  assert.equal(selection.selectedIds.has("hidden"), false);
  assert.deepEqual(ids(emptyClipSelection()), []);
});

test("filter changes prune hidden selection and clear an invisible range anchor", () => {
  let selection = selectAllVisible(visible);
  selection = reconcileClipSelection(selection, ["bravo", "delta"]);
  assert.deepEqual(ids(selection), ["bravo", "delta"]);
  assert.equal(selection.anchorId, null);

  const unchanged = reconcileClipSelection(selection, ["delta", "bravo"]);
  assert.equal(unchanged, selection, "sort-only reconciliation keeps selection by stable ID");
  assert.deepEqual(selectedVisibleItems(unchanged.selectedIds, [{ id: "delta" }, { id: "bravo" }]), [
    { id: "delta" },
    { id: "bravo" },
  ]);
});

test("batch Favorite and Protect targets set mixed groups, then unset all-on groups", () => {
  const mixed = [{ favorite: true, pinned: false }, { favorite: false, pinned: true }];
  assert.equal(batchBooleanTarget(mixed, (clip) => clip.favorite), true);
  assert.equal(batchBooleanTarget(mixed, (clip) => clip.pinned), true);
  assert.equal(batchBooleanTarget([{ favorite: true }, { favorite: true }], (clip) => clip.favorite), false);
  assert.equal(batchBooleanTarget([{ pinned: true }, { pinned: true }], (clip) => clip.pinned), false);
});

test("batch collection membership adds mixed groups and removes all-member groups", () => {
  assert.equal(batchCollectionTarget([{ collectionIds: ["one"] }, { collectionIds: [] }], "one"), true);
  assert.equal(batchCollectionTarget([{ collectionIds: ["one"] }, { collectionIds: ["one", "two"] }], "one"), false);
});

test("batch delete confirmation includes the exact count and honors cancellation", () => {
  let message = "";
  assert.equal(confirmBatchDelete(3, 0, (value) => { message = value; return false; }), false);
  assert.match(message, /3 selected clips/);
  assert.match(message, /cannot be undone/);
  assert.equal(confirmBatchDelete(0, 0, () => { throw new Error("must not confirm an empty selection"); }), false);
});

test("manual delete confirmation explains cleanup-protection override", () => {
  let message = "";
  assert.equal(confirmBatchDelete(4, 2, (value) => { message = value; return true; }), true);
  assert.match(message, /2 of the selected clips are protected from automatic cleanup/);
  assert.match(message, /Manual deletion overrides that protection/);
  assert.match(manualDeleteProtectionWarning(1, 1), /This clip is protected from automatic cleanup/);
  assert.equal(manualDeleteProtectionWarning(0, 3), "");
});

test("destructive targets are derived from selected visible items only", () => {
  const selectedIds = new Set(["visible", "hidden"]);
  const items = selectedVisibleItems(selectedIds, [{
    id: "visible",
    fileSizeBytes: 101,
    fileModifiedAtMs: 202,
  }]);
  assert.deepEqual(batchDeleteTargets(items), [{
    clipId: "visible",
    fileSizeBytes: 101,
    fileModifiedAtMs: 202,
  }]);
  assert.deepEqual(items, [{ id: "visible", fileSizeBytes: 101, fileModifiedAtMs: 202 }]);
});
