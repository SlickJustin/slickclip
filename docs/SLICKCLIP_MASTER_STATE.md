# SlickClip Master Project State

Last updated: 2026-08-31. Always inspect the repository for the exact live state.

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

- SlickClip 1.0.3 is the current validated friends checkpoint. Replay now resolves the game's physical monitor once and launches one supervised bundled FFmpeg `ddagrab` child for that display. FFmpeg owns Desktop Duplication, D3D11 frames, CFR delivery, encoding, timestamps, and two-second MP4 segments; Rust retains the logical Replay clock, rolling retention, native independent WASAPI stems, save assembly, indexing, and UI. Alt-Tab and presentation changes do not retarget or restart capture. An unexpected owned-child exit has a three-restart same-display budget while audio and the logical session remain alive. There is no OBS dependency and no fallback to the retired custom Replay DXGI loop. Watch Party's separate WGC implementation is intentionally unchanged and remains hidden/deferred. The real-game ten-minute gate, 30-minute soak, 1.0.3 installer, 1.0.2-to-1.0.3 upgrade preservation, and full-product installed-app regression were reported passed on 2026-08-30. The source audit, final automated gate, UI review, larger-type gate, and visual-only branded-installer gate passed on 2026-08-31. The user authorized a local checkpoint commit but not a push; the user also reported publishing a GitHub Release before this checkpoint existed, so its tag/source alignment remains pending.

- The user subsequently and explicitly authorized the consumer UI redesign that had originally been held out of the FFmpeg-backend stage. Home, Clips, Replay, Editor, Settings, Replay Roulette, and Help now use the compact graphite/purple workspace language while preserving the existing product engines and direct-edit controls. Every screen passed user visual/interaction review on 2026-08-31. The final gate reported 99 frontend tests and 275 Rust tests passed, plus successful frontend production build, `cargo check`, Rust formatting, diff checking, and native Tauri release build without bundling.

- The 1.0.3 polish layer routes the non-focus-stealing Replay Saved overlay to the Replay session's immutable captured-monitor desktop origin rather than the primary/current overlay monitor. Settings also provides a separate optional persisted Save & Name global hotkey. It invokes the same exactly-once save/index worker and requests a name only after successful indexing; regular Save Replay remains non-focus-stealing and unchanged. Save & Name intentionally brings SlickClip forward after success, where a cancel-safe dialog renames only Library metadata and never the source file.

- The first 1.0.3 NSIS branding artifact is permanently withdrawn after the user reported that the Windows pointer disappeared immediately upon double-click, before setup displayed or SlickClip launched, and remained absent until restart. The repository contains no cursor-hiding or system-cursor replacement call. Its setup/uninstaller executable-icon overrides remain disabled and that artifact must never be reused. The later separately authorized visual-only isolation candidate enables only Tauri's supported header/sidebar bitmaps, retains normal executable icons, and uses the exact slogan `Made to capture the DAWGs worst moments.` The resulting unsigned installer is 88,439,111 bytes with SHA-256 `3F8FD05716E3A53AB3468A104875428BAA27AE60E0360CC70B7DA501FECFEA4F`; its branded artwork/slogan, staged open/cancel cursor test, installation, launch, larger typography, existing clips, and existing settings passed on the primary Windows PC on 2026-08-31. It is the approved private friends installer. The stock installer with SHA-256 `3EBEE545817F381A7846764D035D800299BC3303C720893414A54D54C9C3ADDC` remains preserved as the known-safe fallback.

- Display Capture access-loss recovery releases only the invalid duplication object, retains reusable D3D11 resources where possible, requires the same display identity and stable geometry across three 250 ms polls, backs off failed recreation by 500 ms, and remains bounded by one interruptible ten-second deadline. The logical Replay, configured audio workers, monotonic QPC clock, encoder/ring ownership, and finalized retained segments survive recovery. Save/Ready remain disabled until a production frame restores `Healthy` and explicitly rebases CFR without historical catch-up.

- Stages 0–18 remain complete and committed on `master`.
- Stages 19–23 have passed their automated and manual gates: premium UI, Replay Roulette, splash, tray/background/Replay Saved overlay, arbitrary hotkeys and Settings, and game detection/auto-arm are verified.
- Stage 24 Storage Safety has passed its disposable-data destructive cleanup gate. `Protect from Cleanup` excludes clips only from automatic quota cleanup; explicit confirmed manual deletion remains allowed.
- Stage 25 migration/distribution has passed packaged executable, installer, migration, and end-to-end release validation for the private friends build.
- Stage 26 updater has passed the sequential A-to-B install/update/restart preservation test. The 1.0.1 candidate used for that test is historical and must not be reused.
- Clips multi-select and batch actions are committed at `9371391` and manually verified, including intentional manual deletion of clips protected from cleanup.
- SlickClip 1.0.2 remains the last historical friends release. Do not modify its release artifacts; 1.0.3 is the current validated friends build. Authenticode publisher signing and resulting SmartScreen reputation remain unresolved, so the manually uploaded installer is not a production signed-updater release.
- Stage 27 Watch Party (`cf82ba8`) and Stage 27.1 participant-aware layouts (`f38be14`) remain manually unverified and are hidden from normal friends-build navigation by a frontend release visibility flag. Their backend implementation is retained unchanged for later validation.
- SlickEdit remains deferred/on the back burner and is not part of SlickClip 1.0.3.
- The waveform experiment was deliberately deferred and remains in Git stash as `Stage 19 waveform experiment - deferred`.
- The four project-control documents were accidentally committed as empty files in commit `07b3216`; this document set restores their intended content.

The detailed Stage 19–27.1 sections below preserve implementation history and the automated results recorded at each checkpoint. Their original provisional wording is superseded by the current verified-delivery summary above and by `docs/MANUAL_VALIDATION_PENDING.md`.

Never rely on this file for the current commit hash or stash index. Use Git to inspect both.

## Completed systems through Stage 18

### Capture and replay

- Supervised bundled FFmpeg `ddagrab` capture for Replay's selected physical display.
- Windows Graphics Capture remains available only to separate low-level capture tests and the deferred Watch Party subsystem; it is not a Replay product policy or fallback.
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
- Synchronized multitrack replay masters sharing the same monotonic QPC session clock as video.
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

## Stage 19 historical checkpoint

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

The later manual visual/regression gate passed; see the verified delivery state above.

## Stage 20 historical checkpoint

Stage 20 adds a dedicated Replay Roulette page backed only by the existing Library query and ClipPlayer interfaces. It provides Favorites and Collection filters, weighted selection that favors less-played and less-recent clips, a bounded in-session recent-pick exclusion, responsive loading/empty/error/result presentation, and direct playback/copy actions through existing trusted clip identifiers. No capture, database, Editor, export, or waveform implementation changed.

Automated results reported for this Stage 20 diff:

- `npm test`: 67 passed, including five Replay Roulette selection tests.
- `npm run build`: passed.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 150 passed.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed.
- No lint script exists.

The later manual Replay Roulette UI gate passed; see the verified delivery state above.

## Stage 21 historical checkpoint

Stage 21 adds a dedicated Vite splash entry point and Tauri splash window while preserving the existing single backend initialization path. The main window starts hidden; after setup is complete, the splash invokes an idempotent native coordinator that reveals/focuses the main window and closes the splash. A bounded native fallback prevents a failed splash asset or script from leaving SlickClip permanently hidden. CSS supplies restrained purple hex/progress motion and a static reduced-motion state.

Automated results reported for this Stage 21 diff:

- `npm test`: 67 passed.
- `npm run build`: passed and emitted both main and splash entry points.
- `cargo check`: passed, including Tauri configuration validation.
- `cargo test -- --nocapture`: 150 passed.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed.
- No lint script exists.

The later native startup/focus/appearance gate passed; see the verified delivery state above.

## Stage 22 historical checkpoint

Stage 22 adds one Tauri desktop-integration layer over the existing replay, save, preference, and window managers. It provides a live tray status item plus Open/Save Replay/Quit actions, persisted close-or-minimize-to-tray behavior, background launch through a quoted per-user Windows Run entry, and a hidden non-focusable save-overlay WebView shown only after the existing save worker successfully completes. The tray polls existing manager status; it does not create another capture or save path. Preference schema v2 adds only `startWithWindows`, `closeToTray`, and `saveOverlayEnabled`. Automatic Replay Buffer startup remains intentionally disabled until Stage 23 can select and verify an intended target.

Automated results reported for this Stage 22 diff:

- `npm test`: 67 passed.
- `npm run build`: passed and emitted main, splash, and save-overlay entry points.
- `cargo check`: passed with Tauri tray and Windows Registry features enabled.
- `cargo test -- --nocapture`: 152 passed, including startup-command and overlay-position coverage.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed.
- No lint script exists.

The later Windows tray/startup/focus/background-capture gate passed; see the verified delivery state above.

## Stage 23 historical checkpoint

Stage 23 adds a two-second background detector that reuses existing capturable-window enumeration and the existing Replay Buffer manager. Dimension/title/process heuristics produce review-only suggestions. Auto-arm is opt-in and additionally requires exactly one live window whose process the user explicitly approved; explicit exclusions win, and the native target is resolved again by the normal replay start path. A successful auto-arm uses Automatic encoding, a 120-second/60 FPS buffer, and a Game process-audio track for the detected PID. The Ready toast/overlay waits for the real replay state to become `running`. Only buffers tracked as auto-armed are stopped when their game closes, detection is disabled, or approval is removed. Preference schema v3 persists the feature toggle, auto-arm toggle, approvals, and exclusions with normalization and bounded lists.

Automated results reported for this Stage 23 diff:

- `npm test`: 67 passed.
- `npm run build`: passed.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 155 passed, including suggestion-only, approval/exclusion precedence, and launcher/productivity filtering coverage.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed.
- No lint script exists.

The later representative real-game/launcher/fullscreen/false-positive gate passed; see the verified delivery state above.

## Stage 24 historical checkpoint

Stage 24 adds a persisted 1 GB–10 TB Library quota and a schema-v3 `pinned` flag exposed as Protected, deliberately separate from Favorite. Settings performs a backend-owned dry run that lists the exact oldest unprotected clips required by the quota, protected totals, projected reclaim, and whether protected capacity prevents reaching the quota. Execution accepts only an opaque one-use preview token, re-plans from current database metadata, rejects any changed scope, pre-validates every selected clip, and routes deletion through the existing trusted-ID, canonical owned-path, direct-child regular-MP4, database-row, and cache cleanup path. Quota enforcement is explicit rather than silently destructive in the background. Preference schema v4 persists the quota.

Automated results reported for this Stage 24 diff:

- `npm test`: 67 passed.
- `npm run build`: passed.
- `cargo check`: passed.
- `cargo test -- --nocapture`: 160 passed, including cleanup order, protected preservation, stale-scope comparison, migration/persistence, owned-cache removal, and outside-path refusal coverage.
- `cargo fmt -- --check`: passed with the known environment canonicalization warning.
- `git diff --check`: passed.
- No lint script exists.

The later destructive-safety test using disposable real clips passed. The verified semantics are `Protect from Cleanup`, not protection from explicit manual deletion.

## Stage 25 historical checkpoint

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

The later clean-machine, disposable-data migration, installer, and end-to-end media gates passed for private friends distribution. Authenticode remains unresolved.

## Stage 26 historical implementation checkpoint

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

The later sequential updater A-to-B install/restart and data-preservation gate passed; the 1.0.1 test candidate is historical and must not be reused. Public distribution remains blocked on an approved Windows code-signing identity/`signCommand` and the intended production publishing inputs. Private friends distribution must disclose the unsigned-publisher/SmartScreen limitation.

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
