# SlickClip 1.0.3 Friends Release Gate

## SlickClip 1.0.3 addendum

SlickClip 1.0.3 is a separate validated checkpoint and does not modify the historical 1.0.2 release. In addition to every applicable gate below, 1.0.3 must prove the default Any detected game workflow, migration safety, exclusion precedence, two-poll stabilization, deterministic foreground-first selection, manual-display fallback, and exactly-once automatic start/stop and Ready transitions. Replay resolves one physical monitor to its exact DXGI adapter/output pair and runs one hidden owned bundled FFmpeg `ddagrab` child. Alt-Tab and later foreground/presentation changes cannot retarget or restart it. FFmpeg must pass compiled and real display/encoder probes; failure must not invoke the retired custom Replay DXGI loop. Unexpected exit permits at most three sequential same-display restarts while the logical session, retained segments, monotonic QPC clock, and native audio workers survive. Stop/game exit must finalize and reap only the owned child. Video and every enabled Game, Voice Chat, Microphone, and Other WASAPI track use one monotonic timeline; restart gaps are removed piecewise from all stems, and saved output retains Combined/default plus independent stems. A fully assembled and indexed Save emits exactly one Replay Saved overlay; failure does not. The real-game Replay gates, new 1.0.3 installer, 1.0.2-to-1.0.3 upgrade preservation, and full-product installed-app regression were reported passed on 2026-08-30. A local checkpoint commit was explicitly authorized on 2026-08-31; push and tag changes remain unauthorized.

The Replay Saved overlay must appear on the work area of the physical monitor captured by that immutable session, including when SlickClip lives on another monitor, and regular Save must not take game focus. The optional Save & Name binding must be distinct, persisted, disableable, and routed through the same one-save worker. Its naming dialog may appear only after successful indexing; cancelling keeps the automatic name, renaming changes Library metadata only, and failed/duplicate saves must never open it.

The applicable product, Windows, installer, and updater gates below have passed for the private SlickClip 1.0.3 friends build. Automated checks support these results but do not replace the completed real-Windows and clean-machine validation. Authenticode publisher signing and SmartScreen reputation remain unresolved, so this approval does not make the manually uploaded friends installer a production signed-updater release.

## Build and packaging

- Version, visible branding, executable metadata, taskbar/tray icons, installer, and updater all identify SlickClip consistently.
- The first custom NSIS artifact with executable-icon overrides remains permanently withdrawn after its cursor-disappearance report. The final stock installer passed its safety gate, and the separately authorized visual-only candidate enables only the supported header/sidebar bitmaps while leaving setup/uninstaller executable-icon overrides unset. That new-hash candidate passed the staged cursor, install, launch, typography, branding, and data-preservation gate on 2026-08-31.
- A normal user can install and run SlickClip without Node.js, Rust, OBS, developer tools, or a separately installed FFmpeg.
- Updater integrity/signature behavior has passed A-to-B validation. Authenticode publisher signing is still unavailable, so friends must be told to expect Windows/SmartScreen warnings.
- Clean install, upgrade from the supported previous state, Update & Restart, and uninstall are tested.
- Upgrade and uninstall behavior do not unexpectedly delete user clips or settings.

## Capture and Replay Buffer

- Display/window target discovery and selection work.
- Replay Buffer starts, runs reliably under real load, reports useful failures, and stops cleanly.
- Realtime frame scheduling and rolling segment rotation remain stable.
- Save Replay works from the UI and global hotkey.
- Hardware encoder selection/fallback is safe and understandable.
- Game, Voice Chat, Microphone, and configured Other audio behave as designed.
- Saved video and audio remain synchronized.
- Replay Buffer remains stable while the Library, Editor, thumbnail/preview preparation, and export are active.
- Closing/minimizing/background and startup behavior match the documented tray settings.
- The Replay Saved overlay appears without stealing focus or disrupting a game.

## Library and storage

- Existing clips reconcile into the persistent Library without duplication or data loss.
- Search, sorting, grid preferences, Favorites, Collections, Recently Watched, Last Watched, Play Count, storage summary, Copy Clip, rename, open, and delete behave correctly.
- Collections remain metadata-only.
- Thumbnail/preview cache failures do not damage source clips.
- `Protect from Cleanup` remains distinct from Favorite and applies only to automatic storage cleanup.
- Storage quota cleanup removes only eligible oldest owned clips not protected from cleanup.
- Explicit manual deletion remains available for protected-from-cleanup clips, with confirmation copy that states manual deletion overrides cleanup protection.
- Cleanup, clip deletion, and cache cleanup reject paths outside SlickClip-owned roots and handle failures without cascading data loss.

## Player

- Supported clips open and play through the custom ClipPlayer.
- Play/pause, seek, volume, mute, fullscreen, Copy Clip, Open Folder, and close work.
- Normal playback uses Combined/default audio and does not expose Editor stems.
- H.264 preview fallback works where a source master is not directly WebView-compatible.
- Volume and mute preferences persist.

## Editor and mixer

- Original masters remain immutable.
- Trim, split, delete, Undo, Redo, Reset, seeking, and playback across cuts work in edited time.
- EDL timing and source/edited-time mapping remain correct.
- All structural edits apply consistently across audio tracks.
- Game, Voice Chat, Microphone, Other, and Combined fallback mixer behavior is correct.
- Gain, Mute, Solo, Reset Audio, and synchronized preview work.
- Editor/cache preparation does not block realtime capture.

## Export

- Export consumes the current EDL and mixer decisions and creates a new flattened shareable MP4.
- Output is H.264 video with AAC Combined audio when audio exists.
- Progress and cancellation work and partial failures are cleaned safely.
- Hardware encoder failure falls back to the supported software path.
- Successful exports are indexed into the Library.
- Export never modifies the original source master.

## UI and accessibility

- Replay, Clips, ClipPlayer, Editor, Mixer, Timeline, Settings, splash, tray, and overlays have a coherent graphite/purple SlickClip presentation.
- Text and controls remain readable at supported Windows scaling and desktop window sizes.
- Keyboard focus is visible and core flows remain keyboard-operable.
- Hover/pressed/focus feedback is consistent and restrained.
- `prefers-reduced-motion` removes decorative movement without hiding state changes.
- Engineering telemetry is collapsed or development-only unless needed for an active error.
- Responsive layouts do not overlap, clip, or hide required controls.

## Detection, startup, and background behavior

- Game detection works with representative games/launchers and has tested exclusions and manual overrides.
- Auto-arm never silently selects an obviously unintended target and can be disabled.
- Start-with-Windows is opt-in and reversible.
- Tray actions and application-exit semantics are clear.
- Replay Buffer status remains truthful when the main window is hidden or restored.

## Migration and user-data safety

- Migration from historical `JustIn Replay` identifiers/paths preserves clips, database records, preferences, Collections, Favorites, watch metadata, and required cache compatibility.
- Migrations are repeatable/idempotent where applicable and recover safely from interruption.
- Arbitrary frontend paths cannot escape owned directories.
- No release workflow uses destructive shell commands for normal file management.
- Database and preference corruption/failure paths produce recoverable errors where practical.

## Reliability and regression

- `npm test` passes.
- `npm run build` passes.
- `cargo check` passes.
- `cargo test -- --nocapture` passes.
- `cargo fmt -- --check` passes.
- `git diff --check` passes.
- No unexplained warnings, debug-only dependencies, test artifacts, or prominent development diagnostics remain in the release build.
- Representative long capture, repeated Save Replay, Editor/export, disk-pressure, restart, and clean-PC sessions pass.
- The release candidate commit is identified, reproducible, and the intended tree is clean.

## Verified manual delivery state

- The explicitly authorized 1.0.3 consumer UI redesign passed native screen-by-screen review for Home, Clips, Replay, Editor, Settings, Replay Roulette, and Help on 2026-08-31. Its final automated gate passed 99 frontend and 275 Rust tests plus the production frontend and native no-bundle builds.
- Premium UI, Replay Roulette, splash, tray/background behavior, Replay Saved overlay, arbitrary hotkeys and Settings, game detection/auto-arm, storage destructive cleanup, packaged executable, installer, updater A-to-B, and Clips multi-select/batch actions passed manual validation.
- Stage 24 confirmed that protection excludes clips from automatic cleanup without blocking explicit manual deletion.
- Stage 25 passed packaged executable, migration, installer, clean-machine workflow, and uninstall preservation checks.
- Stage 26 passed the sequential updater A-to-B workflow. The historical 1.0.1 test candidate must not be reused.
- The final 1.0.3 branded friends artifact passed the staged cursor, installation, launch, typography, and data-preservation smoke test in `MANUAL_VALIDATION_PENDING.md`.

## Explicitly deferred or post-v1 scope

- The waveform experiment is deferred and is not a v1.0 release requirement.
- Stage 27 Watch Party / Reaction Capture and Stage 27.1 participant-aware crop/reflow remain manually unverified. They are hidden from normal 1.0.3 friends-build navigation behind a release visibility flag; their implementation and backend architecture remain intact.
- Stage 27 v1 will use whole-window Discord reaction capture so participants can join or leave mid-recording; individual camera extraction is not required for that first Watch Party version.
- SlickEdit remains deferred/on the back burner and is not part of SlickClip 1.0.3.

## Release decision

Current status: **SlickClip 1.0.3 has passed its functional, Replay, upgrade-preservation, installed-app, larger-typography, and final visual-only branded-installer cursor-safety gates**. The branded installer with SHA-256 `3F8FD05716E3A53AB3468A104875428BAA27AE60E0360CC70B7DA501FECFEA4F` is the current private friends candidate; the stock installer with SHA-256 `3EBEE545817F381A7846764D035D800299BC3303C720893414A54D54C9C3ADDC` remains the known-safe fallback. The earlier executable-icon artifact remains blocked and withdrawn and must never be distributed or retested. The user reported publishing the GitHub Release before the checkpoint commit existed. After an explicitly authorized push, its tag must be verified and aligned with the checkpoint; Authenticode/SmartScreen remains unresolved.

A production signed-updater release remains blocked until an approved Authenticode identity/sign command and the intended production publishing inputs are available and verified. Record known limitations explicitly; do not represent the manually uploaded friends build as Authenticode-signed.
