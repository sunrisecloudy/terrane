const test = require("node:test");
const assert = require("node:assert/strict");
const calendar = require("../src/domain/calendar.js");

function localIso(year, month, day, hour) {
  return new Date(year, month, day, hour || 12, 0, 0, 0).toISOString();
}

function meal(eatenAt, calories, protein) {
  return {
    eaten_at: eatenAt,
    calories_kcal: calories,
    protein_g: protein,
    carbs_g: 20,
    fat_g: 10,
    fiber_g: 3,
    sugar_g: 4,
    sodium_mg: 250
  };
}

test("monthCells returns a stable six-week grid and aggregates each local day", () => {
  const entries = [
    meal(localIso(2026, 6, 5, 8), 400, 20),
    meal(localIso(2026, 6, 5, 19), 600, 30),
    meal(localIso(2026, 6, 9, 12), 500, 25),
    meal(undefined, 999, 99)
  ];
  const cells = calendar.monthCells(new Date(2026, 6, 1), entries, new Date(2026, 6, 9));

  assert.equal(cells.length, 42);
  assert.equal(cells[0].date.getDay(), 0);
  assert.deepEqual(
    cells.find((cell) => cell.key === "2026-07-05"),
    {
      date: new Date(2026, 6, 5),
      key: "2026-07-05",
      day: 5,
      currentMonth: true,
      today: false,
      count: 2,
      calories: 1000
    }
  );
  assert.equal(cells.find((cell) => cell.key === "2026-07-09").today, true);
});

test("daily, Monday-based weekly, and monthly summaries total nutrition", () => {
  const entries = [
    meal(localIso(2026, 6, 6, 8), 400, 20),
    meal(localIso(2026, 6, 6, 19), 600, 30),
    meal(localIso(2026, 6, 12, 12), 700, 35),
    meal(localIso(2026, 6, 13, 12), 800, 40),
    meal(localIso(2026, 7, 1, 12), 900, 45),
    meal("not-a-date", 999, 99)
  ];

  const day = calendar.summarize(entries, "day", new Date(2026, 6, 6));
  assert.equal(day.entries.length, 2);
  assert.equal(day.totals.calories_kcal, 1000);
  assert.equal(day.totals.protein_g, 50);

  const week = calendar.summarize(entries, "week", new Date(2026, 6, 8));
  assert.equal(calendar.dateKey(week.start), "2026-07-06");
  assert.equal(calendar.dateKey(new Date(week.end.getTime() - 1)), "2026-07-12");
  assert.equal(week.entries.length, 3);
  assert.equal(week.totals.calories_kcal, 1700);
  assert.equal(week.daily_calories, 243);

  const month = calendar.summarize(entries, "month", new Date(2026, 6, 20));
  assert.equal(month.entries.length, 4);
  assert.equal(month.totals.calories_kcal, 2500);
  assert.equal(month.days, 31);
  assert.equal(month.daily_calories, 81);
});

test("undated history remains excluded instead of receiving an invented date", () => {
  assert.equal(calendar.entryDate({ food_name: "Legacy meal" }), null);
  assert.equal(calendar.entryDate({ eaten_at: "invalid" }), null);
  assert.equal(
    calendar.summarize([{ food_name: "Legacy meal", calories_kcal: 500 }], "month", new Date()).entries.length,
    0
  );
});
