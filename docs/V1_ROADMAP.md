# SlickClip Roadmap

Complete stages sequentially. Preserve completed behavior, finish each automated and manual gate, and do not begin a later stage unless explicitly instructed.

## Current checkpoint

- SlickClip 1.0.3 game-detection UX and the persistent-display Replay simplification are combined in the current validated checkpoint. Automated validation, the real-game ten-minute gate, 30-minute soak, 1.0.3 installer, 1.0.2-to-1.0.3 upgrade preservation, and full-product installed-app regression were reported passed on 2026-08-30. The later explicitly authorized Home, Clips, Replay, Editor, Settings, Replay Roulette, and Help redesign passed user review and the final automated/source-audit gate on 2026-08-31. The visual-only branded installer also passed its cursor/install/data gate. A local checkpoint commit was authorized; push and GitHub tag alignment remain pending explicit authorization.

- Stages 0–18: complete and committed.
- Stages 19–23: implementation, automated validation, and manual product gates passed.
- Stage 24: implementation, automated validation, and disposable-data destructive cleanup gate passed.
- Stage 25: packaged executable, migration, installer, and end-to-end manual validation passed for private friends distribution.
- Stage 26: updater A-to-B validation passed with preserved user data and working Update & Restart behavior. Authenticode/SmartScreen remains unresolved for private friends distribution.
- Clips multi-select and batch actions: implemented at `9371391` and manually passed. Manual deletion intentionally overrides `Protect from Cleanup` after explicit confirmation.
- Friends release: SlickClip 1.0.2 is the last historical release; SlickClip 1.0.3 is the current validated friends build. Watch Party is hidden from normal navigation until its real-Windows gate passes.
- SlickEdit: deferred/on the back burner and outside SlickClip 1.0.3.
- Waveform: explicitly deferred; not part of this roadmap.

## Stage 19 — Professional UI/UX Polish

Status: checkpoint `c62c81f`; automated and manual gates passed.

Goals:

- Larger readable typography and stronger visual hierarchy.
- Clean spacing and fewer tiny/scattered labels.
- Collapse developer diagnostics without removing useful troubleshooting detail.
- Consistent sidebar, button, card, focus, hover, pressed, and tooltip behavior.
- Improve Replay, Clips, ClipPlayer, Editor, Mixer, Timeline, and Settings presentation.
- Responsive desktop layouts and accessible reduced-motion behavior.
- Preserve all application behavior and architecture.
- No waveform work.

Gate result: passed manual visual and regression validation across the full application, including reduced motion and responsive layouts.

## Stage 20 — Replay Roulette

Status: checkpoint `6b40cc6`; automated and manual gates passed.

Goals:

- Built-in random clip resurfacing using existing Library metadata.
- Collection and Favorites filters.
- Reduce excessive repeats using play count and recently watched data.
- Open the selected clip in the existing ClipPlayer.
- Polished, restrained roulette interaction.

Gate result: automated selection coverage and manual UI validation passed.

## Stage 21 — Animated Launch Experience

Status: checkpoint `f5c14f2`; automated and manual gates passed.

Goals:

- A real SlickClip splash window with black background, animated purple hex foreground, and centered branding.
- Restrained motion tied to real initialization phases where practical.
- Keep the main window hidden until ready and close the splash cleanly.
- Respect reduced motion.

Gate result: manual visual, focus, startup, and reduced-motion validation passed.

## Stage 22 — Tray, Background, Startup, and Save Overlay

Status: checkpoint `db7328c`; automated and manual gates passed.

Goals:

- System tray status and actions.
- Defined close/minimize-to-background behavior.
- Replay Buffer operation while the main window is hidden.
- Optional start with Windows.
- Non-focus-stealing `Replay Saved` overlay.

Gate result: manual Windows tray, startup, focus, overlay, and background-capture validation passed.

## Stage 23 — Game Detection and Auto-Arm

Status: checkpoint `4c8e3ec`; automated and manual gates passed.

Goals:

- Detect likely games from processes/windows with a launcher-agnostic design where practical.
- Exclusions and manual overrides.
- Optional Replay Buffer auto-arm and ready notification.
- Never silently capture an unintended target.

Gate result: manual testing with representative real games, launchers, fullscreen modes, and false-positive scenarios passed.

1.0.3 FFmpeg Replay backend (validated friends checkpoint): automatic game detection and manual fallback both resolve one physical display, then launch one hidden bundled FFmpeg `ddagrab` child. FFmpeg owns frame acquisition, D3D11 handling, CFR encoding, timestamps, and two-second rolling MP4 segments. Alt-Tab and presentation changes never retarget or restart Replay. Unexpected child exit has a bounded three-restart same-display policy while the logical session, retained segments, shared monotonic QPC clock, and independent native audio tracks survive. There is no OBS dependency and no custom Replay DXGI fallback. Legacy mode values remain wire-compatible only. This does not reopen Stage 23 detection rules, Watch Party, Editor, or Library.

## Stage 24 — Storage Safety

Status: checkpoint `bdfc167`; automated and disposable-data manual gates passed.

Goals:

- Configurable storage quota.
- Pin/Protect clips; Favorite remains a separate concept.
- Safe oldest-unprotected cleanup.
- Strong owned-path and regular-file safeguards.
- Clear preview/dry-run information before destructive cleanup where practical.

Gate result: automated cleanup-order/path-safety coverage and manual destructive-safety validation using disposable data passed. `Protect from Cleanup` affects automatic cleanup only; explicit manual deletion remains available.

## Stage 25 — Final SlickClip Migration and Distribution

Status: checkpoint `cc79345`; automated, migration, packaged executable, installer, clean-machine regression, and uninstall-preservation validation passed for private friends distribution. Version 1.0.2 supersedes the historical 1.0.0 candidate.

Goals:

- Carefully migrate remaining visible/product `JustIn Replay` naming.
- Preserve existing clips, Library database, preferences, previews, and caches.
- Final taskbar, tray, executable, and installer branding.
- Bundle FFmpeg so users do not install it separately.
- Produce a standalone Windows installer for the current product version.

Gate result: migration plus clean-machine install, launch, capture, save, playback, edit, export, and uninstall validation passed.

## Stage 26 — Updater and Release Candidate

Status: checkpoint `3322c46`; updater A-to-B behavior, install/restart, and user-data preservation passed manual validation. The 1.0.1 updater candidate is historical and must not be reused. Authenticode publisher signing and SmartScreen reputation remain unresolved, and no GitHub Release is approved by this friends-build pass.

Goals:

- Signed updater and release feed.
- Update-and-restart experience.
- Preserve clips and settings across upgrades.
- Clean install, upgrade, rollback/failure handling, and uninstall safety.
- Final regression suite and release candidate.

Gate result: functional clean-PC A-to-B upgrade validation passed. Public release still requires the Authenticode/publisher conditions in `RELEASE_GATE.md`.

SlickClip 1.0.2 may be distributed privately to friends with the unsigned-publisher limitation disclosed. Public release remains separately gated.

## Stage 27 — Watch Party / Reaction Capture

Status: deferred and hidden from normal SlickClip 1.0.2 friends-build navigation. Checkpoint `cf82ba8` passes automated validation, but the required real-Windows multi-hour, dynamic-participant, synchronization, source-loss, disk-pressure, recovery, playback, and Editor gate remains pending. The implementation and backend architecture remain intact.

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

Status: deferred and hidden with Watch Party for SlickClip 1.0.2. Optional checkpoint `f38be14` passes automated detector/tracker/reflow and fallback tests, while real Discord UI variants and participant behavior remain manually unverified.

Potential scope:

- Detect/crop individual participant tiles from the captured Discord window.
- Position and resize participant cameras independently.
- Dynamically reflow practical 2 → 3 → 4 participant layouts.
- Saved custom Watch Party layout presets.
- Robust handling for screen shares, hidden cameras, speaking indicators, Discord UI changes, and ambiguous tile boundaries.

Constraint: Discord does not provide SlickClip with clean individual participant streams. This feature would rely on visual tile detection/cropping and is therefore more fragile than whole-window capture. Keep it out of the Stage 27 v1 critical path unless a reliable integration becomes available.
