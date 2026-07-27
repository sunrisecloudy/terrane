import "../domain/calendar.js";

import { useMemo, useState } from "react";

import { NutritionSummary } from "../components/NutritionSummary";
import type { MealEntry, Period } from "../types";

export function InsightsRoute({
  history,
  initialDate,
  initialPeriod = "week",
  onOpen,
}: {
  history: MealEntry[];
  initialDate?: string;
  initialPeriod?: Period;
  onOpen: (entry: MealEntry) => void;
}) {
  const [period, setPeriod] = useState<Period>(initialPeriod);
  const [date, setDate] = useState(() => {
    const parsed = initialDate ? new Date(`${initialDate}T12:00:00`) : new Date();
    return Number.isFinite(parsed.getTime()) ? parsed : new Date();
  });
  const summary = useMemo(
    () => HealthCalendar.summarize(history, period, date),
    [history, period, date],
  );
  const label = `${summary.start.toLocaleDateString([], { month: "long", day: "numeric" })} – ${new Date(summary.end.getTime() - 86400000).toLocaleDateString([], { month: "long", day: "numeric", year: "numeric" })}`;

  return (
    <div className="route-stack">
      <div className="route-heading">
        <div>
          <h2>Nutrition insights</h2>
          <p className="subtitle">Compare daily, weekly, and monthly totals.</p>
        </div>
        <input
          className="route-date"
          type="date"
          value={HealthCalendar.dateKey(date)}
          onChange={(event) => setDate(new Date(`${event.target.value}T12:00:00`))}
        />
      </div>
      <NutritionSummary
        summary={summary}
        period={period}
        label={label}
        onPeriod={setPeriod}
        onOpen={onOpen}
      />
    </div>
  );
}
