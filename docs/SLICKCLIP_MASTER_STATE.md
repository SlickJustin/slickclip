# SlickClip Master Project State

Last restored: 2026-08-23. Always inspect the repository for the exact live state.

## Product

SlickClip is a standalone Windows game capture and replay application.

Core product rules:

- No OBS dependency for normal SlickClip workflows.
- Local storage; no account, subscription, ad, or watermark requirement.
- Original source masters remain immutable.
- Normal ClipPlayer playback uses Combined/default audio.
- Editor workflows may use independent Game, Voice Chat, Microphone, and Other stems.
- Finished Editor exports are flattened H.264/AAC MP4 files.

Primary stack:

- Tauri 2, React 19, and TypeScript
- Rust backend
- Windows Graphics Capture and native WASAPI audio
- SQLite Library metadata
- FFmpeg-based media assembly, preview, and export paths

Repository: `C:\Users\Jakea\source\replay-app`

## Verified delivery state

- Stages 0–18 are committed on `master`.
- Stage 19 Professional UI/UX Polish has provisional checkpoint `c62c81f`. Its automated checks pass, but manual visual/regression validation remains pending and is tracked in `docs/MANUAL_VALIDATION_PENDING.md`.
- Stage 20 Replay Roulette has provisional checkpoint `6b40cc6`. Its automated checks pass, but manual UI validation remains pending and is tracked in `docs/MANUAL_VALIDATION_PENDING.md`.
- Stage 21 Animated Launch Experience has provisional checkpoint `f5c14f2`. Its automated checks pass, but manual native startup/focus/reduced-motion validation remains pending and is tracked in `docs/MANUAL_VALIDATION_PENDING.md`.
- Stage 22 Tray, Background, Startup, and Save Overlay has provisional checkpoint `db7328c`. Its automated checks pass, but manual native tray/startup/focus/background-capture validation remains pending and is tracked in `docs/MANUAL_VALIDATION_PENDING.md`.
- Stage 23 Game Detection and Auto-Arm has provisional checkpoint `4c8e3ec`. Its automated checks pass, but manual representative-game/launcher/fullscreen/false-positive validation remains pending and is tracked in `docs/MANUAL_VALIDATION_PENDING.md`.
- Stage 24 has not started.
- The waveform experiment was deliberately deferred and remains in Git stash as `Stage 19 waveform experiment - deferred`.
- The four project-control documents were accidentally committed as empty files in commit `07b3216`; this document set restores their intended content.

Never rely on this file for the current commit hash or stash index. Use Git to inspect both.

## Completed systems through Stage 18

### Capture and replay

- Native Windows Graphics Capture for display/window targets.
- Realtime constant-frame-rate scheduling.
- Rolling replay video segments and synchronized rolling audio.
- Replay Buffer start/stop/status lifecycle.
- Save Replay assembly and progress reporting.
- Global Save Replay hotkey and hotkey test workflow.
- Hardware encoder capability handling and fallback architecture.
- H.264/HEVC capture architecture and AV1 feasibility/probe work.

### Audio

- Native WASAPI microphone capture.
- Per-process loopback for Game, Voice Chat/Discord, and Other sources.
- Synchronized multitrack replay masters.
- Combined/default playback representation.
- Audio capture test and diagnostic paths.

### Library

- Persistent SQLite clip metadata and migrations.
- Filesystem reconciliation and owned-path safety.
- Thumbnails and H.264 preview fallback/cache.
- Search, sorting, grid preferences, Favorites, and Collections.
- Recently Watched, Last Watched, Play Count, and storage summary.
- Copy Clip through the native Windows file clipboard.
- Open file/folder, rename, delete, and persisted UI preferences.

### Player

- In-app custom SlickClip player.
- Graphite/purple branded controls.
- Combined/default audio during normal playback.
- Persisted volume and mute state.

### Nondestructive Editor

- Edit Decision List with integer-microsecond timing.
- Separate source-time and edited-time mapping.
- Trim, split, delete, Undo, Redo, and Reset.
- Contiguous preview playback across cuts.
- Original master remains unchanged.

### Editor mixer and export

- Game, Voice Chat, Microphone, Other, and Combined fallback tracks.
- 0–300% gain, Mute, Solo, Reset Audio, and synchronized preview.
- Flattened H.264/AAC export from EDL plus mixer decisions.
- Progress, cancellation, hardware/software encoder fallback, and Library indexing.

## Stage 19 provisional checkpoint

The Stage 19 UI-polish checkpoint modified:

- `src/App.css`
- `src/components/AudioCaptureTest.tsx`
- `src/components/ClipPlayer.tsx`
- `src/pages/ClipsPage.tsx`
- `src/pages/EditorPage.tsx`
- `src/pages/ReplayPage.tsx`

Implemented polish includes typography, spacing, color and motion tokens; sidebar, button, card and control states; clearer Replay, Clips, Player, Editor, Mixer, Timeline and Settings presentation; collapsed diagnostics; larger Editor workspace/preview; responsive layouts; custom dark/purple scrollbars; and strengthened reduced-motion behavior.

Automated results reported for this Stage 19 diff:

- `npm test`: 62 passed.
- `npm run build`: passed.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 150 passed.
- `cargo fmt -- --check`: passed.
- `git diff --check`: passed.
- No lint script exists.

These results do not replace the outstanding manual visual/regression gate. Stage 19 remains provisionally complete rather than manually verified.

## Stage 20 provisional checkpoint

Stage 20 adds a dedicated Replay Roulette page backed only by the existing Library query and ClipPlayer interfaces. It provides Favorites and Collection filters, weighted selection that favors less-played and less-recent clips, a bounded in-session recent-pick exclusion, responsive loading/empty/error/result presentation, and direct playback/copy actions through existing trusted clip identifiers. No capture, database, Editor, export, or waveform implementation changed.

Automated results reported for this Stage 20 diff:

- `npm test`: 67 passed, including five Replay Roulette selection tests.
- `npm run build`: passed.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 150 passed.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed.
- No lint script exists.

These results do not replace the outstanding manual UI gate. Stage 20 remains provisionally complete rather than manually verified.

## Stage 21 provisional checkpoint

Stage 21 adds a dedicated Vite splash entry point and Tauri splash window while preserving the existing single backend initialization path. The main window starts hidden; after setup is complete, the splash invokes an idempotent native coordinator that reveals/focuses the main window and closes the splash. A bounded native fallback prevents a failed splash asset or script from leaving SlickClip permanently hidden. CSS supplies restrained purple hex/progress motion and a static reduced-motion state.

Automated results reported for this Stage 21 diff:

- `npm test`: 67 passed.
- `npm run build`: passed and emitted both main and splash entry points.
- `cargo check`: passed, including Tauri configuration validation.
- `cargo test -- --nocapture`: 150 passed.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed.
- No lint script exists.

These results do not replace the outstanding native startup/focus/appearance gate. Stage 21 remains provisionally complete rather than manually verified.

## Stage 22 provisional checkpoint

Stage 22 adds one Tauri desktop-integration layer over the existing replay, save, preference, and window managers. It provides a live tray status item plus Open/Save Replay/Quit actions, persisted close-or-minimize-to-tray behavior, background launch through a quoted per-user Windows Run entry, and a hidden non-focusable save-overlay WebView shown only after the existing save worker successfully completes. The tray polls existing manager status; it does not create another capture or save path. Preference schema v2 adds only `startWithWindows`, `closeToTray`, and `saveOverlayEnabled`. Automatic Replay Buffer startup remains intentionally disabled until Stage 23 can select and verify an intended target.

Automated results reported for this Stage 22 diff:

- `npm test`: 67 passed.
- `npm run build`: passed and emitted main, splash, and save-overlay entry points.
- `cargo check`: passed with Tauri tray and Windows Registry features enabled.
- `cargo test -- --nocapture`: 152 passed, including startup-command and overlay-position coverage.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed.
- No lint script exists.

These results do not replace the outstanding Windows tray/startup/focus/background-capture gate. Stage 22 remains provisionally complete rather than manually verified.

## Stage 23 provisional checkpoint

Stage 23 adds a two-second background detector that reuses existing capturable-window enumeration and the existing Replay Buffer manager. Dimension/title/process heuristics produce review-only suggestions. Auto-arm is opt-in and additionally requires exactly one live window whose process the user explicitly approved; explicit exclusions win, and the native target is resolved again by the normal replay start path. A successful auto-arm uses Automatic encoding, a 120-second/60 FPS buffer, and a Game process-audio track for the detected PID. The Ready toast/overlay waits for the real replay state to become `running`. Only buffers tracked as auto-armed are stopped when their game closes, detection is disabled, or approval is removed. Preference schema v3 persists the feature toggle, auto-arm toggle, approvals, and exclusions with normalization and bounded lists.

Automated results reported for this Stage 23 diff:

- `npm test`: 67 passed.
- `npm run build`: passed.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 155 passed, including suggestion-only, approval/exclusion precedence, and launcher/productivity filtering coverage.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed.
- No lint script exists.

These results do not replace the outstanding representative real-game/launcher/fullscreen/false-positive gate. Stage 23 remains provisionally complete rather than manually verified.

## Architecture invariants

- Realtime capture must not be blocked by Library, Editor, cache, clipboard, preference, or FFmpeg export work.
- Frontend operations should pass trusted identifiers; backend code resolves and validates owned paths.
- Collections are metadata-only and do not move clip files.
- Editor operations never physically cut or overwrite the source master.
- Normal ClipPlayer does not expose individual stems.
- Capture, replay assembly, media cache, Library, Editor, and export should reuse existing managers and queues rather than creating parallel systems.

## Branding and compatibility

Product name: SlickClip.

Accepted visual direction: graphite/near-black, royal purple, restrained and professional, with a subtle sci-fi/military-tech influence. The current SlickClip icon and sidebar logo are accepted unless the user explicitly reopens branding.

Some migration-sensitive identifiers and paths still contain `JustIn Replay`, including the current Videos subdirectories and possibly application-data identifiers. Do not rename them piecemeal. Stage 25 owns the migration and must preserve existing clips, databases, preferences, previews, and caches.

## Deferred waveform decision

The waveform feature was attempted and intentionally deferred. Its old work is preserved separately in Git stash. It is not part of the current v1.0 plan and must not be restored, applied, dropped, or recreated without explicit instruction.

## Planned Watch Party direction

Stage 27 adds a Watch Party / Reaction Capture mode intended to replace the user's tedious OBS setup for long-form event and reaction recording.

The v1 design captures two live video sources: a Main Content window and the entire Discord call/popout window as one Reaction source. Discord remains responsible for arranging participant tiles. If someone joins or leaves after recording starts, Discord changes the same captured window and SlickClip records the updated layout automatically. Participants do not need to be present before recording begins.

This requires simultaneous Windows Graphics Capture sources, GPU composition, independent content/voice/microphone audio handling, safe multi-hour segmented recording, finalization/recovery, and preset layouts. It is planned work only; no Stage 27 application code exists yet.

A later Stage 27.1 may detect and crop individual Discord camera tiles, position them independently, and dynamically reflow 2/3/4-person layouts. Because Discord does not provide clean per-participant video streams to SlickClip, this participant-aware crop/reflow is explicitly an advanced option, not a v1 requirement for Watch Party.
