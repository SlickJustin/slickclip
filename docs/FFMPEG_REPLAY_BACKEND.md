# SlickClip FFmpeg Replay Backend

Recorded: 2026-08-29. This document describes the uncommitted 1.0.3 working tree.

## Bundled binary probe

The existing sidecar was not replaced.

- File: `src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe`
- Size: 145,240,576 bytes
- SHA-256: `DCEC5129F94A0E7338303A9BDB6548889D28238F57E1A2315884946C47FA1C40`
- Version: `N-125875-g5d4d3bdc61-20260731`
- Upstream commit: `5d4d3bdc61`
- Distributor/build source: BtbN FFmpeg Builds, pinned tag `autobuild-2026-07-31-14-10`
- License: GPLv3 build; the existing bundled `FFmpeg-LICENSE.txt` and `FFmpeg-SOURCE.txt` remain the attribution/source offer.

Exact metadata command:

```powershell
src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe -hide_banner -version
```

The build reports GCC 15.2.0 and includes `--enable-gpl --enable-version3 --enable-amf --enable-ffnvcodec --enable-cuda-llvm --enable-libvpl --enable-libx264 --enable-libx265`.

Exact compiled-capability commands:

```powershell
src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe -hide_banner -filters
src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe -hide_banner -h filter=ddagrab
src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe -hide_banner -encoders
```

Results:

- `ddagrab` is compiled and exposes `output_idx`, `draw_mouse`, `framerate`, `video_size`, offsets, output formats, and `dup_frames`.
- H.264 encoders compiled: `h264_nvenc`, `h264_amf`, `h264_qsv`, and `libx264`.
- HEVC encoders compiled: `hevc_nvenc`, `hevc_amf`, `hevc_qsv`, and `libx265`.

Exact real encoder probes used a two-frame 1280x720 color source and `-f null NUL`. On this machine, H.264 NVENC, AMF, and x264 exited 0. QSV failed to create an MFX session and was correctly rejected.

Exact real Desktop Duplication probe:

```powershell
src-tauri\binaries\ffmpeg-x86_64-pc-windows-msvc.exe -hide_banner -loglevel warning -init_hw_device d3d11va=dda:0 -filter_hw_device dda -filter_complex "ddagrab=output_idx=0:draw_mouse=1:framerate=60:dup_frames=1,hwdownload,format=bgra" -frames:v 1 -f null NUL
```

Result in the Codex automation environment: exit `-1`, `Desktop duplication access denied`, `Operation not permitted`. This proves the binary contains `ddagrab`, but this automation session cannot complete a real screen-capture permission probe. SlickClip now performs the same adapter/output-specific real probe before starting native audio. It reports the gap and does not fall back to the retired custom Replay DXGI loop. The packaged manual gate must prove the normal installed desktop process is granted access.

## Runtime design

Game detection or manual selection resolves a Windows physical monitor. SlickClip maps its `HMONITOR` to the exact DXGI adapter index and adapter-local output index. It then verifies bundled `ddagrab`, probes the chosen display, probes encoders in hardware-first order, and launches one hidden owned FFmpeg process.

FFmpeg owns Desktop Duplication, D3D11 frames, CFR delivery, encoding, timestamps, keyframes, and two-second MP4 segment muxing. Rust owns the logical Replay session, process supervision, segment discovery/validation/retention, save pinning, native WASAPI audio, clip assembly, Library indexing, and UI status.

The session holds at most one capture child. An unexpected exit closes and inspects usable tail material, keeps the logical Replay clock and native audio workers alive, and performs at most three sequential same-display restarts. It never searches for or terminates unrelated `ffmpeg.exe` processes. Stop writes `q` to the owned child's stdin, waits up to three seconds for clean finalization, and only then terminates that owned child if required.

## Representative 2560x1440 60 FPS command shape

```text
ffmpeg.exe
  -hide_banner -loglevel warning -y
  -init_hw_device d3d11va=dda:<DXGI_ADAPTER_INDEX>
  -filter_hw_device dda
  -filter_complex "ddagrab=output_idx=<ADAPTER_LOCAL_OUTPUT_INDEX>:draw_mouse=1:framerate=60:dup_frames=1[capture]"
  -map "[capture]" -an
  -c:v h264_nvenc -preset p4 -tune ll -rc vbr -cq 23 -b:v 0
  -r 60 -fps_mode cfr -g 120 -keyint_min 120
  -force_key_frames "expr:gte(t,n_forced*2)"
  -f segment -segment_time 2 -segment_time_delta 0.008333333
  -reset_timestamps 1 -segment_start_number <NEXT_SEQUENCE>
  -segment_format mp4 -segment_format_options movflags=+faststart
  "<SLICKCLIP_REPLAY_SESSION>\segment-%06d.mp4"
```

The monitor's native 2560x1440 size flows from `ddagrab`; no scale filter is inserted. Software fallback explicitly inserts `hwdownload,format=bgra,format=yuv420p`. High, Balanced, and Smaller Files map to quantizer values 18, 23, and 28 across NVENC/AMF/QSV/software command variants. Automatic preserves SlickClip's codec preference while retaining a compatibility fallback: HEVC NVENC → AMF → QSV, H.264 NVENC → AMF → QSV, then x264. Explicit H.264 and HEVC use their respective hardware-first lists, with x264/x265 as the final fallback. Every candidate must pass a runtime probe on the selected display.

## Audio and session timeline

Game, Voice Chat/Discord, Microphone, and Other remain independent native WASAPI rings. Combined/default is still produced during successful clip assembly and remains normal playback. FFmpeg receives no audio.

Every video segment and audio packet uses the existing `ReplaySessionClock` QPC calibration. FFmpeg segment PTS is normalized into that monotonic session domain. If the owned child restarts, the video segments retain the real QPC gap; save-time piecewise mapping removes that same gap from every audio stem before muxing. This keeps post-restart audio aligned with the stream-copy-concatenated video without collapsing stems or restarting audio ownership.
