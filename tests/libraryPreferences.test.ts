import assert from "node:assert/strict";
import test from "node:test";
import { resolvedCollectionSelection } from "../src/utils/libraryPreferences.ts";

test("deleted persisted collection falls back to All Clips", () => {
  assert.equal(resolvedCollectionSelection("deleted", ["funny", "best"]), null);
  assert.equal(resolvedCollectionSelection("funny", ["funny", "best"]), "funny");
  assert.equal(resolvedCollectionSelection(null, ["funny"]), null);
});
