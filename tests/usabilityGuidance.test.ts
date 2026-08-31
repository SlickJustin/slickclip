import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import test from "node:test";
import { buildHelpSections } from "../src/content/helpContent.ts";
import { formatReplayWindow, replayHotkeyGuidance, saveLastLabel } from "../src/utils/replayGuidance.ts";
import { detectedReplayLabel, showCandidateApprovalControls } from "../src/utils/gameDetectionStatus.ts";

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

test("Replay exposes one persistent display-capture product policy", () => {
  const replayPage = readFileSync(new URL("../src/pages/ReplayPage.tsx", import.meta.url), "utf8");
  const settingsPage = readFileSync(new URL("../src/pages/SettingsPage.tsx", import.meta.url), "utf8");
  assert.match(replayPage, /Display Capture/);
  assert.match(replayPage, /Automatic game detection chooses the game&apos;s display/);
  assert.match(settingsPage, /keeps that same display for the whole Replay session/);
  assert.doesNotMatch(settingsPage, /Reset learned capture compatibility|Game Capture was selected/);
});

test("Game detection presents authoritative capture states and scopes approval controls", () => {
  assert.equal(detectedReplayLabel("detected"), "Detected");
  assert.equal(detectedReplayLabel("starting"), "Starting Replay…");
  assert.equal(detectedReplayLabel("replayReady"), "Replay Ready");
  assert.equal(detectedReplayLabel("captureFailed"), "Capture failed");
  assert.equal(detectedReplayLabel("replayStopped"), "Replay stopped");
  assert.equal(showCandidateApprovalControls("anyDetectedGame"), false);
  assert.equal(showCandidateApprovalControls("approvedGamesOnly"), true);
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

test("Replay UI continuously reconciles authoritative backend state", () => {
  const replayPage = readFileSync(new URL("../src/pages/ReplayPage.tsx", import.meta.url), "utf8");
  assert.match(replayPage, /listen<ReplayBufferStatus>\("replay-buffer-status-changed"/);
  assert.match(replayPage, /setInterval\(\(\) => void refreshReplayStatus\(\), 500\)/);
  assert.match(replayPage, /!replayStatusLoaded/);
  assert.match(replayPage, /Stop Replay before choosing a capture source for the next session/);
  assert.match(replayPage, /replayStatus\.captureHealth === "Healthy"/);
  assert.doesNotMatch(replayPage, /setReplayCommandError\((?:status|event\.payload)\.errorMessage/);
  assert.match(replayPage, /setReplayStatusFetchError\(null\)/);
  assert.match(replayPage, /Last recovery/);
  assert.doesNotMatch(replayPage, /captureModeOptions|captureModeLabel/);
});

test("Replay recovery never exposes Save or Ready while capture is non-healthy", () => {
  const replayPage = readFileSync(new URL("../src/pages/ReplayPage.tsx", import.meta.url), "utf8");
  const gameDetection = readFileSync(new URL("../src-tauri/src/game_detection.rs", import.meta.url), "utf8");
  assert.match(replayPage, /replayStatus\.state === "running"\s*&&\s*replayStatus\.captureHealth === "Healthy"/);
  assert.match(replayPage, /replayStatus\.captureHealth === "Recovering"\s*\? "Recovering capture"/);
  assert.match(replayPage, /replayCommandError \|\| replayStatusFetchError \|\| replayStatus\.errorMessage/);
  assert.match(gameDetection, /replay_status\.capture_health != "Recovering"/);
});

test("Save Replay completion owns exactly one success overlay path", () => {
  const saveSource = readFileSync(new URL("../src-tauri/src/clips/save.rs", import.meta.url), "utf8");
  assert.equal(saveSource.match(/"save-replay-completed"/g)?.length, 1);
  assert.equal(saveSource.match(/show_save_overlay\(\s*&app_handle/g)?.length, 1);
  assert.match(saveSource, /if should_show_success_overlay\(true, library_indexed\)/);
  assert.match(saveSource, /assembly_succeeded && library_indexed/);
  assert.match(saveSource, /show_save_failure_overlay/);
});

test("Save overlay follows the captured monitor and Save & Name remains a single indexed save", () => {
  const desktop = readFileSync(new URL("../src-tauri/src/desktop.rs", import.meta.url), "utf8");
  const saveSource = readFileSync(new URL("../src-tauri/src/clips/save.rs", import.meta.url), "utf8");
  const settings = readFileSync(new URL("../src/pages/SettingsPage.tsx", import.meta.url), "utf8");
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  assert.match(desktop, /preferred_monitor_origin/);
  assert.match(desktop, /monitor_matches_origin/);
  assert.match(saveSource, /SaveIntent::SaveAndName/);
  assert.match(saveSource, /should_request_name\(intent, library_indexed\)/);
  assert.match(settings, /Save &amp; Name Hotkey/);
  assert.match(settings, /clear_save_and_name_hotkey/);
  assert.match(app, /save-replay-name-requested/);
  assert.match(app, /rename_clip_display_name/);
});

test("visual-only NSIS branding keeps custom executable icons out of packaging", () => {
  const config = JSON.parse(
    readFileSync(new URL("../src-tauri/tauri.conf.json", import.meta.url), "utf8"),
  );
  const packageJson = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8"));
  const artworkGenerator = readFileSync(
    new URL("../scripts/generate-installer-assets.ps1", import.meta.url),
    "utf8",
  );
  const nsis = config.bundle.windows.nsis;
  assert.equal(nsis.installMode, "currentUser");
  assert.equal(nsis.headerImage, "icons/installer/header.bmp");
  assert.equal(nsis.sidebarImage, "icons/installer/sidebar.bmp");
  assert.equal(nsis.installerIcon, undefined);
  assert.equal(nsis.uninstallerIcon, undefined);
  assert.equal(nsis.uninstallerHeaderImage, undefined);
  assert.match(packageJson.scripts.bundle, /^npm run prepare:installer && npm run prepare:ffmpeg/);
  assert.match(artworkGenerator, /Made to capture the DAWGs`nworst moments\./);
  assert.doesNotMatch(artworkGenerator, /READY WHEN THE MOMENT HITS/);

  const dimensions = (relativePath: string) => {
    const bitmap = readFileSync(new URL(relativePath, import.meta.url));
    assert.equal(bitmap.toString("ascii", 0, 2), "BM");
    return [bitmap.readInt32LE(18), bitmap.readInt32LE(22)];
  };
  assert.deepEqual(dimensions("../src-tauri/icons/installer/header.bmp"), [150, 57]);
  assert.deepEqual(dimensions("../src-tauri/icons/installer/sidebar.bmp"), [164, 314]);
});

test("Home uses live Replay and Library data without replacing the full Clips workspace", () => {
  const app = readFileSync(new URL("../src/App.tsx", import.meta.url), "utf8");
  const sidebar = readFileSync(new URL("../src/components/Sidebar.tsx", import.meta.url), "utf8");
  const home = readFileSync(new URL("../src/pages/HomePage.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");

  assert.match(app, /useState<PageId>\("home"\)/);
  assert.match(app, /home: <HomePage/);
  assert.match(sidebar, /id: "home", label: "Home"/);
  assert.match(home, /invoke<ReplayStatus>\("get_replay_buffer_status"\)/);
  assert.match(home, /listen<ReplayStatus>\("replay-buffer-status-changed"/);
  assert.match(home, /invoke<HotkeyState>\("get_save_and_name_hotkey"\)/);
  assert.match(home, /invoke<ClipListResponse>\("list_clips"/);
  assert.match(home, /limit: 12/);
  assert.match(home, /set_clip_favorite/);
  assert.match(home, /<ClipPlayer/);
  assert.match(home, /onEditClip\(clip\)/);
  assert.match(home, /onOpenClips/);
  assert.match(home, /className="home-capture-tools"/);
  assert.match(home, /id="home-quick-panel"/);
  assert.match(home, /aria-expanded=\{quickPanel === "capture"\}/);
  assert.match(home, /toggleQuickPanel\("capture"\)/);
  assert.match(home, /document\.addEventListener\("pointerdown", closeOnOutsideClick\)/);
  assert.match(home, /event\.key === "Escape"/);
  assert.match(home, /Save &amp; Name/);
  assert.match(home, /updateQuickPreferences/);
  assert.match(home, /replayDurationSeconds/);
  assert.match(home, /replayFrameRate/);
  assert.match(home, /replayQuality/);
  assert.match(home, /replayEncoder/);
  assert.match(home, /set_save_replay_hotkey/);
  assert.match(home, /set_save_and_name_hotkey/);
  assert.match(home, /start_replay_buffer/);
  assert.match(home, /list_capture_monitors/);
  assert.match(home, /Game Detection/);
  assert.match(home, /Replay Saved overlay/);
  assert.match(styles, /Phase 2 — Moments-density Home workspace/);
  assert.match(styles, /Home quick panels/);
  assert.match(styles, /home-quick-form/);
  assert.match(styles, /home-quick-hotkey-editor/);
  assert.match(styles, /--rail-width: 300px/);
  assert.match(styles, /grid-template-columns: repeat\(auto-fill, minmax\(310px, 1fr\)\)/);
  assert.doesNotMatch(home, /delete_clip|set_clip_pinned|set_clip_collection_membership/);
});

test("Replay defaults persist across Home, Settings, Replay, and automatic game starts", () => {
  const home = readFileSync(new URL("../src/pages/HomePage.tsx", import.meta.url), "utf8");
  const settings = readFileSync(new URL("../src/pages/SettingsPage.tsx", import.meta.url), "utf8");
  const replay = readFileSync(new URL("../src/pages/ReplayPage.tsx", import.meta.url), "utf8");
  const preferences = readFileSync(new URL("../src-tauri/src/preferences.rs", import.meta.url), "utf8");
  const detection = readFileSync(new URL("../src-tauri/src/game_detection.rs", import.meta.url), "utf8");

  for (const field of ["replayDurationSeconds", "replayFrameRate", "replayQuality", "replayEncoder"]) {
    assert.match(home, new RegExp(field));
    assert.match(settings, new RegExp(field));
    assert.match(replay, new RegExp(field));
  }
  for (const field of ["replay_duration_seconds", "replay_frame_rate", "replay_quality", "replay_encoder"]) {
    assert.match(preferences, new RegExp(field));
    assert.match(detection, new RegExp(field));
  }
  assert.doesNotMatch(settings, /const \[clipLength, setClipLength\]/);
  assert.doesNotMatch(settings, /const \[resolution, setResolution\]/);
});

test("full Clips workspace keeps management features in the dense thumbnail-first layout", () => {
  const clips = readFileSync(new URL("../src/pages/ClipsPage.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");

  assert.match(clips, /className="page page-clips"/);
  assert.match(clips, /className="clip-card-media"/);
  assert.match(clips, /className="clip-card-duration"/);
  assert.match(clips, /Local Library/);
  assert.match(clips, /create_collection_command/);
  assert.match(clips, /set_clip_collection_membership/);
  assert.match(clips, /set_clip_pinned/);
  assert.match(clips, /delete_clip/);
  assert.match(styles, /Phase 2 — dense full Library workspace/);
  assert.match(styles, /repeat\(auto-fill, minmax\(294px, 1fr\)\)/);
});

test("Replay control workspace uses the compact Phase 2 shell without hiding diagnostics", () => {
  const replay = readFileSync(new URL("../src/pages/ReplayPage.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");

  assert.match(replay, /className="page-header replay-page-header"/);
  assert.match(replay, /Capture workspace/);
  assert.match(replay, /replay-page-badge/);
  assert.match(replay, /replay-capture-test-diagnostics/);
  assert.match(replay, /Capture diagnostics/);
  assert.match(styles, /Phase 2 — compact Replay control workspace/);
  assert.match(styles, /grid-template-columns: minmax\(560px, 1\.45fr\) minmax\(330px, \.82fr\)/);
});

test("Editor presents the full non-destructive engine in the focused workbench", () => {
  const editor = readFileSync(new URL("../src/pages/EditorPage.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");

  assert.match(editor, /editor-title-eyebrow/);
  assert.match(editor, /editor-header-facts/);
  assert.match(editor, /editor-stage-play/);
  assert.match(editor, /Mix every saved stem/);
  assert.match(editor, /data-track-role=\{track\.role\}/);
  assert.match(editor, /--editor-gain/);
  assert.match(editor, /Shape the final clip/);
  assert.match(editor, /Export new clip/);
  assert.match(editor, /Original safe/);
  assert.match(styles, /Phase 2 — focused Editor workbench/);
  assert.match(styles, /repeat\(auto-fit, minmax\(245px, 1fr\)\)/);
  assert.match(styles, /editor-mixer-track\[data-track-role="VoiceChat"\]/);
  assert.match(styles, /editor-stage-play/);
  assert.match(editor, /splitAtPlayhead/);
  assert.match(editor, /deleteSelectedSegment/);
  assert.match(editor, /trimEditorSegment/);
  assert.match(editor, /start_editor_export/);
});

test("Settings presents every real control in the indexed control-center workspace", () => {
  const settings = readFileSync(new URL("../src/pages/SettingsPage.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");

  assert.match(settings, /SlickClip control center/);
  assert.match(settings, /className="settings-workbench"/);
  assert.match(settings, /className="settings-index"/);
  assert.match(settings, /SettingsIndexItem number="01" title="General"/);
  assert.match(settings, /SettingsIndexItem number="06" title="Advanced"/);
  assert.match(settings, /openSettingsCategory/);
  assert.match(settings, /Saved automatically/);
  assert.match(settings, /Standalone by design/);
  assert.match(settings, /SettingsCategory id="settings-capture"/);
  assert.match(settings, /SettingsCategory id="settings-hotkeys"/);
  assert.match(settings, /SettingsCategory id="settings-storage"/);
  assert.match(settings, /SettingsCategory id="settings-game-detection"/);
  assert.match(styles, /Phase 2 — indexed Settings control center/);
  assert.match(styles, /grid-template-columns: 238px minmax\(0, 1fr\)/);
  assert.match(styles, /\.settings-index/);
});

test("Replay Roulette presents the existing weighted picker as a focused discovery screen", () => {
  const roulette = readFileSync(new URL("../src/pages/ReplayRoulettePage.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");

  assert.match(roulette, /Library wildcard/);
  assert.match(roulette, /roulette-header-facts/);
  assert.match(roulette, /roulette-filter-controls/);
  assert.match(roulette, /Less-watched first/);
  assert.match(roulette, /No immediate repeats/);
  assert.match(roulette, /Pick My Replay/);
  assert.match(roulette, /Pick Another/);
  assert.match(roulette, /copyClip\(selectedClip\)/);
  assert.match(roulette, /selectRouletteClip\(clips, recentIds\.current\)/);
  assert.match(styles, /Phase 2 — focused Replay Roulette/);
  assert.match(styles, /roulette-card-shadow/);
});

test("Help presents beginner guidance in the indexed quick-answer workspace", () => {
  const help = readFileSync(new URL("../src/pages/HelpPage.tsx", import.meta.url), "utf8");
  const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");

  assert.match(help, /Learn SlickClip/);
  assert.match(help, /Play first\. Save the moment after\./);
  assert.match(help, /className="help-replay-flow"/);
  assert.match(help, /className="help-workbench"/);
  assert.match(help, /className="help-index"/);
  assert.match(help, /openHelpSection/);
  assert.match(help, /section\.steps \? " help-section-primary"/);
  assert.match(help, /<kbd>\{saveReplayHotkey\}<\/kbd>/);
  assert.match(styles, /Phase 2 — beginner Help Center/);
  assert.match(styles, /grid-template-columns: 244px minmax\(0, 1fr\)/);
});

test("the redesigned desktop workspaces share a readable text baseline", () => {
  const styles = readFileSync(new URL("../src/App.css", import.meta.url), "utf8");

  assert.match(styles, /Phase 2 — readable desktop type scale/);
  assert.match(styles, /\.page-home, \.page-clips, \.page-replay, \.editor-page, \.settings-page, \.roulette-page, \.page-help/);
  assert.match(styles, /font-size: 12\.5px !important/);
  assert.match(styles, /\.sidebar \.nav-item/);
  assert.match(styles, /font-size: 14px !important/);
  assert.match(styles, /\.page-help \.help-intro h2/);
});
