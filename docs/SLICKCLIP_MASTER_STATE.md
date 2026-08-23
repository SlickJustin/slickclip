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
- Stage 24 Storage Safety has provisional checkpoint `bdfc167`. Its automated checks pass, but manual destructive-safety validation with disposable data remains pending and is tracked in `docs/MANUAL_VALIDATION_PENDING.md`.
- Stage 25 Final SlickClip Migration and Distribution has provisional checkpoint `cc79345`. Its automated checks and unsigned NSIS bundle pass, but disposable-data migration and clean-machine install/capture/save/playback/edit/export/uninstall validation remain pending and are tracked in `docs/MANUAL_VALIDATION_PENDING.md`.
- Stage 26 Updater and Release Candidate has provisional implementation checkpoint `3322c46`. Its updater/release code and unsigned packaging checks pass, but signing credentials, hosted release infrastructure, a signed candidate, clean-PC upgrade/failure tests, and the complete release gate remain pending. SlickClip v1.0.0 is not approved for release.
- Stage 27 Watch Party / Reaction Capture has provisional checkpoint `cf82ba8` and is automatically clean. Its real Windows multi-hour/dynamic-participant/synchronization/failure/recovery/playback/Editor validation is pending in `docs/MANUAL_VALIDATION_PENDING.md`.
- Stage 27.1 Advanced Participant-Aware Reaction Layouts is provisionally implemented as an opt-in, confidence-gated layer with automatic whole-window fallback. Real Discord layout-variant validation remains pending.
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
- Configurable Library quota, independent Protected metadata, and explicit oldest-unprotected cleanup previews.

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

## Stage 24 provisional checkpoint

Stage 24 adds a persisted 1 GB–10 TB Library quota and a schema-v3 `pinned` flag exposed as Protected, deliberately separate from Favorite. Settings performs a backend-owned dry run that lists the exact oldest unprotected clips required by the quota, protected totals, projected reclaim, and whether protected capacity prevents reaching the quota. Execution accepts only an opaque one-use preview token, re-plans from current database metadata, rejects any changed scope, pre-validates every selected clip, and routes deletion through the existing trusted-ID, canonical owned-path, direct-child regular-MP4, database-row, and cache cleanup path. Quota enforcement is explicit rather than silently destructive in the background. Preference schema v4 persists the quota.

Automated results reported for this Stage 24 diff:

- `npm test`: 67 passed.
- `npm run build`: passed.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 160 passed, including cleanup order, protected preservation, stale-scope comparison, migration/persistence, owned-cache removal, and outside-path refusal coverage.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed.
- No lint script exists.

These results do not replace the outstanding destructive-safety test using disposable real clips. Stage 24 remains provisionally complete rather than manually verified.

## Stage 25 provisional checkpoint

Stage 25 completes the product-facing SlickClip 1.0.0 migration and creates a standalone current-user NSIS installer. Startup first performs a no-overwrite migration from the legacy application-data and Videos roots, rejects Windows reparse points throughout the source tree, transactionally rewrites only direct-child legacy MP4 Library paths, atomically rewrites cached JSON path metadata, and remains safe to retry. Automated coverage verifies clip bytes, favorites, protection, play count, collections, preferences, cache paths, idempotence, collision refusal, and the current-install/no-legacy no-op. The existing internal crate name, legacy environment-variable fallback, legacy process exclusion, and migration-source names remain intentionally compatible.

The bundle is branded SlickClip throughout, uses `com.slickclip.desktop`, emits `SlickClip.exe` and a per-user NSIS installer, and packages pinned checksum-verified static GPL FFmpeg/ffprobe sidecars plus the corresponding license and source notice. A source preparation script owns the target-suffixed Tauri sidecar names; generated dependency binaries stay ignored. New clips use the `SlickClip-<timestamp>` prefix and live under `Videos\SlickClip\Clips`.

Automated results reported for this Stage 25 diff:

- `npm test`: 67 passed.
- `npm run build`: passed.
- `npm run prepare:ffmpeg`: passed and re-verified the pinned files.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 163 passed, including three migration tests.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed; line-ending notices are advisory only.
- `npm run bundle`: passed without Rust warnings and emitted the unsigned NSIS bundle.
- Final `SlickClip.exe`: 15,172,608 bytes; SHA-256 `508C48716B5FAC46F840910E69F8A677D602853AB3ED935F678C342DA3B53AF1`; PE product/file version SlickClip 1.0.0.
- Final `SlickClip_1.0.0_x64-setup.exe`: 87,131,060 bytes; SHA-256 `B49C34D313E240DE305D44D978C5846480BDAE2A7E67043B8AC8584AC0755622`; PE product/file version SlickClip 1.0.0.
- Generated NSIS source inspection confirms current-user install mode, `com.slickclip.desktop`, both media sidecars, the FFmpeg license/source notice, and no uninstall rule targeting the Videos clip root.
- No lint script exists.

These results do not replace the clean-machine and disposable-data migration gate. The installer is unsigned, and code signing, updater/feed configuration, release legal review, and signed upgrade/rollback testing belong to Stage 26. Stage 25 remains provisionally complete rather than manually verified.

## Stage 26 provisional implementation checkpoint

Stage 26 adds the official Tauri signed-updater backend and a Settings experience for Check for Updates and Update & Restart. Release trust inputs are embedded only at compile time; ordinary local builds clearly disable update checks instead of accepting runtime-supplied keys. Checks are single-operation, HTTPS-only, use the updater's SemVer comparison, and re-check the expected version immediately before download. The complete installer is verified against the embedded updater public key before SlickClip shuts down replay, audio tests, hotkeys, exports, saves, and rolling capture through the normal exit cleanup routine and launches the passive installer.

`npm run bundle:release` is a fail-closed release-machine workflow. It requires the endpoint, updater public/private key inputs, versioned artifact URL, and a real Tauri Windows sign command; creates a temporary configuration overlay; enables updater artifacts; builds; requires valid Authenticode on the app and NSIS installer; requires a nonempty updater signature; generates `latest.json`; prints SHA-256 hashes; and removes the temporary overlay. `docs/RELEASE_PROCESS.md` defines key custody, artifact-first/manifest-last publishing, failure behavior, and higher-SemVer recovery. It never publishes or invents credentials.

Automated results reported for this Stage 26 diff:

- `npm test`: 67 passed.
- `npm run build`: passed.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 166 passed, including HTTPS/trust-input, expected-version, and one-operation updater tests.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed; line-ending notices are advisory only.
- Release-script validation with non-secret fixtures: passed.
- Release-script validation without inputs: failed closed on missing `SLICKCLIP_UPDATER_ENDPOINT`, as intended.
- Unsigned `npm run bundle`: passed without compiler warnings and proved the updater-enabled app still packages.
- Local installer: 88,195,355 bytes; SHA-256 `8C556C4727E97AA4A9B2C1FB1227E476D86CD640432057152F6A341225522840`; Authenticode `NotSigned`; no updater `.sig` or `latest.json`, all expected for the non-release command.
- No lint script exists.

The remaining blocker is external and exact: provide the production updater key pair/public key custody decision, approved Windows code-signing identity and `signCommand` credentials, HTTPS feed/artifact URLs and publishing access. Then produce and manually validate signed sequential-version candidates on clean/disposable Windows machines. Until that occurs, Stage 26 is implementation-complete but not release-candidate-complete, and the release gate remains failed.

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

Stage 25 moved current product data to SlickClip-named roots. Intentional legacy names remain only where required to locate and migrate historical data, honor development environment-variable overrides, exclude the former executable during game detection, or preserve internal/vendor compatibility. Do not remove those compatibility paths without a separately tested migration decision.

## Deferred waveform decision

The waveform feature was attempted and intentionally deferred. Its old work is preserved separately in Git stash. It is not part of the current v1.0 plan and must not be restored, applied, dropped, or recreated without explicit instruction.

## Stage 27 provisional Watch Party implementation

Stage 27 adds a dedicated Watch Party / Reaction Capture mode intended to replace the user's tedious OBS setup for long-form event and reaction recording.

The v1 design captures two live video sources: a Main Content window and the entire Discord call/popout window as one Reaction source. Discord remains responsible for arranging participant tiles. If someone joins or leaves after recording starts, Discord changes the same captured window and SlickClip records the updated layout automatically. Participants do not need to be present before recording begins.

The native implementation runs two simultaneous WGC sessions into bounded latest-frame slots and a dedicated D3D11 shader compositor. It produces a fixed 1920x1080 H.264 canvas at 30 FPS with Reactions Right, Reaction Strip, and Picture-in-Picture presets. Source aspect changes reflow inside the selected preset without participant detection or cropping. The reaction selector accepts only a whole Discord desktop window, and Discord/Voice Chat audio must resolve to a Discord desktop process from the active audio-session list (which may be a different Electron child PID from the visible window).

Main Content, Voice Chat, and Microphone use the existing native WASAPI/QPC audio pipeline. Stop finalizes the last 30-second-or-shorter video segment, establishes the audio coverage barrier, and reuses the verified FFmpeg assembly/mux and Library indexing path. Normal output contains Combined/default audio plus the three independent stems. Replay Buffer and Watch Party are mutually exclusive, and existing Library/Editor architecture remains unchanged.

Every finalized video segment updates a flushed, atomic app-owned checkpoint. Recovery accepts only canonical finalized MP4 segments that are direct children of the selected Watch Party session, validates their CFR timeline, and creates a video-only Library clip from valid checkpointed material. After a verified Library output, temporary session media is removed only after direct-child, canonical-parent, name, and reparse-point checks; a cleanup failure retains it and surfaces a non-destructive warning. A source that closes retains its last frame and surfaces a clear Stop/finalize message; low disk space fails before opening the next segment. Application shutdown requests orderly Stop and joins the Watch Party worker.

The Save Replay hotkey remains scoped to Replay Buffer. Watch Party is continuous rather than rolling, and enabling a moment-save without a separately designed retention/pinning policy would risk unbounded long-form work or contention with finalization; the roadmap makes this optional only where safely permitted.

Automated results reported for this Stage 27 diff:

- `npm test`: 67 passed.
- `npm run build`: passed.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 177 passed, including real D3D11 shader compilation/composition, layout/reflow, exact segment cadence, source-loss, required audio/process binding, atomic replacement, outside-path recovery refusal, and scoped temporary-session cleanup tests.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed; line-ending notices are advisory only.
- No lint script exists.

Manual validation is still mandatory. No real Discord participant, multi-hour soak, real audio synchronization, disk-pressure, crash recovery, playback, or Editor behavior is claimed as passed.

## Stage 27.1 provisional participant-aware layer

The optional participant-aware checkbox adds a local-only visual detector above Stage 27 without changing its default. A bounded 48-column downsample estimates Discord's border/background color, finds separated visual tile components, accepts only balanced high-coverage 2/3/4-tile results, and samples at 2 Hz. A stability tracker requires three similar detections before enabling crops and returns to the entire Discord window after ten uncertain observations. The UI reports active tile count/confidence or explicit whole-window fallback.

The compositor's existing GPU path now accepts normalized UV crops through a D3D11 constant buffer and arranges 2, 3, or 4 detected tiles within the selected Reactions Right, Reaction Strip, or Picture-in-Picture reaction region. Participant images are never identified, named, uploaded, or persisted as detection records. The original whole Discord frame remains the only captured reaction source. No bot, Discord API, token, OCR, face recognition, or Editor architecture change was added.

This remains an experimental/manual-validation feature because Discord does not expose clean participant streams and can change its UI. False or uncertain detection must visibly fall back to Stage 27's whole-window composition, never silently omit the reaction source. Saved custom layouts were not added because the roadmap lists them as potential scope and persistence would not improve detector reliability before real Discord validation.

Automated results reported for this Stage 27.1 diff:

- `npm test`: 67 passed.
- `npm run build`: passed.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 181 passed, including synthetic 2/3/4-tile detection, unsupported-count fallback, stabilization/timeout, bounded crop placement, and the D3D11 UV crop path.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed; line-ending notices are advisory only.
- No lint script exists.

Manual validation is still mandatory; no real Discord tile detection accuracy is claimed as passed.
