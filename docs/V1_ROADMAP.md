# SlickClip Roadmap

Complete stages sequentially. Preserve completed behavior, finish each automated and manual gate, and do not begin a later stage unless explicitly instructed.

## Current checkpoint

- Stages 0–18: complete and committed.
- Stage 19: provisional checkpoint `c62c81f`; implementation and automated validation complete, with manual visual/regression validation still pending.
- Stage 20: provisional checkpoint `6b40cc6`; implementation and automated validation complete, with manual UI validation still pending.
- Stage 21: provisional checkpoint `f5c14f2`; implementation and automated validation complete, with manual startup/focus/reduced-motion validation still pending.
- Stage 22: provisional checkpoint `db7328c`; implementation and automated validation complete, with manual Windows tray/startup/focus/background-capture validation still pending.
- Stage 23: provisional checkpoint `4c8e3ec`; implementation and automated validation complete, with manual real-game/launcher/fullscreen/false-positive validation still pending.
- Stage 24: provisional checkpoint `bdfc167`; implementation and automated validation complete, with manual destructive-safety validation still pending.
- Waveform: explicitly deferred; not part of this roadmap.

## Stage 19 — Professional UI/UX Polish

Status: provisional automated checkpoint `c62c81f`; manual gate pending.

Goals:

- Larger readable typography and stronger visual hierarchy.
- Clean spacing and fewer tiny/scattered labels.
- Collapse developer diagnostics without removing useful troubleshooting detail.
- Consistent sidebar, button, card, focus, hover, pressed, and tooltip behavior.
- Improve Replay, Clips, ClipPlayer, Editor, Mixer, Timeline, and Settings presentation.
- Responsive desktop layouts and accessible reduced-motion behavior.
- Preserve all application behavior and architecture.
- No waveform work.

Required gate: manual visual and regression validation across the full application, including reduced motion and responsive layouts. Do not commit the current working-tree implementation until that gate is approved.

## Stage 20 — Replay Roulette

Status: provisional automated checkpoint `6b40cc6`; manual gate pending.

Goals:

- Built-in random clip resurfacing using existing Library metadata.
- Collection and Favorites filters.
- Reduce excessive repeats using play count and recently watched data.
- Open the selected clip in the existing ClipPlayer.
- Polished, restrained roulette interaction.

Required gate: automated coverage for selection logic and manual UI validation.

## Stage 21 — Animated Launch Experience

Status: provisional automated checkpoint `f5c14f2`; manual gate pending.

Goals:

- A real SlickClip splash window with black background, animated purple hex foreground, and centered branding.
- Restrained motion tied to real initialization phases where practical.
- Keep the main window hidden until ready and close the splash cleanly.
- Respect reduced motion.

Required gate: manual visual, focus, startup, and reduced-motion validation.

## Stage 22 — Tray, Background, Startup, and Save Overlay

Status: provisional automated checkpoint `db7328c`; manual gate pending.

Goals:

- System tray status and actions.
- Defined close/minimize-to-background behavior.
- Replay Buffer operation while the main window is hidden.
- Optional start with Windows.
- Non-focus-stealing `Replay Saved` overlay.

Required gate: manual Windows tray, startup, focus, overlay, and background-capture validation.

## Stage 23 — Game Detection and Auto-Arm

Status: provisional automated checkpoint `4c8e3ec`; manual gate pending.

Goals:

- Detect likely games from processes/windows with a launcher-agnostic design where practical.
- Exclusions and manual overrides.
- Optional Replay Buffer auto-arm and ready notification.
- Never silently capture an unintended target.

Required gate: manual testing with representative real games, launchers, fullscreen modes, and false-positive scenarios.

## Stage 24 — Storage Safety

Status: provisional automated checkpoint `bdfc167`; manual gate pending.

Goals:

- Configurable storage quota.
- Pin/Protect clips; Favorite remains a separate concept.
- Safe oldest-unprotected cleanup.
- Strong owned-path and regular-file safeguards.
- Clear preview/dry-run information before destructive cleanup where practical.

Required gate: automated cleanup-order/path-safety coverage and manual destructive-safety validation using disposable data.

## Stage 25 — Final SlickClip Migration and Distribution

Status: provisional implementation checkpoint `cc79345`; automated validation and an unsigned NSIS 1.0.0 bundle pass, while clean-machine migration/install/regression/uninstall validation remains pending in `docs/MANUAL_VALIDATION_PENDING.md`.

Goals:

- Carefully migrate remaining visible/product `JustIn Replay` naming.
- Preserve existing clips, Library database, preferences, previews, and caches.
- Final taskbar, tray, executable, and installer branding.
- Bundle FFmpeg so users do not install it separately.
- Produce a standalone Windows installer for version 1.0.0.

Required gate: migration tests plus clean-machine install, launch, capture, save, playback, edit, export, and uninstall validation.

## Stage 26 — Updater and Release Candidate

Status: provisional implementation checkpoint `3322c46`; signed updater plumbing and the fail-closed release workflow are implemented and automatically clean. A signed release candidate remains blocked on the user-controlled updater key, Windows code-signing identity/command, HTTPS artifact URL/feed, publishing access, and required clean-PC/manual validation. SlickClip v1.0.0 is not release-approved.

Goals:

- Signed updater and release feed.
- Update-and-restart experience.
- Preserve clips and settings across upgrades.
- Clean install, upgrade, rollback/failure handling, and uninstall safety.
- Final regression suite and release candidate.

Required gate: signed clean-PC install/upgrade testing and every requirement in `RELEASE_GATE.md`.

After Stage 26 passes, SlickClip v1.0.0 may be released.

## Stage 27 — Watch Party / Reaction Capture

Status: provisional implementation complete; automated validation passes, while the required real-Windows multi-hour, dynamic-participant, synchronization, source-loss, disk-pressure, recovery, playback, and Editor gate remains pending. The overnight instruction explicitly authorized this post-v1 roadmap work. Stage 27.1 has not been folded into the base implementation.

Purpose: replace the user's OBS-based workflow for recording a long-form event/PPV together with Discord camera reactions.

Core v1 behavior:

- A dedicated continuous long-form recording mode, separate from Replay Buffer.
- Select a Main Content window/source and a Discord Reaction window/source.
- Capture both video sources simultaneously with Windows Graphics Capture.
- GPU-compose the sources into one final canvas without requiring OBS.
- Capture Main Content, Discord/Voice Chat, and Microphone audio.
- Preserve useful sources as separate editable tracks while producing a dependable normal combined playback/output path.
- Provide preset layouts such as reactions on the right, reaction strip, and picture-in-picture.
- Use safe segmented recording for multi-hour events, then finalize on Stop.
- Recover as much valid recorded material as possible after an application or recording failure.
- Where architecture safely permits, allow the Save Replay hotkey to save a reaction moment while Watch Party recording continues.

Dynamic participant requirement:

- Capture the entire Discord video-call/popout window as one live Reaction source.
- Do not require everyone to join before recording starts.
- Keep capturing the same Discord window when participants join or leave mid-recording.
- Let Discord update its own participant grid; SlickClip records those layout changes automatically.
- Do not make v1 depend on identifying individual people or extracting individual camera streams.
- Handle normal Discord window resizing/layout changes without stopping the recording where technically possible; surface a clear recoverable error if the selected window closes or becomes unavailable.

Required engineering work:

- Multiple simultaneous WGC sessions and lifecycle coordination.
- GPU frame composition, aspect-ratio/crop rules, timestamp policy, and bounded resource use.
- Long-duration audio/video synchronization and independent track mapping.
- Segment rotation, atomic metadata/checkpoints, finalization, cancellation, disk-full behavior, and crash recovery.
- Source-loss behavior, privacy-conscious target selection, and clear recording-state UI.
- Independence from Library/Editor work and protection of existing Replay Buffer behavior.

Required gate:

- Automated composition, timestamp, segment/finalization, recovery, and source-state tests.
- Multi-hour real Windows recording soak test.
- A participant joins and another leaves after recording begins; both Discord layout changes appear without restarting SlickClip.
- Main Content, Discord audio, and microphone remain synchronized.
- Window resize, temporary source interruption, disk pressure, Stop/finalize, and crash-recovery scenarios.
- Playback and Editor verification of the resulting file/tracks.

## Stage 27.1 — Advanced Participant-Aware Reaction Layouts

Status: later optional enhancement; not required for Stage 27 v1.

Potential scope:

- Detect/crop individual participant tiles from the captured Discord window.
- Position and resize participant cameras independently.
- Dynamically reflow practical 2 → 3 → 4 participant layouts.
- Saved custom Watch Party layout presets.
- Robust handling for screen shares, hidden cameras, speaking indicators, Discord UI changes, and ambiguous tile boundaries.

Constraint: Discord does not provide SlickClip with clean individual participant streams. This feature would rely on visual tile detection/cropping and is therefore more fragile than whole-window capture. Keep it out of the Stage 27 v1 critical path unless a reliable integration becomes available.
