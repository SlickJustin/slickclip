import { type MouseEvent, useCallback, useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import slickClipIcon from "../assets/branding/slickclip-icon.svg";
import { Icon } from "./Icon";

const isTauriRuntime = "__TAURI_INTERNALS__" in window;
const appWindow = isTauriRuntime ? getCurrentWindow() : null;

export function TitleBar() {
  const [maximized, setMaximized] = useState(false);

  const syncMaximized = useCallback(async () => {
    if (!appWindow) return;
    setMaximized(await appWindow.isMaximized());
  }, []);

  useEffect(() => {
    if (!appWindow) return;
    let disposed = false;
    let unlisten: (() => void) | undefined;
    void syncMaximized();
    void appWindow.onResized(() => void syncMaximized()).then((cleanup) => {
      if (disposed) cleanup();
      else unlisten = cleanup;
    });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, [syncMaximized]);

  const toggleMaximize = useCallback(async () => {
    if (!appWindow) return;
    await appWindow.toggleMaximize();
    await syncMaximized();
  }, [syncMaximized]);

  const handleDragRegionMouseDown = useCallback((event: MouseEvent<HTMLElement>) => {
    if (!appWindow || event.buttons !== 1) return;
    if (event.detail === 2) void toggleMaximize();
    else void appWindow.startDragging();
  }, [toggleMaximize]);

  return (
    <header className="titlebar" onMouseDown={handleDragRegionMouseDown}>
      <div className="titlebar-brand">
        <img src={slickClipIcon} alt="" />
        <strong>SlickClip</strong>
      </div>
      <div className="titlebar-drag-region">
      </div>
      <div className="window-controls" onMouseDown={(event) => event.stopPropagation()}>
        <button className="window-control" type="button" aria-label="Minimize SlickClip" title="Minimize" onClick={() => appWindow && void appWindow.minimize()}>
          <Icon name="minimize" size={16} />
        </button>
        <button className="window-control" type="button" aria-label={maximized ? "Restore SlickClip" : "Maximize SlickClip"} title={maximized ? "Restore" : "Maximize"} onClick={() => void toggleMaximize()}>
          <Icon name={maximized ? "restore" : "maximize"} size={15} />
        </button>
        <button className="window-control window-control-close" type="button" aria-label="Close SlickClip" title="Close" onClick={() => appWindow && void appWindow.close()}>
          <Icon name="close" size={16} />
        </button>
      </div>
    </header>
  );
}
