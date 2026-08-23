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
