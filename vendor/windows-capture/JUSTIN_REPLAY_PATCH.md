# JustIn Replay local patch

This directory is a repository-owned fork of `windows-capture` 2.0.1. The upstream package name,
version, source layout, README, and MIT `LICENCE` are preserved. Cargo selects it through the path
dependency in `src-tauri/Cargo.toml`; files in Cargo's registry cache are not modified.

The Stage 7.4 changes are deliberately limited to capture-pool construction and encoder input:

- `Direct3D11CaptureFramePool::CreateFreeThreaded` replaces `Create`. The buffer count is carried by
  `Settings`, defaults to 2, and is clamped to the intended test range 1–3.
- `VideoEncoder::send_frame` no longer clones and queues `Frame::as_raw_surface()`. It creates a
  default-usage, CPU-inaccessible D3D11 texture on the frame's existing device, submits
  `CopyResource` for equal dimensions (or a cleared `CopySubresourceRegion` for padding), creates a
  WinRT surface projection, and queues only that owned texture/surface pair.
- The GPU copy is submitted before `send_frame` returns. D3D11 command ordering keeps the source
  resource valid for the asynchronous copy; the Rust borrow ends without any CPU map/readback.
- The video channel is a bounded `sync_channel` (capacity configured by `VideoSettingsBuilder`, 8
  in JustIn Replay). Submission uses `try_send`; a full queue drops the newest frame and records
  telemetry rather than blocking the WGC callback.
- The queue owns both `ID3D11Texture2D` and `IDirect3DSurface` until `MediaStreamSample` takes its
  surface reference. This prevents texture reuse or destruction while MediaStreamSource may still
  consume it.

No Media Foundation MFT, audio feature, CPU readback, or unrelated upstream code is introduced.
