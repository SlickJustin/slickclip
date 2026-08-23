# SlickClip v1.0 Release Gate

SlickClip is ready for v1.0.0 only when every applicable requirement below passes on the release build. Automated checks support these gates but do not replace real Windows and clean-machine validation.

## Build and packaging

- Version, visible branding, executable metadata, taskbar/tray icons, installer, and updater all identify SlickClip consistently.
- A normal user can install and run SlickClip without Node.js, Rust, OBS, developer tools, or a separately installed FFmpeg.
- Release binaries and updater artifacts are signed as intended.
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
- Pin/Protect remains distinct from Favorite.
- Storage quota cleanup removes only eligible oldest-unprotected owned clips.
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

## Stage 19 manual gate still outstanding

Stage 19 now has a provisional automated checkpoint, but its manual UI gate remains outstanding. Validate Sidebar, Replay, Save Replay, Clips, ClipPlayer, Editor, Settings, responsive sizing, and reduced motion; confirm capture/save/playback/edit/export behavior did not regress. Provisional later-stage work does not clear this release gate.

## Explicitly deferred or post-v1 scope

- The waveform experiment is deferred and is not a v1.0 release requirement.
- Stage 27 Watch Party / Reaction Capture has a provisional post-v1 implementation checkpoint but remains manually unverified; Stage 27.1 participant-aware crop/reflow remains optional later work. Neither changes the still-failed Stage 0–26 v1.0 release decision.
- Stage 27 v1 will use whole-window Discord reaction capture so participants can join or leave mid-recording; individual camera extraction is not required for that first Watch Party version.

## Release decision

Current status: **not approved for release**. Stage 26 implementation checkpoint `3322c46` has no production updater/signing credentials or hosted feed, and no signed clean-PC install/upgrade candidate has been validated. Stages 19–26 also retain the exact human gates listed in `MANUAL_VALIDATION_PENDING.md`.

Release only after all Stage 0–26 requirements, this checklist, clean-machine validation, and outstanding manual gates pass. Record known limitations explicitly; do not label an unvalidated build as SlickClip v1.0.0.
