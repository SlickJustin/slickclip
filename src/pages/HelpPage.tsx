import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { buildHelpSections } from "../content/helpContent";
import type { UiPreferencesResponse } from "../types/clips";
import { defaultUiPreferences } from "../types/clips";

export function HelpPage() {
  const [saveReplayHotkey, setSaveReplayHotkey] = useState(defaultUiPreferences.saveReplayHotkey);

  useEffect(() => {
    void invoke<UiPreferencesResponse>("get_ui_preferences")
      .then((response) => {
        if (response.success) setSaveReplayHotkey(response.preferences.saveReplayHotkey);
      })
      .catch(() => undefined);
  }, []);

  const sections = buildHelpSections(saveReplayHotkey);

  return (
    <div className="page page-help">
      <header className="page-header help-page-header">
        <div>
          <span className="help-page-eyebrow">Learn SlickClip</span>
          <h1>Help Center</h1>
          <p>Short answers for capturing, finding, and sharing a moment.</p>
        </div>
        <div className="help-header-status"><span>Beginner friendly</span><span>SlickClip 1.0.3</span></div>
      </header>

      <section className="help-intro panel" aria-labelledby="help-intro-heading">
        <div className="help-intro-copy"><span className="eyebrow">Replay in one line</span><h2 id="help-intro-heading">Play first. Save the moment after.</h2><p>SlickClip keeps a temporary rolling window. Press your shortcut only when something worth keeping happens.</p></div>
        <div className="help-replay-flow" aria-label="Basic Replay workflow">
          <span><b>01</b><strong>Open game</strong></span><i>→</i>
          <span><b>02</b><strong>Replay ready</strong></span><i>→</i>
          <span><b>03</b><strong>{saveReplayHotkey}</strong></span><i>→</i>
          <span><b>04</b><strong>Clip saved</strong></span>
        </div>
      </section>

      <div className="help-workbench">
        <aside className="help-index" aria-label="Help topics">
          <div><span>Quick answers</span><strong>What do you need?</strong><small>Choose a topic to open its guide.</small></div>
          <nav>{sections.map((section, index) => <button type="button" key={section.id} onClick={() => openHelpSection(section.id)}><span>{String(index + 1).padStart(2, "0")}</span><strong>{section.title}</strong><b aria-hidden="true">→</b></button>)}</nav>
          <section className="help-hotkey-card"><span>Your Save Replay hotkey</span><kbd>{saveReplayHotkey}</kbd><small>Use it while your game is focused.</small></section>
        </aside>

        <main className="help-sections">
          {sections.map((section, sectionIndex) => (
            <details className={`panel help-section${section.steps ? " help-section-primary" : ""}`} id={section.id} key={section.id} open={Boolean(section.steps) || undefined}>
              <summary><div><span>{String(sectionIndex + 1).padStart(2, "0")}</span><h2>{section.title}</h2></div><i aria-hidden="true">⌄</i></summary>
              <div className="help-section-body">
                {section.paragraphs?.map((paragraph) => <p key={paragraph}>{paragraph}</p>)}
                {section.steps && (
                  <ol className="help-steps">
                    {section.steps.map((step, index) => <li key={step}><span>Step {index + 1}</span><p>{step}</p></li>)}
                  </ol>
                )}
                {section.bullets && <ul>{section.bullets.map((bullet) => <li key={bullet}>{bullet}</li>)}</ul>}
              </div>
            </details>
          ))}
        </main>
      </div>
    </div>
  );
}

function openHelpSection(targetId: string) {
  const section = document.getElementById(targetId);
  if (!(section instanceof HTMLDetailsElement)) return;
  section.open = true;
  section.scrollIntoView({ behavior: "smooth", block: "start" });
  window.setTimeout(() => section.querySelector<HTMLElement>("summary")?.focus({ preventScroll: true }), 250);
}
