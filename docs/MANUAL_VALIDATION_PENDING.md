# SlickClip 1.0.2 Manual Validation Status

This file tracks only manual work that remains after the verified friends-release passes. Historical checkpoint procedures are preserved in Git history and summarized in `SLICKCLIP_MASTER_STATE.md`.

## Passed manual gates

- Stage 19 premium UI and regression behavior.
- Stage 20 Replay Roulette.
- Stage 21 splash and startup handoff.
- Stage 22 tray, background behavior, Replay Saved overlay, and startup behavior.
- Arbitrary hotkeys and the redesigned Settings experience.
- Stage 23 game detection and auto-arm.
- Stage 24 disposable-data destructive cleanup, cleanup ordering, and `Protect from Cleanup` behavior.
- Stage 25 packaged release executable, migration, installer, clean-machine media workflow, and uninstall preservation.
- Stage 26 sequential updater A-to-B behavior, including Update & Restart and user-data preservation. Version 1.0.1 was a historical test candidate and must not be reused.
- Clips multi-select and batch actions, including the intentional ability to manually delete clips protected from automatic cleanup.

## Final 1.0.2 friends-build smoke test

Run this 5–10 minute check against the exact packaged build intended for friends, using disposable clips for deletion:

1. Cold-launch SlickClip. Confirm the splash hands off to one focused main window, the sidebar footer reads `v1.0.2`, and Watch Party is absent from normal navigation.
2. Start the Replay Buffer against a safe window, wait for it to reach Running, save once from the UI or configured global hotkey, and confirm the non-focus-stealing Replay Saved overlay and new Library clip.
3. In Clips, mark one disposable clip `Protect from Cleanup`. Confirm its badge and menu use `Protected from Cleanup`/`Remove Cleanup Protection` wording.
4. Select that protected disposable clip plus at least one unprotected disposable clip and choose Delete selected. Confirm the dialog reports the exact protected count and states that manual deletion overrides automatic-cleanup protection. Cancel once and verify both files remain; repeat, confirm, and verify only the selected files are permanently removed.
5. Open Settings > Storage. Confirm quota, preview, summary, and cleanup confirmation copy consistently describes automatic cleanup and clips protected from cleanup. Do not run destructive cleanup against irreplaceable media.
6. Play a remaining clip, open it in Editor, make a small nondestructive edit, export, and play the exported H.264/AAC clip. Confirm the source master remains unchanged.
7. Hide/restore SlickClip through the tray while the Replay Buffer is running, then stop it and Quit cleanly. If the build has a configured update feed, confirm Check for Updates never offers the historical 1.0.1 candidate to 1.0.2.

## Remaining distribution limitation

Authenticode publisher signing and SmartScreen reputation remain unresolved for private friends distribution. Friends must be told to expect an unsigned/unknown-publisher Windows warning. Do not claim the build is Authenticode-signed, and do not publish a GitHub Release from this checkpoint.

## Deferred manual gates

### Stage 27 — Watch Party / Reaction Capture

Watch Party is hidden from normal SlickClip 1.0.2 friends-build navigation behind `featureVisibility.watchParty`. The implementation and backend architecture remain present, but real Windows multi-hour recording, dynamic Discord participants, source loss, disk pressure, crash recovery, audio synchronization, playback, and Editor validation remain pending. Re-enable the visibility flag only when resuming that dedicated validation.

### Stage 27.1 — Participant-aware reaction layouts

Real Discord layout/theme/camera/share variants and crop/fallback behavior remain manually unverified. This layer stays hidden with Watch Party.

### SlickEdit and waveform

SlickEdit remains deferred/on the back burner and is not part of SlickClip 1.0.2. The waveform experiment remains deferred in the stash named `Stage 19 waveform experiment - deferred`; do not restore, apply, drop, or recreate it during this release pass.
