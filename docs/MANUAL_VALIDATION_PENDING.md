# SlickClip Manual Validation Status

## Passed SlickClip 1.0.3 Replay gates

The validated 1.0.3 checkpoint replaces Replay's custom frame-acquisition loop with one supervised bundled FFmpeg physical-display capture lifecycle. The user reported the exact ten-minute real-game workflow and the recommended 30-minute soak passed on 2026-08-30, including the captured-monitor Replay Saved overlay and optional Save & Name workflow. Preserve the procedures below as the acceptance record. The 1.0.3 installer, upgrade-preservation, and full-product installed-app regression also passed on 2026-08-30. The user authorized a local checkpoint commit on 2026-08-31 but did not authorize a push. Do not alter the historical 1.0.2 release or touch the deferred waveform stash.

### Exact ten-minute persistent-display Replay test

Use the packaged 1.0.3 candidate, a game that can switch between windowed and borderless/fullscreen, a second audible application such as Discord, and a microphone. Set Replay to 60 FPS and expand Capture diagnostics. Keep the Windows pointer moving during game presentation changes.

1. `0:00–1:00` — Begin with automatic game detection and automatic Replay start enabled. Confirm Replay is stopped and logical Replay, owned FFmpeg child, WGC, Rust DXGI, video-worker, and audio-worker counts are zero. Launch the game on a known physical display without choosing a manual source. The game must stabilize for two detector polls and produce exactly one start. Diagnostics must show `Display Capture`, `FFmpeg ddagrab`, `Probing`, then `Healthy` and `Ready`; exactly one owned FFmpeg child must appear, while Replay WGC/Rust-DXGI sessions and frame pools stay zero.
2. `1:00–2:00` — Record the selected `HMONITOR` identity, FFmpeg DXGI adapter/output pair, logical session ID, encoder, retained segment count, and worker counts. Let the game run at high refresh. Segments must rotate about every two seconds at configured 60 FPS without a second child, Replay owner, audio worker set, or duplicate encoder.
3. `2:00–3:00` — Alt-Tab to the desktop, use another application on the same display, and return to the game twice. The adapter/output pair, logical session ID, FFmpeg PID, ring, audio ownership, and encoder must remain unchanged. SlickClip must record the desktop while away and must not follow the foreground window or retarget another monitor.
4. `3:00–4:00` — Switch windowed → borderless/fullscreen → windowed without stopping Replay. There must be no child restart, WGC/custom-DXGI creation, backend transition, compatibility learning, ring reset, audio restart, or cursor suppression. If the owned child genuinely exits, health must become `Recovering`, Save/Ready must pause, only that child may be reaped, and at most three sequential same-display restarts are allowed while session/audio ownership stays unchanged.
5. `4:00–5:00` — Press the global Save Replay hotkey exactly once. Wait for segment/audio coverage, assembly, verification, atomic promotion, and Library indexing. Exactly one non-focus-stealing `Replay Saved` overlay and one new Library clip must appear; the overlay must be in the captured display's work-area corner even when SlickClip is on another display. The key press alone must not emit success, and no duplicate save or overlay may occur.
6. `5:00–6:00` — Play the saved clip across pre-Alt-Tab, desktop, and post-Alt-Tab material. Duration must match the selected replay length within normal one-frame/container tolerance. Video must be moving and monotonic with no burst, timestamp reversal, black/cursor-only output, or corrupt segment boundary. Normal ClipPlayer playback must use Combined/default.
7. `6:00–7:00` — Open the clip in Editor. Confirm separate Game, Voice Chat, and Microphone controls are present when those sources were configured, plus Other when the source master contains it; Combined remains the normal/default playback representation and fallback. Mute/solo/change gain independently and verify structural edits stay synchronized across all tracks. Do not overwrite the source master.
8. `7:00–8:00` — Exit the automatically detected game. Exactly one safe automatic stop must occur. The owned FFmpeg child must finalize and disappear; logical Replay, video-worker, and audio-worker diagnostics must return to zero/idle. Confirm no unrelated FFmpeg process was affected. The saved clip and existing data remain intact.
9. `8:00–9:00` — Turn automatic Replay start off, relaunch the game, and wait for at least three detector polls. Detection may identify the game, but Replay must remain stopped with zero logical Replay, owned FFmpeg child, and audio workers. Re-enable automatic start only after confirming this gate.
10. `9:00–10:00` — With automatic start still off, manually select the game's physical display, configure Game, Voice Chat, and Microphone, start once, wait for `Healthy`/`Ready`, use the separately configured Save & Name hotkey once, and stop once. Confirm one owned FFmpeg child, the configured independent audio workers, one indexed clip, one success overlay on the captured display, and a naming dialog only after indexing succeeds. Enter a name and verify it changes Library metadata without renaming or overwriting the MP4. Confirm clean zero/idle resources after Stop. Quit/relaunch; Library data, preferences, both hotkeys, tray behavior, and existing clips remain unchanged.

Fail the candidate if Replay creates WGC or the retired custom DXGI loop, asks for a separate FFmpeg install, follows foreground changes, changes its adapter/output identity during Alt-Tab, owns two FFmpeg children, starts a second Replay/ring/audio owner, exceeds the three-restart budget, reports Ready during recovery, misses 30/60 FPS mapping, loses monotonic A/V timing, emits duplicate saves/overlays, collapses stems, fails to stop on game exit, auto-starts while disabled, affects unrelated FFmpeg processes, or modifies existing user data.

### Recommended 30-minute soak after the ten-minute gate

Run the packaged build at 1440p60 (or the monitor's native resolution at 60 FPS) with Game, Voice Chat, Microphone, and Other enabled. Play continuously for 30 minutes, Alt-Tab at minutes 5, 12, 19, and 26, and switch windowed/borderless at minutes 8 and 22. Save once at minutes 6, 15, 24, and 29, waiting for each save/index/overlay to finish before the next. During one non-save interval, force only the owned child to exit using its recorded PID to exercise one supervised restart; do not terminate by process name. Verify the logical session ID and audio workers survive, the output pair stays fixed, exactly four clips and four overlays result, every requested duration is within one-frame/container tolerance, all separate stems remain aligned before and after the restart, retention stays bounded to the configured duration plus one in-progress safety segment, memory/handles do not trend upward, and Stop returns every owned process/worker counter to zero without affecting other applications or user data.

## Passed SlickClip 1.0.3 installed-app release gate

Use `SlickClip_1.0.3_x64-setup.exe` and disposable clips for every delete/export action. This gate validates the rest of the product and upgrade preservation; it does not repeat the completed Replay soak.

1. Before installing, record the installed SlickClip version, Library clip count, one existing Collection and its membership, one Favorite, one clip protected from cleanup, both Replay hotkeys, Library sort/grid choice, volume/mute, start-with-Windows, close-to-tray, and automatic Replay setting. Exit SlickClip through its tray menu and confirm no SlickClip process remains.
2. Run the 1.0.3 current-user installer over the existing 1.0.2 installation. An unsigned/unknown-publisher SmartScreen warning is expected. The installer must not request Node.js, Rust, OBS, FFmpeg, or another developer runtime.
3. Launch the installed application from the Start menu. Confirm the splash hands off once, visible version text reads `v1.0.3`, Watch Party remains absent from normal navigation, and no console window appears.
4. Confirm the pre-install Library count and existing clips, Collection membership, Favorite, cleanup protection, hotkeys, sort/grid choice, volume/mute, tray preferences, and automatic Replay setting survived unchanged. The optional Save & Name hotkey must remain enabled or disabled exactly as configured.
5. Exercise Library search and sorting, open an existing clip, seek, play/pause, mute/unmute, change volume, enter/exit fullscreen, Copy Clip, and Open Folder. Normal playback must use Combined/default audio and must not expose independent stems.
6. Create one disposable Replay clip with the normal hotkey and one with Save & Name. Confirm exactly two new Library entries, two captured-monitor overlays, no game-focus theft from normal Save, and metadata-only naming after the second clip is indexed.
7. Add one disposable clip to a Collection, Favorite it, and protect it from automatic cleanup. Restart SlickClip and confirm all three metadata states persist without moving or renaming the MP4.
8. Open the other disposable source in Editor. Make a trim, split, and delete; Undo and Redo; adjust the available Game, Voice Chat, Microphone, and Other mixer tracks; then export. Confirm progress completes, the exported H.264/AAC clip is indexed and playable, and the original source master remains unchanged.
9. In Settings > Storage, confirm the quota, summary, cleanup preview, and confirmation text consistently describe automatic cleanup and exclude the protected disposable clip. Do not run quota cleanup against irreplaceable media.
10. Multi-select the protected disposable clip and the disposable export, choose Delete, and confirm the dialog states that manual deletion overrides cleanup protection. Cancel once and verify both remain. Delete only those disposable entries after confirming their exact selection and verify unrelated clips remain.
11. With Replay stopped, minimize/close according to the configured tray preference, restore from the tray, toggle start-with-Windows on and back to its original value, quit from the tray, relaunch, and confirm there is one main window and the original preference is restored.
12. Confirm no orphan SlickClip-owned FFmpeg process remains after Quit. Record the final Library count and compare it with the expected additions/deletions. If a separate disposable Windows account or clean test PC is available, install 1.0.3 there and repeat launch, one Replay save, playback, and uninstall while confirming user-created clips are not unexpectedly removed.

Fail the release gate for any lost existing data or preference, duplicated Library entry, source-master modification, missing independent Editor stem, incorrect Combined playback, unsafe cleanup/delete target, duplicate window/process, missing sidecar, external dependency request, or installer/version/branding mismatch. The unsigned friends installer does not validate the production updater feed or Authenticode signing; those remain separate release limitations.

## Passed SlickClip-branded installer visual gate

The rebuilt 1.0.3 NSIS package used reproducibly generated 24-bit SlickClip artwork at Tauri's supported 164×314 welcome/finish and 150×57 header sizes, plus explicit SlickClip setup and uninstall icons. The candidate with SHA-256 `3E60D09E72198B8319EB00159B28E46BE6D3D47B4B5624426C986A5DC3CF280D` is withdrawn from testing after the user reported that the Windows pointer disappeared immediately after double-clicking it, before the installer window appeared, installation began, or SlickClip launched; the pointer returned only after restarting the PC. Do not ask the user to run this artifact again. No SlickClip or installer source path calls `ShowCursor`, `SetCursor`, `ClipCursor`, or replaces the system cursor; Replay only reads cursor flags for diagnostics and asks FFmpeg to composite the pointer into captured output. The custom NSIS header/sidebar/icon references and automatic artwork-generation step remain disabled in normal packaging, with a regression test preventing accidental reactivation. The generated artwork source is preserved for investigation and contains the corrected exact slogan `Made to capture the DAWGs worst moments.`

The stock-NSIS fallback containing the user-approved larger typography was built on 2026-08-31 at 88,406,512 bytes, version 1.0.3, unsigned, with SHA-256 `3EBEE545817F381A7846764D035D800299BC3303C720893414A54D54C9C3ADDC`. The user reported that its staged open/cancel cursor test, installation, launch, larger-text visual check, existing clips, and existing settings all passed on the primary Windows PC. Preserve it as the known-safe fallback.

The separately authorized visual-only branded candidate enables only Tauri's supported 150×57 header and 164×314 sidebar bitmap fields. It deliberately leaves setup, application, and uninstaller executable-icon overrides unset. Its artwork contains the exact slogan `Made to capture the DAWGs worst moments.` The resulting unsigned 1.0.3 installer is 88,439,111 bytes with SHA-256 `3F8FD05716E3A53AB3468A104875428BAA27AE60E0360CC70B7DA501FECFEA4F`. On 2026-08-31 the user reported that the branded artwork/slogan, staged open/cancel cursor test, installation, launch, larger typography, existing clips, and existing settings all passed on the primary Windows PC. This new hash is the approved private friends installer. The earlier executable-icon candidate remains permanently withdrawn and must never be distributed or retested.

## Passed manual gates

- The explicitly authorized 1.0.3 consumer UI redesign passed screen-by-screen user review for Home, Clips, Replay, Editor, Settings, Replay Roulette, and Help on 2026-08-31. Editable Home shortcuts, separate Editor stems, Settings quick navigation, Roulette filters/actions, and Help topic navigation were exercised from native release builds.
- SlickClip 1.0.3 installer, 1.0.2-to-1.0.3 upgrade preservation, and full-product installed-app regression.
- SlickClip 1.0.3 persistent-display FFmpeg Replay ten-minute gate and 30-minute soak, including captured-monitor overlay and Save & Name.
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

## Remaining distribution limitation

Authenticode publisher signing and SmartScreen reputation remain unresolved for private friends distribution. Friends must be told to expect an unsigned/unknown-publisher Windows warning. Do not claim the build is Authenticode-signed. The user reported publishing a GitHub Release before the local 1.0.3 checkpoint commit existed; after an explicitly authorized push, verify and align that release tag with the checkpoint so GitHub's source archives match the installer. The unsigned installer must not be represented as a production signed-updater release.

## Deferred manual gates

### Stage 27 — Watch Party / Reaction Capture

Watch Party remains hidden from normal navigation. Its WGC sources and compositor are separate from the simplified Replay pipeline and remain pending real-Windows multi-hour, participant, source-loss, disk-pressure, recovery, audio-sync, playback, and Editor validation.

### Stage 27.1 — Participant-aware reaction layouts

Real Discord layout/theme/camera/share variants and crop/fallback behavior remain manually unverified. This layer stays hidden with Watch Party.

### SlickEdit and waveform

SlickEdit remains deferred. The waveform experiment remains in `stash@{0}` as `Stage 19 waveform experiment - deferred`; do not restore, apply, drop, or recreate it during this release pass.
