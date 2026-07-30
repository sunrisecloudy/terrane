import { useEffect, useMemo, useState } from "react";

import { blobUrl, invokeJson } from "../terrane";
import type { Dish, MealEntry, MutationResult } from "../types";

const nutrients: Array<{
  field: keyof Dish;
  label: string;
  unit: string;
  step: string;
}> = [
  { field: "calories_kcal", label: "Calories", unit: "kcal", step: "1" },
  { field: "protein_g", label: "Protein", unit: "g", step: ".1" },
  { field: "carbs_g", label: "Carbs", unit: "g", step: ".1" },
  { field: "fat_g", label: "Fat", unit: "g", step: ".1" },
  { field: "fiber_g", label: "Fiber", unit: "g", step: ".1" },
  { field: "sugar_g", label: "Sugar", unit: "g", step: ".1" },
  { field: "sodium_mg", label: "Sodium", unit: "mg", step: "1" },
];

function localDateTime(value: string): string {
  const date = new Date(value);
  if (!Number.isFinite(date.getTime())) return "";
  const local = new Date(date.getTime() - date.getTimezoneOffset() * 60000);
  return local.toISOString().slice(0, 16);
}

function confidence(value: number): string {
  if (value >= 0.75) return "High confidence";
  if (value >= 0.5) return "Medium confidence";
  return "Low confidence";
}

function withDishTotals(entry: MealEntry, dishes: Dish[]): MealEntry {
  const totals: Record<string, number> = {};
  nutrients.forEach(({ field }) => {
    totals[field] =
      Math.round(
        dishes.reduce((sum, dish) => sum + Number(dish[field] || 0), 0) * 10,
      ) / 10;
  });
  return { ...entry, ...totals, dishes };
}

export function MealRoute({
  entry,
  onChanged,
  onDeleted,
}: {
  entry?: MealEntry;
  onChanged: (entry: MealEntry, history: MealEntry[]) => void;
  onDeleted: (history: MealEntry[]) => void;
}) {
  const [draft, setDraft] = useState<MealEntry | undefined>(entry);
  const [status, setStatus] = useState("");
  const [error, setError] = useState("");

  useEffect(() => setDraft(entry), [entry]);
  const source = useMemo(
    () =>
      draft?.provider === "codex"
        ? "Analyzed with Codex"
        : draft?.provider === "claude"
          ? "Analyzed with Claude"
          : `Analyzed with OpenCode · ${draft?.model || "opencode-go/kimi-k2.6"}`,
    [draft],
  );

  if (!draft) {
    return (
      <section className="panel route-empty">
        <strong>Meal not found</strong>
        <p>This meal may have been removed, or the deep link is no longer valid.</p>
      </section>
    );
  }

  function updateDish(index: number, changes: Partial<Dish>) {
    if (!draft) return;
    const dishes = draft.dishes.map((dish, candidate) =>
      candidate === index ? { ...dish, ...changes } : dish,
    );
    setDraft(withDishTotals(draft, dishes));
  }

  function updateNumber(field: keyof Dish, value: string) {
    const number = Number(value);
    if (!draft || !Number.isFinite(number) || number < 0) return;
    setDraft({ ...draft, [field]: number });
  }

  async function save() {
    if (!draft) return;
    setStatus("Saving…");
    setError("");
    try {
      const result = await invokeJson<MutationResult>(
        "update",
        draft.id,
        JSON.stringify(draft),
      );
      if (!result.ok || !result.estimate) {
        throw new Error(result.error || "Could not save the review.");
      }
      setDraft(result.estimate);
      onChanged(result.estimate, result.history || []);
      setStatus("Saved");
    } catch (caught) {
      setStatus("");
      setError(caught instanceof Error ? caught.message : "Could not save the review.");
    }
  }

  async function remove() {
    if (!draft || !window.confirm("Delete this meal and its photo?")) return;
    try {
      const result = await invokeJson<{ ok: boolean; history: MealEntry[] }>(
        "remove",
        draft.id,
      );
      onDeleted(result.history || []);
    } catch (caught) {
      setError(caught instanceof Error ? caught.message : "Could not delete the meal.");
    }
  }

  return (
    <div className="route-stack">
      <div className="route-heading">
        <div>
          <h2>Meal details</h2>
          <p className="subtitle">Review each dish and save corrected nutrition.</p>
        </div>
        <span className="confidence">{confidence(draft.confidence)}</span>
      </div>
      <section className="panel result visible">
        <div className="result-layout has-image">
          <aside className="result-photo-panel">
            <img
              className="result-image"
              src={blobUrl(draft.blob_name)}
              alt={`${draft.food_name} photo`}
            />
          </aside>
          <div className="result-nutrition">
            <div className="result-head">
              <div className="result-title">
                <input
                  aria-label="Meal name"
                  value={draft.food_name}
                  onChange={(event) => setDraft({ ...draft, food_name: event.target.value })}
                />
                <input
                  className="serving"
                  aria-label="Serving description"
                  value={draft.serving_description}
                  onChange={(event) =>
                    setDraft({ ...draft, serving_description: event.target.value })
                  }
                />
                <span className="result-source">{source}</span>
                <label className="meal-time">
                  Eaten
                  <input
                    type="datetime-local"
                    value={localDateTime(draft.eaten_at)}
                    onChange={(event) =>
                      setDraft({
                        ...draft,
                        eaten_at: new Date(event.target.value).toISOString(),
                      })
                    }
                  />
                </label>
              </div>
            </div>
            {draft.dishes?.length ? (
              <section className="dish-breakdown">
                <div className="dish-breakdown-head">
                  <h3>Dishes in this photo</h3>
                  <span>Edit each dish; the meal total updates automatically.</span>
                </div>
                <div className="dish-list">
                  {draft.dishes.map((dish, index) => (
                    <article className="dish-card" key={`${draft.id}-${index}`}>
                      <div className="dish-card-head">
                        <input
                          aria-label={`Dish ${index + 1} name`}
                          value={dish.food_name}
                          onChange={(event) =>
                            updateDish(index, { food_name: event.target.value })
                          }
                        />
                        <span className="dish-confidence">{confidence(dish.confidence)}</span>
                      </div>
                      <input
                        className="dish-serving"
                        aria-label={`${dish.food_name} serving`}
                        value={dish.serving_description}
                        onChange={(event) =>
                          updateDish(index, { serving_description: event.target.value })
                        }
                      />
                      <div className="dish-number-grid">
                        {nutrients.map((nutrient) => (
                          <label className="dish-number" key={nutrient.field}>
                            {nutrient.label}
                            <span>
                              <input
                                type="number"
                                min="0"
                                step={nutrient.step}
                                value={Number(dish[nutrient.field])}
                                onChange={(event) =>
                                  updateDish(index, {
                                    [nutrient.field]: Math.max(
                                      0,
                                      Number(event.target.value) || 0,
                                    ),
                                  })
                                }
                              />
                              {nutrient.unit}
                            </span>
                          </label>
                        ))}
                      </div>
                    </article>
                  ))}
                </div>
              </section>
            ) : null}
            <div className="meal-total-label">Combined meal total</div>
            <div className="calorie">
              <input
                type="number"
                min="0"
                value={draft.calories_kcal}
                readOnly={Boolean(draft.dishes?.length)}
                onChange={(event) => updateNumber("calories_kcal", event.target.value)}
              />
              <span>kcal estimated</span>
            </div>
            <div className="macro-grid">
              {[
                ["protein_g", "Protein", "var(--blue)"],
                ["carbs_g", "Carbs", "var(--orange)"],
                ["fat_g", "Fat", "var(--red)"],
              ].map(([field, label, color]) => (
                <div className="macro" style={{ "--macro-color": color } as React.CSSProperties} key={field}>
                  <label>{label}</label>
                  <input
                    type="number"
                    min="0"
                    step=".1"
                    value={Number(draft[field as keyof Dish])}
                    readOnly={Boolean(draft.dishes?.length)}
                    onChange={(event) => updateNumber(field as keyof Dish, event.target.value)}
                  />{" "}g
                </div>
              ))}
            </div>
            <div className="details">
              {[
                ["fiber_g", "Fiber", "g"],
                ["sugar_g", "Sugar", "g"],
                ["sodium_mg", "Sodium", "mg"],
              ].map(([field, label, unit]) => (
                <label className="detail" key={field}>
                  {label}
                  <span>
                    <input
                      type="number"
                      min="0"
                      value={Number(draft[field as keyof Dish])}
                      readOnly={Boolean(draft.dishes?.length)}
                      onChange={(event) => updateNumber(field as keyof Dish, event.target.value)}
                    />{" "}{unit}
                  </span>
                </label>
              ))}
            </div>
            {draft.assumptions?.length || draft.warnings?.length ? (
              <div className="notes">
                {draft.assumptions?.length ? (
                  <><h3>Assumptions</h3><ul>{draft.assumptions.map((item) => <li key={item}>{item}</li>)}</ul></>
                ) : null}
                {draft.warnings?.length ? (
                  <><h3>Check carefully</h3><ul>{draft.warnings.map((item) => <li key={item}>{item}</li>)}</ul></>
                ) : null}
              </div>
            ) : null}
            {error ? <div className="error visible" role="alert">{error}</div> : null}
            <div className="review-row">
              <button className="danger-button" type="button" onClick={remove}>Delete meal</button>
              <span role="status">{status}</span>
              <button className="save" type="button" onClick={save}>Save review</button>
            </div>
          </div>
        </div>
      </section>
    </div>
  );
}
