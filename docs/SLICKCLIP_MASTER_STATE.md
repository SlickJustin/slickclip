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
- Stage 20 has not started.
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
