import assert from "node:assert/strict";
import test from "node:test";
import {
  combinationFromKeyboardEvent,
  displayKeyFromCode,
  isBareAlphanumericShortcut,
  isModifierCode,
  shortcutDraftFromKeyboardEvent,
  type ShortcutKeyboardEvent,
} from "../src/lib/hotkeyShortcut.ts";

function keyboardEvent(code: string, modifiers: Partial<ShortcutKeyboardEvent> = {}): ShortcutKeyboardEvent {
  return {
    code,
    ctrlKey: false,
    shiftKey: false,
    altKey: false,
    metaKey: false,
    ...modifiers,
  };
}

test("recorder interprets unmodified, modified, function, and numpad shortcuts", () => {
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("F8")), "F8");
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("KeyR")), "R");
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("Digit5")), "5");
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("Numpad5")), "Numpad5");
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("MediaPlayPause")), "MediaPlayPause");
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("IntlBackslash", { ctrlKey: true })), "Ctrl + IntlBackslash");
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("F9", { ctrlKey: true })), "Ctrl + F9");
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("F12", { altKey: true })), "Alt + F12");
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("KeyR", { ctrlKey: true, altKey: true })), "Ctrl + Alt + R");
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("Numpad0", { shiftKey: true })), "Shift + Numpad0");
});

test("recorder rejects modifier-only and unrepresentable input", () => {
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("ControlLeft", { ctrlKey: true })), null);
  assert.equal(combinationFromKeyboardEvent(keyboardEvent("Unidentified")), null);
  assert.equal(displayKeyFromCode("Fn"), null);
  assert.equal(isModifierCode("MetaRight"), true);
});

test("recorder shows held modifiers live", () => {
  assert.equal(shortcutDraftFromKeyboardEvent(keyboardEvent("ControlLeft", { ctrlKey: true })), "Ctrl + …");
  assert.equal(shortcutDraftFromKeyboardEvent(keyboardEvent("ShiftLeft", { ctrlKey: true, shiftKey: true })), "Ctrl + Shift + …");
  assert.equal(shortcutDraftFromKeyboardEvent(keyboardEvent("ControlLeft")), "Press a shortcut…");
});

test("bare alphanumeric warning is limited to typing keys", () => {
  assert.equal(isBareAlphanumericShortcut("R"), true);
  assert.equal(isBareAlphanumericShortcut("5"), true);
  assert.equal(isBareAlphanumericShortcut("F8"), false);
  assert.equal(isBareAlphanumericShortcut("Numpad5"), false);
  assert.equal(isBareAlphanumericShortcut("Ctrl + R"), false);
});
