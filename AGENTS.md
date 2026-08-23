# SlickClip Agent Instructions

SlickClip is an existing Windows desktop game capture and replay application. The goal is to finish the product while preserving its working capture, audio, Library, Editor, export, and data architecture.

Do not start over. Do not casually redesign or replace working subsystems. Do not overwrite unrelated user work.

## Source of truth

Before significant work, read:

- `docs/SLICKCLIP_MASTER_STATE.md`
- `docs/V1_ROADMAP.md`
- `docs/RELEASE_GATE.md`

These documents define the verified project state, remaining stages, release requirements, and intentionally deferred features. If documentation and code disagree, inspect the code and report the discrepancy before making a destructive or migration-sensitive change.

## Stack and product constraints

Primary stack:

- Windows 11
- Tauri 2
- React and TypeScript
- Rust
- Windows Graphics Capture
- native WASAPI audio
- FFmpeg
- SQLite

SlickClip is a standalone Windows application. End users must not need Node.js, Rust, OBS, a separate FFmpeg installation, or developer tools.

## Engineering rules

- Preserve completed-stage behavior unless a later approved stage requires a change.
- Prefer existing utilities, models, repository layers, media queues, cache systems, path-safety helpers, database abstractions, and Windows integrations.
- Do not add unrelated features while implementing a stage.
- Do not begin the next roadmap stage unless explicitly instructed.
- Keep realtime capture independent from Library, Editor, cache, export, clipboard, and preference work.
- Never hold realtime capture locks while performing SQLite operations, preference writes, clipboard operations, thumbnail or media-cache generation, Editor work, or FFmpeg export.

## User data safety

Never casually delete, overwrite, or relocate source clips, exports, databases, preferences, collections, favorites, watch metadata, required caches, or migrations.

The Editor is nondestructive. Source masters remain immutable, structural edits are represented by an Edit Decision List, authoritative edit timing uses integer microseconds, and exports create new files.

Collections are metadata-only. Do not move an MP4 because it belongs to a Collection.

Frontend commands should prefer trusted Clip IDs, Collection IDs, stream indexes, and typed data instead of arbitrary paths. Backend code must validate owned paths and file types. Do not introduce shell-based path handling when direct APIs are available.

## Protected architecture

The existing capture system includes Windows Graphics Capture, realtime CFR scheduling, rolling video segments, synchronized rolling audio, a global Save Replay hotkey, hardware encoder handling, and replay assembly. Do not replace it without explicit approval.

Source masters may contain Combined, Game, Voice Chat, Microphone, and Other audio. Normal ClipPlayer playback uses Combined/default audio. The Editor uses independent stems where available and applies structural cuts across all tracks. Do not reintroduce stem selection into normal ClipPlayer playback unless explicitly requested.

The existing Editor mixer supports Game, Voice Chat, Microphone, Other, and Combined fallback, with 0–300% gain, Mute, Solo, and synchronized preview. Existing export consumes EDL and mixer decisions, creates a flattened H.264/AAC MP4, preserves the master, supports progress and cancellation, falls back from hardware to software encoding, and indexes successful exports into the Library.

## Historical naming and migration

Some internal paths and identifiers still use `JustIn Replay`. Do not casually rename the Tauri identifier, LocalAppData root, Library database location, Clips root, or preview/cache roots. Final migration is a dedicated roadmap stage; visible branding may already say SlickClip.

## Current working-tree protection

At the time these instructions were restored, Stage 19 UI polish was intentionally present as an uncommitted six-file working-tree diff pending manual visual validation. Preserve it. The deferred waveform experiment is stored separately as `Stage 19 waveform experiment - deferred`; do not restore, pop, apply, drop, or recreate it unless explicitly instructed.

Always inspect current Git state rather than assuming this paragraph is still current.

## Git workflow

Before beginning a stage:

1. Run `git status` and identify the branch and current commit.
2. Determine whether the tree is clean or intentionally contains current-stage work.
3. Read the relevant roadmap stage and master state.
4. Inspect existing code before changing architecture.

Never use destructive Git commands such as `git reset --hard`, `git clean -fd`, or a forced checkout of user work without explicit approval.

If a stage still requires manual Windows or visual validation, do not commit it automatically. Report that implementation is ready for the manual gate and stop. After the user confirms the gate passes, a stage checkpoint may be committed and pushed when explicitly requested.

## Required automated validation

After meaningful code changes, run:

```text
npm test
npm run build
cargo check
cargo test -- --nocapture
cargo fmt -- --check
git diff --check
```

If no lint script exists, report that rather than inventing one. Fix failures caused by current changes before reporting readiness.

## Manual validation gates

Automated tests do not prove visual appearance, WebView2 rendering, actual audio, taskbar or tray behavior, focus and overlay behavior, Replay Buffer behavior under real load, Save Replay during other media work, real game detection, Windows startup, installer/updater behavior, or clean-PC compatibility. Request exact manual validation for stages involving these areas.

## UI principles

SlickClip should feel like polished consumer software: graphite or near-black, royal purple, clean, restrained, professional, with subtle sci-fi or military-tech influence.

Avoid excessive or looping motion, giant glow effects, tiny text, clutter, and prominent developer telemetry. Prefer subtle interaction feedback, respect `prefers-reduced-motion`, and keep engineering diagnostics collapsed or development-only unless an active error requires them.

## Deferred waveform

The waveform experiment is intentionally deferred from v1.0. Do not restore or recreate waveform functionality unless explicitly instructed.

## Stage completion report

At the end of each stage, report:

1. Files created and modified.
2. Dependency changes.
3. Architecture implemented and important safety decisions.
4. Migrations, if any.
5. Automated verification results.
6. Manual validation still required, with an exact procedure.
7. Git status.
8. Confirmation that the next stage was not started.

The public release target is SlickClip v1.0.0. It must satisfy `docs/RELEASE_GATE.md` and should be treated as a finished release, not a rough beta.
