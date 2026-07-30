import { blobUrl } from "../terrane";
import type { MealEntry } from "../types";

export function MealCard({
  entry,
  onOpen,
}: {
  entry: MealEntry;
  onOpen: (entry: MealEntry) => void;
}) {
  const eaten = new Date(entry.eaten_at);
  const date = Number.isFinite(eaten.getTime())
    ? eaten.toLocaleString([], { dateStyle: "medium", timeStyle: "short" })
    : "Date not set";
  return (
    <button className="history-card" type="button" onClick={() => onOpen(entry)}>
      <img src={blobUrl(entry.blob_name)} alt="" />
      <div>
        <strong>{entry.food_name}</strong>
        <span>
          {Math.round(entry.calories_kcal)} kcal
          {entry.dishes?.length > 1 ? ` · ${entry.dishes.length} dishes` : ""}
          {` · ${date} · ${
            entry.provider === "codex"
              ? "Codex"
              : entry.provider === "claude"
                ? "Claude"
                : "OpenCode"
          }`}
          {entry.reviewed ? " · reviewed" : ""}
        </span>
      </div>
    </button>
  );
}
