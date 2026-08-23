# SlickClip Manual Validation Pending

This is the short human-validation checklist for provisional automated checkpoints. A listed stage is implemented and automatically validated, but is not considered manually verified.

## Stage 19 — Professional UI/UX Polish

- Checkpoint: `c62c81f` (`Stage 19 - professional UI and UX polish [manual validation pending]`)
- Why human validation is required: WebView2 appearance, Windows scaling, real media interaction, fullscreen, focus, responsive layout, and reduced-motion behavior cannot be proven by unit/build checks.
- Shortest practical test: Launch SlickClip; inspect Sidebar, Replay, Clips, ClipPlayer, Editor, and Settings at narrow and maximized widths. Exercise Replay start/stop and Save Replay; Clips search/grid/More menu/Copy Clip; ClipPlayer controls; Editor custom transport, EDL seek after deleting a segment, mixer, fullscreen, and export; Hotkey Test; then repeat interaction checks with Windows animation effects disabled.
- Expected pass behavior: Graphite/purple presentation remains readable and uncluttered; diagnostics are collapsed; card menus remain inside the window and keyboard-operable; Editor seeking stays in edited time; no controls overlap or disappear; capture, save, playback, edit, mixer, export, preferences, and hotkeys behave as before.
- Specific risks to inspect: collection-menu focus and viewport placement, Compact card density, Editor fullscreen/custom seek behavior, seeking across cuts, Windows display scaling, warnings remaining visible, and decorative motion under reduced-motion settings.

## Stage 20 — Replay Roulette

- Checkpoint: `6b40cc6` (`Stage 20 - add Replay Roulette [manual validation pending]`)
- Why human validation is required: the native WebView layout, thumbnail generation, modal ClipPlayer integration, real Library mutations, Windows scaling, and reduced-motion presentation cannot be proven by selection-unit tests or a production build.
- Shortest practical test: Prepare at least seven clips with a mix of favorites, collection membership, play counts, and last-watched times. Open Replay Roulette; verify All Collections and Favorites-only counts and empty states; choose six replays and confirm a pick does not immediately repeat while alternatives exist; open a result, play at least three meaningful seconds, seek, change volume, copy it, and close the player; spin again; then repeat the layout check at narrow and maximized widths with Windows animation effects disabled.
- Expected pass behavior: filters constrain the eligible pool, recent roulette picks stay out of rotation until necessary, less-played/less-recent clips are favored over time, every chosen thumbnail opens the existing ClipPlayer, watch metadata still updates, Copy Clip shows feedback, empty/error/loading states remain clear, and the restrained reveal disappears under reduced motion.
- Specific risks to inspect: probabilistic weighting over repeated runs, one-clip and fully filtered pools, deleted collections, asynchronous thumbnail readiness, player focus/escape handling, long clip names, narrow widths, high display scaling, and motion settings.

## Stage 21 — Animated Launch Experience

- Checkpoint: `f5c14f2` (`Stage 21 - add animated launch experience [manual validation pending]`)
- Why human validation is required: only a real packaged/native launch can verify whether the splash paints before initialization work, stays centered, avoids taskbar/focus artifacts, hands focus to the main WebView cleanly, and respects the user’s Windows motion preference.
- Shortest practical test: Cold-launch SlickClip normally and with Windows animation effects disabled. In each mode confirm the black borderless splash appears centered with SlickClip branding, the main window is not visible behind it, and exactly one focused main window replaces it without a white flash, stranded splash, taskbar duplicate, or input delay. Repeat once after an unclean process termination and once on a high-DPI display setting.
- Expected pass behavior: normal mode shows restrained purple hex/progress motion during real initialization; reduced-motion mode is static and hands off faster; startup state initializes once; the splash closes; the main window becomes visible and focused at its normal size; and the eight-second safety reveal is never noticeable during a healthy launch.
- Specific risks to inspect: first-frame white flash, splash loading too late to be useful, high-DPI centering, focus loss, taskbar duplication, WebView asset-load failure, slow initialization beyond eight seconds, reduced-motion detection, and closing the app during splash display.

## Stage 22 — Tray, Background, Startup, and Save Overlay

- Checkpoint: `db7328c` (`Stage 22 - add tray and background lifecycle [manual validation pending]`)
- Why human validation is required: Windows tray menus, HKCU startup registration, native close/minimize events, background capture continuity, multi-monitor placement, focus behavior, and transparent always-on-top overlays require a real interactive Windows session.
- Shortest practical test: Launch SlickClip and confirm one tray icon reports Replay Buffer stopped. Start a replay buffer, wait for a finalized segment, and confirm the tray reports the selected target and enables Save Replay. With “Close or minimize to tray” on, minimize and then close the main window; verify SlickClip leaves the taskbar, capture counters continue advancing, tray Save Replay and the global hotkey each produce a valid Library clip, and the lower-right Replay Saved overlay never steals focus from another app. Open SlickClip from the tray, test the overlay-off preference, enable Windows startup, sign out/in (or run the registered command), confirm background launch creates only the tray presence, then disable startup and verify the registration is removed. Finally turn close-to-tray off, close the window, and confirm the process exits; repeat Quit from the tray while capture is active.
- Expected pass behavior: tray status/save availability follows the real replay/save managers; hide/restore preserves the active buffer and UI state; only explicit Quit or closing with background mode disabled exits and cleans capture; startup opens quietly with `--background`; preferences survive restart; successful saves show one approximately 3.2-second noninteractive overlay on the correct monitor without changing foreground focus; disabled overlays stay hidden.
- Specific risks to inspect: duplicate or missing tray icons, stale status, Save Replay enable timing, minimize/restore loops, unsaved Editor state while hiding, fullscreen capture while hidden, registry paths containing spaces, startup after app relocation/update, taskbar flashes, multi-monitor work areas and scaling, overlay focus theft, repeated saves extending overlay duration, shutdown during an active save, and preference-write/registry rollback errors.
