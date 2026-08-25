# SlickClip 1.0.2 Friends Release Gate

The applicable product, Windows, installer, and updater gates below have passed for the private SlickClip 1.0.2 friends build. Automated checks support these results but do not replace the completed real-Windows and clean-machine validation. Authenticode publisher signing and SmartScreen reputation remain unresolved, so this approval does not extend to a public GitHub Release.

## Build and packaging

- Version, visible branding, executable metadata, taskbar/tray icons, installer, and updater all identify SlickClip consistently.
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

- Premium UI, Replay Roulette, splash, tray/background behavior, Replay Saved overlay, arbitrary hotkeys and Settings, game detection/auto-arm, storage destructive cleanup, packaged executable, installer, updater A-to-B, and Clips multi-select/batch actions passed manual validation.
- Stage 24 confirmed that protection excludes clips from automatic cleanup without blocking explicit manual deletion.
- Stage 25 passed packaged executable, migration, installer, clean-machine workflow, and uninstall preservation checks.
- Stage 26 passed the sequential updater A-to-B workflow. The historical 1.0.1 test candidate must not be reused.
- The final 1.0.2 polish still requires the short smoke test in `MANUAL_VALIDATION_PENDING.md` after building the intended friends artifact.

## Explicitly deferred or post-v1 scope

- The waveform experiment is deferred and is not a v1.0 release requirement.
- Stage 27 Watch Party / Reaction Capture and Stage 27.1 participant-aware crop/reflow remain manually unverified. They are hidden from normal 1.0.2 friends-build navigation behind a release visibility flag; their implementation and backend architecture remain intact.
- Stage 27 v1 will use whole-window Discord reaction capture so participants can join or leave mid-recording; individual camera extraction is not required for that first Watch Party version.
- SlickEdit remains deferred/on the back burner and is not part of SlickClip 1.0.2.

## Release decision

Current status: **approved for private friends distribution as SlickClip 1.0.2 after the final smoke test, with the unsigned-publisher/SmartScreen limitation disclosed**. The product gates through Stage 26 and Clips multi-select have passed. No public GitHub Release is approved by this decision.

Public release remains blocked until an approved Authenticode identity/sign command and the intended production publishing inputs are available and verified. Record known limitations explicitly; do not represent the friends build as Authenticode-signed.
