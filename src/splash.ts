import { invoke } from "@tauri-apps/api/core";
import "./splash.css";

const status = document.getElementById("startup-status");
const reducedMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;
const minimumDisplayMs = reducedMotion ? 120 : 900;

async function revealApplication() {
  if (status) status.textContent = "Replay workspace ready";
  try {
    await invoke("complete_startup");
  } catch (cause) {
    console.error("SlickClip startup reveal failed:", cause);
    if (status) status.textContent = "Finishing startup…";
  }
}

window.setTimeout(() => void revealApplication(), minimumDisplayMs);
