import { listen } from "@tauri-apps/api/event";
import "./saveOverlay.css";

type SaveOverlayPayload = { title: string; detail: string };

const title = document.getElementById("overlay-title");
const detail = document.getElementById("overlay-detail");

void listen<SaveOverlayPayload>("replay-saved-overlay", (event) => {
  if (title) title.textContent = event.payload.title;
  if (detail) detail.textContent = event.payload.detail;
  document.body.classList.remove("overlay-arriving");
  window.requestAnimationFrame(() => document.body.classList.add("overlay-arriving"));
});
