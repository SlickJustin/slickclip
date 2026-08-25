import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { buildHelpSections } from "../src/content/helpContent.ts";
import { formatReplayWindow, replayHotkeyGuidance, saveLastLabel } from "../src/utils/replayGuidance.ts";

test("Replay guidance uses the configured duration and hotkey", () => {
  assert.equal(formatReplayWindow(30), "30 seconds");
  assert.equal(formatReplayWindow(60), "1 minute");
  assert.equal(formatReplayWindow(120), "2 minutes");
  assert.equal(saveLastLabel(300), "Save Last 5 minutes");
  assert.equal(
    replayHotkeyGuidance("Alt + F8", 120),
    "Press Alt + F8 anytime to save the previous 2 minutes.",
  );
});

test("Help contains the required beginner sections and current shortcut", () => {
  const sections = buildHelpSections("Ctrl + F9");
  assert.deepEqual(sections.map((section) => section.title), [
    "Getting Started",
    "How Replay Works",
    "Saving a Replay",
    "Game Detection",
    "Audio Setup",
    "Clips & Collections",
    "Editing a Clip",
    "Storage & Cleanup",
    "Keyboard Shortcuts",
    "Troubleshooting",
  ]);

  const content = JSON.stringify(sections);
  assert.match(content, /Ctrl \+ F9/);
  assert.match(content, /previous configured duration—not the next duration/);
  assert.doesNotMatch(content, /Watch Party|SlickEdit/);
});

test("Help is available from the application shell", () => {
  const sidebar = readFileSync(new URL("../src/components/Sidebar.tsx", import.meta.url), "utf8");
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  assert.match(sidebar, /id: "help", label: "Help"/);
  assert.match(app, /help: <HelpPage \/>/);
});

test("Info tips expose keyboard-focusable tooltip semantics", () => {
  const component = readFileSync(new URL("../src/components/InfoTip.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");
  assert.match(component, /<button/);
  assert.match(component, /aria-describedby=\{tooltipId\}/);
  assert.match(component, /role="tooltip"/);
  assert.match(styles, /\.info-tip:focus-within \.info-tip-content/);
  assert.match(styles, /max-width: calc\(100vw - 64px\)/);
});
