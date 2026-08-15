import { useState } from "react";

export function ClipsPage() {
  const [search, setSearch] = useState("");
  const [game, setGame] = useState("All Games");
  const [sort, setSort] = useState("Newest First");

  return (
    <div className="page">
      <header className="page-header">
        <div>
          <h1>Clips</h1>
          <p>Your saved moments will appear here.</p>
        </div>
      </header>

      <section className="clips-panel" aria-label="Clips library">
        <div className="clips-toolbar">
          <label className="search-field">
            <span className="visually-hidden">Search clips</span>
            <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="7" /><path d="m20 20-4-4" /></svg>
            <input value={search} onChange={(event) => setSearch(event.target.value)} placeholder="Search clips..." />
          </label>
          <label>
            <span className="visually-hidden">Filter by game</span>
            <select value={game} onChange={(event) => setGame(event.target.value)}>
              <option>All Games</option>
            </select>
          </label>
          <label>
            <span className="visually-hidden">Sort clips</span>
            <select value={sort} onChange={(event) => setSort(event.target.value)}>
              <option>Newest First</option>
              <option>Oldest First</option>
            </select>
          </label>
        </div>

        <div className="empty-state">
          <div className="empty-state-icon" aria-hidden="true">
            <svg viewBox="0 0 24 24"><rect x="3" y="5" width="18" height="14" rx="2" /><path d="m10 9 5 3-5 3Z" /></svg>
          </div>
          <h2>No clips yet</h2>
          <p>Saved replays will appear here once the capture engine is connected.</p>
        </div>
      </section>
    </div>
  );
}
