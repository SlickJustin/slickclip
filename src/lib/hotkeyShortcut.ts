export type ShortcutKeyboardEvent = {
  code: string;
  ctrlKey: boolean;
  shiftKey: boolean;
  altKey: boolean;
  metaKey: boolean;
};

const modifierCodes = new Set([
  "ControlLeft", "ControlRight", "ShiftLeft", "ShiftRight",
  "AltLeft", "AltRight", "MetaLeft", "MetaRight",
]);

export function isModifierCode(code: string) {
  return modifierCodes.has(code);
}

export function displayKeyFromCode(code: string) {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3);
  if (/^Digit[0-9]$/.test(code)) return code.slice(5);
  if (/^Numpad[0-9]$/.test(code)) return code;
  if (/^F(?:[1-9]|[12][0-9]|3[0-5])$/.test(code)) return code;
  if (["Fn", "FnLock", "Unidentified"].includes(code)) return null;
  return /^[A-Za-z][A-Za-z0-9]*$/.test(code) ? code : null;
}

function modifierParts(event: ShortcutKeyboardEvent) {
  return [
    event.ctrlKey ? "Ctrl" : null,
    event.shiftKey ? "Shift" : null,
    event.altKey ? "Alt" : null,
    event.metaKey ? "Win" : null,
  ].filter((part): part is string => part !== null);
}

export function combinationFromKeyboardEvent(event: ShortcutKeyboardEvent) {
  if (isModifierCode(event.code)) return null;
  const key = displayKeyFromCode(event.code);
  return key ? [...modifierParts(event), key].join(" + ") : null;
}

export function shortcutDraftFromKeyboardEvent(event: ShortcutKeyboardEvent) {
  const parts = modifierParts(event);
  return parts.length ? `${parts.join(" + ")} + …` : "Press a shortcut…";
}

export function isBareAlphanumericShortcut(combination: string) {
  return /^[A-Z0-9]$/.test(combination);
}
