import { useEffect, useState } from "react";
import { convertFileSrc, invoke } from "@tauri-apps/api/core";
import type { ClipListItem, PrepareClipMediaResponse } from "../types/clips";

type Props = {
  clip: ClipListItem;
  onPlay: () => void;
};

export function ClipThumbnail({ clip, onPlay }: Props) {
  const [source, setSource] = useState<string | null>(null);
  const [failed, setFailed] = useState(false);

  useEffect(() => {
    let disposed = false;
    let timer: number | undefined;

    async function request() {
      try {
        const response = await invoke<PrepareClipMediaResponse>("request_clip_thumbnail", {
          request: {
            clipId: clip.id,
            retry: false,
            currentTimeSeconds: 0,
            wasPlaying: false,
          },
        });
        if (disposed) return;
        if (response.artifact.state === "ready" && response.artifact.filePath) {
          setSource(`${convertFileSrc(response.artifact.filePath)}?v=${clip.fileModifiedAtMs}-${clip.fileSizeBytes}`);
          setFailed(false);
        } else if (response.artifact.state === "preparing") {
          timer = window.setTimeout(request, 900);
        } else if (response.artifact.state === "error") {
          setFailed(true);
        }
      } catch {
        if (!disposed) setFailed(true);
      }
    }

    setSource(null);
    setFailed(false);
    void request();
    return () => {
      disposed = true;
      if (timer !== undefined) window.clearTimeout(timer);
    };
  }, [clip.fileModifiedAtMs, clip.fileSizeBytes, clip.id]);

  return (
    <button className="clip-card-visual" type="button" onClick={onPlay} aria-label={`Play ${clip.displayName}`}>
      {source && !failed ? (
        <img src={source} alt="" onError={() => setFailed(true)} />
      ) : (
        <span className="clip-codec-placeholder">
          <strong>{clip.videoCodec.toUpperCase()}</strong>
          <small>{clip.width}×{clip.height}</small>
        </span>
      )}
      <span className="clip-play-overlay" aria-hidden="true">▶</span>
    </button>
  );
}
