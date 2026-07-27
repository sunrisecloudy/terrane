(function (root, factory) {
  var api = factory();
  if (typeof module === "object" && module.exports) {
    module.exports = api;
  } else {
    root.HealthCalendar = api;
  }
})(typeof globalThis === "object" ? globalThis : this, function () {
  var NUTRIENTS = [
    "calories_kcal",
    "protein_g",
    "carbs_g",
    "fat_g",
    "fiber_g",
    "sugar_g",
    "sodium_mg"
  ];

  function startOfDay(value) {
    var date = value instanceof Date ? new Date(value.getTime()) : new Date(value);
    if (!isFinite(date.getTime())) return null;
    date.setHours(0, 0, 0, 0);
    return date;
  }

  function dateKey(value) {
    var date = startOfDay(value);
    if (!date) return "";
    return [
      date.getFullYear(),
      String(date.getMonth() + 1).padStart(2, "0"),
      String(date.getDate()).padStart(2, "0")
    ].join("-");
  }

  function entryDate(entry) {
    if (!entry || typeof entry.eaten_at !== "string") return null;
    var date = new Date(entry.eaten_at);
    return isFinite(date.getTime()) ? date : null;
  }

  function addDays(value, amount) {
    var date = startOfDay(value);
    date.setDate(date.getDate() + amount);
    return date;
  }

  function rangeFor(mode, selected) {
    var anchor = startOfDay(selected) || startOfDay(new Date());
    var start = new Date(anchor.getTime());
    var end;
    if (mode === "week") {
      var mondayOffset = (start.getDay() + 6) % 7;
      start = addDays(start, -mondayOffset);
      end = addDays(start, 7);
    } else if (mode === "month") {
      start = new Date(start.getFullYear(), start.getMonth(), 1);
      end = new Date(start.getFullYear(), start.getMonth() + 1, 1);
    } else {
      end = addDays(start, 1);
    }
    return { start: start, end: end };
  }

  function numeric(value) {
    var number = typeof value === "number" ? value : parseFloat(value);
    return isFinite(number) && number >= 0 ? number : 0;
  }

  function summarize(entries, mode, selected) {
    var range = rangeFor(mode, selected);
    var matched = (entries || []).filter(function (entry) {
      var date = entryDate(entry);
      return date && date >= range.start && date < range.end;
    });
    matched.sort(function (left, right) {
      return entryDate(right).getTime() - entryDate(left).getTime();
    });
    var totals = {};
    NUTRIENTS.forEach(function (key) { totals[key] = 0; });
    matched.forEach(function (entry) {
      NUTRIENTS.forEach(function (key) { totals[key] += numeric(entry[key]); });
    });
    NUTRIENTS.forEach(function (key) {
      totals[key] = Math.round(totals[key] * 10) / 10;
    });
    var dayCount = Math.max(
      1,
      Math.round((range.end.getTime() - range.start.getTime()) / 86400000)
    );
    return {
      start: range.start,
      end: range.end,
      entries: matched,
      totals: totals,
      daily_calories: Math.round(totals.calories_kcal / dayCount),
      days: dayCount
    };
  }

  function monthCells(cursor, entries, today) {
    var month = startOfDay(cursor) || startOfDay(new Date());
    month = new Date(month.getFullYear(), month.getMonth(), 1);
    var firstCell = addDays(month, -month.getDay());
    var byDate = {};
    (entries || []).forEach(function (entry) {
      var date = entryDate(entry);
      if (!date) return;
      var key = dateKey(date);
      if (!byDate[key]) byDate[key] = { count: 0, calories: 0 };
      byDate[key].count += 1;
      byDate[key].calories += numeric(entry.calories_kcal);
    });
    var todayKey = dateKey(today || new Date());
    var cells = [];
    for (var index = 0; index < 42; index++) {
      var date = addDays(firstCell, index);
      var key = dateKey(date);
      var summary = byDate[key] || { count: 0, calories: 0 };
      cells.push({
        date: date,
        key: key,
        day: date.getDate(),
        currentMonth: date.getMonth() === month.getMonth(),
        today: key === todayKey,
        count: summary.count,
        calories: Math.round(summary.calories)
      });
    }
    return cells;
  }

  return {
    dateKey: dateKey,
    entryDate: entryDate,
    monthCells: monthCells,
    rangeFor: rangeFor,
    startOfDay: startOfDay,
    summarize: summarize
  };
});
