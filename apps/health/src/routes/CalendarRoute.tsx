import "../domain/calendar.js";

import { useMemo, useState } from "react";

import { NutritionSummary } from "../components/NutritionSummary";
import type { MealEntry, Period } from "../types";

declare global {
  var HealthCalendar: {
    dateKey(value: Date | string): string;
    monthCells(cursor: Date, entries: MealEntry[], today: Date): Array<{
      date: Date;
      key: string;
      day: number;
      currentMonth: boolean;
      today: boolean;
      count: number;
      calories: number;
    }>;
    startOfDay(value: Date | string): Date;
    summarize(entries: MealEntry[], mode: Period, selected: Date): {
      entries: MealEntry[];
      totals: Record<string, number>;
      daily_calories: number;
      days: number;
      start: Date;
      end: Date;
    };
  };
}

function parseDate(value?: string): Date {
  if (value) {
    const parsed = new Date(`${value}T12:00:00`);
    if (Number.isFinite(parsed.getTime())) return parsed;
  }
  return new Date();
}

function summaryLabel(period: Period, date: Date): string {
  if (period === "day") {
    return date.toLocaleDateString([], {
      weekday: "long",
      month: "long",
      day: "numeric",
    });
  }
  if (period === "month") {
    return date.toLocaleDateString([], { month: "long", year: "numeric" });
  }
  const range = HealthCalendar.summarize([], "week", date);
  const end = new Date(range.end.getTime() - 86400000);
  return `${range.start.toLocaleDateString([], { month: "short", day: "numeric" })} – ${end.toLocaleDateString([], { month: "short", day: "numeric" })}`;
}

export function CalendarRoute({
  history,
  initialDate,
  initialPeriod = "day",
  onOpen,
}: {
  history: MealEntry[];
  initialDate?: string;
  initialPeriod?: Period;
  onOpen: (entry: MealEntry) => void;
}) {
  const [selected, setSelected] = useState(() => parseDate(initialDate));
  const [period, setPeriod] = useState<Period>(initialPeriod);
  const [cursor, setCursor] = useState(
    () => new Date(selected.getFullYear(), selected.getMonth(), 1),
  );
  const cells = useMemo(
    () => HealthCalendar.monthCells(cursor, history, new Date()),
    [cursor, history],
  );
  const summary = useMemo(
    () => HealthCalendar.summarize(history, period, selected),
    [history, period, selected],
  );

  function moveMonth(amount: number) {
    const next = new Date(cursor.getFullYear(), cursor.getMonth() + amount, 1);
    setCursor(next);
    setSelected(next);
  }

  function today() {
    const next = new Date();
    setSelected(next);
    setCursor(new Date(next.getFullYear(), next.getMonth(), 1));
  }

  return (
    <div className="route-stack">
      <section className="panel insights">
        <div className="insights-head">
          <div>
            <h2>Nutrition calendar</h2>
            <p className="subtitle">See meals and nutrition totals over time.</p>
          </div>
          <div className="month-controls">
            <button type="button" aria-label="Previous month" onClick={() => moveMonth(-1)}>←</button>
            <button type="button" onClick={today}>Today</button>
            <span>{cursor.toLocaleDateString([], { month: "long", year: "numeric" })}</span>
            <button type="button" aria-label="Next month" onClick={() => moveMonth(1)}>→</button>
          </div>
        </div>
        <div className="insights-grid">
          <div className="calendar">
            <div className="calendar-weekdays" aria-hidden="true">
              <span>Sun</span><span>Mon</span><span>Tue</span><span>Wed</span>
              <span>Thu</span><span>Fri</span><span>Sat</span>
            </div>
            <div className="calendar-grid">
              {cells.map((cell) => (
                <button
                  type="button"
                  key={cell.key}
                  className={[
                    "calendar-day",
                    cell.currentMonth ? "" : "other-month",
                    cell.today ? "today" : "",
                    cell.key === HealthCalendar.dateKey(selected) ? "selected" : "",
                    cell.count ? "has-meals" : "",
                  ].filter(Boolean).join(" ")}
                  onClick={() => {
                    setSelected(cell.date);
                    setCursor(new Date(cell.date.getFullYear(), cell.date.getMonth(), 1));
                  }}
                >
                  <strong>{cell.day}</strong>
                  {cell.count ? (
                    <small>{cell.count} {cell.count === 1 ? "meal" : "meals"} · {cell.calories} kcal</small>
                  ) : null}
                </button>
              ))}
            </div>
          </div>
          <NutritionSummary
            summary={summary}
            period={period}
            label={summaryLabel(period, selected)}
            onPeriod={setPeriod}
            onOpen={onOpen}
          />
        </div>
      </section>
    </div>
  );
}
