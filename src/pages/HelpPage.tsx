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
      <header className="page-header">
        <div>
          <h1>Help & Getting Started</h1>
          <p>The essentials for capturing a moment and finding it again.</p>
        </div>
        <span className="demo-badge">SlickClip 1.0.2</span>
      </header>

      <section className="help-intro panel" aria-labelledby="help-intro-heading">
        <span className="eyebrow">The five-step replay</span>
        <h2 id="help-intro-heading">Capture the moment after it happens</h2>
        <p>Set up once, play normally, then use your shortcut when something worth keeping happens.</p>
      </section>

      <div className="help-sections">
        {sections.map((section) => (
          <section className={`panel help-section${section.steps ? " help-section-primary" : ""}`} id={section.id} key={section.id}>
            <h2>{section.title}</h2>
            {section.paragraphs?.map((paragraph) => <p key={paragraph}>{paragraph}</p>)}
            {section.steps && (
              <ol className="help-steps">
                {section.steps.map((step, index) => <li key={step}><span>Step {index + 1}</span><p>{step}</p></li>)}
              </ol>
            )}
            {section.bullets && <ul>{section.bullets.map((bullet) => <li key={bullet}>{bullet}</li>)}</ul>}
          </section>
        ))}
      </div>
    </div>
  );
}
