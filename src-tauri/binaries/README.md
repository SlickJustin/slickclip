# SlickClip FFmpeg sidecars

Run `npm run prepare:ffmpeg` from the repository root to stage the pinned, checksum-verified Windows x64 `ffmpeg` and `ffprobe` executables used by release bundles.

The generated executables and license/source files are intentionally ignored by Git. The canonical `npm run bundle` command stages them before invoking the Tauri NSIS bundler. Do not replace them with an unverified PATH installation.
