const tracks = ["Video", "Game", "Discord", "Microphone"];
const editControls = ["Trim", "Split", "Delete Selection", "Undo", "Redo"];

export function EditorPage() {
  return (
    <div className="page">
      <header className="page-header">
        <div>
          <h1>Editor</h1>
          <p>Quick edits without a full video editor.</p>
        </div>
      </header>

      <div className="editor-workspace" aria-disabled="true">
        <section className="preview-area" aria-label="Video preview">
          <div className="preview-placeholder">
            <svg viewBox="0 0 24 24" aria-hidden="true"><path d="m9 8 7 4-7 4Z" /><rect x="3" y="4" width="18" height="16" rx="2" /></svg>
            <span>Select a clip to begin</span>
          </div>
        </section>

        <div className="editor-controls" aria-label="Editing controls">
          {editControls.map((control) => <button type="button" disabled key={control}>{control}</button>)}
        </div>

        <section className="timeline-panel" aria-label="Timeline">
          <div className="timeline-ruler"><span>00:00</span><span>00:15</span><span>00:30</span><span>00:45</span><span>01:00</span></div>
          {tracks.map((track) => (
            <div className="timeline-track" key={track}>
              <span>{track}</span>
              <div className="track-lane" />
            </div>
          ))}
        </section>

        <section className="audio-panel" aria-labelledby="editor-audio-heading">
          <div className="section-heading">
            <div>
              <span className="eyebrow">MIXER</span>
              <h2 id="editor-audio-heading">Audio</h2>
            </div>
            <span className="section-note">Clip required</span>
          </div>
          {["Game", "Discord", "Microphone"].map((source) => (
            <div className="volume-row" key={source}>
              <span>{source}</span>
              <input type="range" value="100" aria-label={`${source} volume`} disabled readOnly />
              <span>100%</span>
            </div>
          ))}
        </section>

        <div className="editor-footer">
          <span>Load a clip to enable editing tools.</span>
          <button className="primary-button" type="button" disabled>Export Clip</button>
        </div>
      </div>
    </div>
  );
}
