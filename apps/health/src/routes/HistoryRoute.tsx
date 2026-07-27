import { MealCard } from "../components/MealCard";
import type { MealEntry } from "../types";

export function HistoryRoute({
  history,
  onOpen,
}: {
  history: MealEntry[];
  onOpen: (entry: MealEntry) => void;
}) {
  return (
    <div className="route-stack">
      <div className="route-heading">
        <div>
          <h2>Meal history</h2>
          <p className="subtitle">Review saved meals, photos, dishes, and nutrition.</p>
        </div>
        <span className="route-count">{history.length} meals</span>
      </div>
      {history.length ? (
        <div className="history-grid route-history-grid">
          {history.map((entry) => (
            <MealCard entry={entry} onOpen={onOpen} key={entry.id} />
          ))}
        </div>
      ) : (
        <section className="panel route-empty">
          <strong>No meals yet</strong>
          <p>Add a food photo to start your nutrition history.</p>
        </section>
      )}
    </div>
  );
}
