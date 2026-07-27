import type { MealEntry, Period } from "../types";

type Summary = {
  entries: MealEntry[];
  totals: Record<string, number>;
  daily_calories: number;
  days: number;
};

function value(number: number, unit: string): string {
  const rounded = Math.round((number || 0) * 10) / 10;
  return `${rounded} ${unit}`;
}

export function NutritionSummary({
  summary,
  period,
  label,
  onPeriod,
  onOpen,
}: {
  summary: Summary;
  period: Period;
  label: string;
  onPeriod: (period: Period) => void;
  onOpen: (entry: MealEntry) => void;
}) {
  return (
    <div className="summary-panel">
      <div className="period-tabs" aria-label="Nutrition summary period">
        {(["day", "week", "month"] as Period[]).map((item) => (
          <button
            type="button"
            className={period === item ? "active" : ""}
            onClick={() => onPeriod(item)}
            key={item}
          >
            {item[0].toUpperCase() + item.slice(1)}
          </button>
        ))}
      </div>
      <p className="summary-label">{label}</p>
      <div className="summary-kcal">
        <strong>{Math.round(summary.totals.calories_kcal || 0)}</strong>
        <span>kcal total</span>
      </div>
      <div className="summary-metrics">
        <div className="summary-metric"><span>Protein</span><strong>{value(summary.totals.protein_g, "g")}</strong></div>
        <div className="summary-metric"><span>Carbs</span><strong>{value(summary.totals.carbs_g, "g")}</strong></div>
        <div className="summary-metric"><span>Fat</span><strong>{value(summary.totals.fat_g, "g")}</strong></div>
        <div className="summary-metric"><span>Fiber</span><strong>{value(summary.totals.fiber_g, "g")}</strong></div>
        <div className="summary-metric"><span>Sugar</span><strong>{value(summary.totals.sugar_g, "g")}</strong></div>
        <div className="summary-metric"><span>Sodium</span><strong>{value(summary.totals.sodium_mg, "mg")}</strong></div>
      </div>
      <p className="summary-average">
        {period === "day"
          ? `${summary.entries.length} saved ${summary.entries.length === 1 ? "meal" : "meals"}`
          : `${summary.daily_calories} kcal daily average across ${summary.days} days`}
      </p>
      <div className="period-meals">
        {summary.entries.length ? (
          summary.entries.map((entry) => (
            <button
              className="period-meal"
              type="button"
              key={entry.id}
              onClick={() => onOpen(entry)}
            >
              <img src={window.terrane?.blobUrl?.(entry.blob_name) || ""} alt="" />
              <strong>{entry.food_name}</strong>
              <span>{Math.round(entry.calories_kcal)} kcal</span>
            </button>
          ))
        ) : (
          <p className="period-empty">No meals saved in this period.</p>
        )}
      </div>
    </div>
  );
}
