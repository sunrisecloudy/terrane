import { useState } from "react";

import { invokeJson } from "../terrane";
import type { Settings } from "../types";

export function SettingsRoute({
  settings,
  onSettings,
}: {
  settings: Settings;
  onSettings: (settings: Settings) => void;
}) {
  const [draft, setDraft] = useState(settings);
  const [status, setStatus] = useState("");

  async function save() {
    setStatus("Saving…");
    try {
      await invokeJson("configure", draft.provider, draft.model);
      onSettings(draft);
      setStatus("Saved");
    } catch (caught) {
      setStatus(caught instanceof Error ? caught.message : "Could not save settings.");
    }
  }

  return (
    <div className="route-stack">
      <div className="route-heading">
        <div>
          <h2>Health settings</h2>
          <p className="subtitle">Choose the vision provider used for new estimates.</p>
        </div>
      </div>
      <section className="panel settings-panel">
        <label className="model-control">
          <span>Vision provider</span>
          <select
            value={draft.provider}
            onChange={(event) =>
              setDraft({
                ...draft,
                provider:
                  event.target.value === "codex"
                    ? "codex"
                    : event.target.value === "claude"
                      ? "claude"
                      : "opencode",
              })
            }
          >
            <option value="opencode">OpenCode</option>
            <option value="codex">Codex</option>
            <option value="claude">Claude</option>
          </select>
        </label>
        <label className="model-control">
          <span>Vision model</span>
          <input
            value={draft.model}
            disabled={draft.provider !== "opencode"}
            onChange={(event) => setDraft({ ...draft, model: event.target.value })}
          />
        </label>
        <p className="panel-help">
          OpenCode defaults to <strong>opencode-go/kimi-k2.6</strong>. Codex and
          Claude use their signed-in CLI configurations.
        </p>
        <div className="settings-actions">
          <button className="save" type="button" onClick={save}>Save settings</button>
          <span role="status">{status}</span>
        </div>
      </section>
    </div>
  );
}
