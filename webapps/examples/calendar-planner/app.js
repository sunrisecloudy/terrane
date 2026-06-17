(function () {
  const APP_ID = "calendar-planner";
  const KEY = APP_ID + ":events";
  const $ = (id) => document.getElementById(id);
  let events = [];
  let cursor = monthStart(new Date());
  let selectedDate = toDateKey(new Date());

  async function call(method, params) {
    if (window.AppRuntime && typeof window.AppRuntime.call === "function") {
      return window.AppRuntime.call(method, params);
    }
    window.__mockStorage = window.__mockStorage || new Map();
    if (method === "storage.get") {
      return { value: window.__mockStorage.has(params.key) ? window.__mockStorage.get(params.key) : params.defaultValue };
    }
    if (method === "storage.set") {
      window.__mockStorage.set(params.key, params.value);
      return { ok: true };
    }
    if (method === "core.step") {
      return {
        ok: true,
        stateVersion: Date.now(),
        actions: [{ type: "Toast", message: "Mock core accepted " + params.event.type }]
      };
    }
    if (method === "notification.toast" || method === "app.log") return { ok: true };
    throw new Error("Unknown mock method " + method);
  }

  async function load() {
    const result = await call("storage.get", { key: KEY, defaultValue: [] });
    events = Array.isArray(result.value) ? result.value.map(normalizeEvent).filter(Boolean) : [];
    $("event-date").value = selectedDate;
    render();
  }

  async function persist() {
    await call("storage.set", { key: KEY, value: events });
  }

  async function addEvent() {
    const title = $("event-title").value.trim();
    const date = $("event-date").value || selectedDate;
    const start = $("event-start").value || "09:00";
    const durationMinutes = Number($("event-duration").value || 60);
    const notes = $("event-notes").value.trim();
    if (!title) {
      $("event-title").focus();
      return;
    }

    const core = await call("core.step", {
      app: APP_ID,
      event: {
        type: "CreateCalendarEvent",
        payload: { title, date, start, durationMinutes, notes }
      }
    });
    $("core-output").textContent = JSON.stringify(core);

    const event = {
      id: "event_" + Date.now(),
      title,
      date,
      start,
      durationMinutes,
      notes,
      createdAt: Date.now()
    };
    events.unshift(event);
    selectedDate = date;
    cursor = monthStart(parseDateKey(date));
    await persist();
    await call("notification.toast", { message: "Event added", level: "success" });
    await call("app.log", { level: "info", message: "calendar event added" });
    $("event-title").value = "";
    $("event-notes").value = "";
    render();
  }

  function render() {
    renderMonth();
    renderAgenda();
  }

  function renderMonth() {
    $("month-label").textContent = monthName(cursor);
    const monthKey = cursor.getFullYear() + "-" + pad(cursor.getMonth() + 1);
    const monthEvents = events.filter((event) => event.date && event.date.startsWith(monthKey));
    $("month-summary").textContent = monthEvents.length === 1 ? "1 event" : monthEvents.length + " events";

    const grid = $("month-grid");
    grid.textContent = "";
    const first = monthStart(cursor);
    const gridStart = new Date(first.getFullYear(), first.getMonth(), 1 - first.getDay());
    for (let index = 0; index < 42; index += 1) {
      const date = new Date(gridStart.getFullYear(), gridStart.getMonth(), gridStart.getDate() + index);
      const dateKey = toDateKey(date);
      const button = document.createElement("button");
      button.type = "button";
      button.className = "day";
      button.dataset.testid = "calendar-day-button";
      if (date.getMonth() !== cursor.getMonth()) button.classList.add("outside");
      if (dateKey === toDateKey(new Date())) button.classList.add("today");
      if (dateKey === selectedDate) button.classList.add("active");
      button.setAttribute("aria-label", readableDate(date));
      button.addEventListener("click", function () {
        selectedDate = dateKey;
        $("event-date").value = dateKey;
        render();
      });

      const number = document.createElement("span");
      number.className = "day-number";
      number.textContent = String(date.getDate());
      button.append(number);

      const count = events.filter((event) => event.date === dateKey).length;
      if (count > 0) {
        const dot = document.createElement("span");
        dot.className = "event-dot";
        dot.textContent = count === 1 ? "1 event" : count + " events";
        button.append(dot);
      }
      grid.append(button);
    }
  }

  function renderAgenda() {
    const dayEvents = events
      .filter((event) => event.date === selectedDate)
      .sort((a, b) => (a.start || "").localeCompare(b.start || ""));
    $("selected-date-label").textContent = readableDate(parseDateKey(selectedDate));
    const list = $("agenda-list");
    list.textContent = "";
    $("agenda-empty").hidden = dayEvents.length !== 0;
    for (const event of dayEvents) {
      const item = document.createElement("li");
      item.className = "agenda-item";
      const time = document.createElement("div");
      time.className = "agenda-time";
      time.textContent = (event.start || "09:00") + " / " + String(event.durationMinutes || 60) + " min";
      const title = document.createElement("div");
      title.className = "agenda-title";
      title.textContent = event.title;
      item.append(time, title);
      if (event.notes) {
        const notes = document.createElement("div");
        notes.className = "agenda-notes";
        notes.textContent = event.notes;
        item.append(notes);
      }
      list.append(item);
    }
  }

  function normalizeEvent(value) {
    if (!value || typeof value !== "object") return null;
    return {
      id: typeof value.id === "string" ? value.id : "event_" + String(value.createdAt || Date.now()),
      title: String(value.title || "Untitled"),
      date: /^\d{4}-\d{2}-\d{2}$/.test(String(value.date || "")) ? String(value.date) : selectedDate,
      start: /^\d{2}:\d{2}$/.test(String(value.start || "")) ? String(value.start) : "09:00",
      durationMinutes: Number(value.durationMinutes || 60),
      notes: String(value.notes || ""),
      createdAt: Number(value.createdAt || Date.now())
    };
  }

  function monthStart(date) {
    return new Date(date.getFullYear(), date.getMonth(), 1);
  }

  function parseDateKey(value) {
    const parts = String(value).split("-").map((part) => Number(part));
    if (parts.length !== 3 || parts.some((part) => !Number.isFinite(part))) return new Date();
    return new Date(parts[0], parts[1] - 1, parts[2]);
  }

  function toDateKey(date) {
    return date.getFullYear() + "-" + pad(date.getMonth() + 1) + "-" + pad(date.getDate());
  }

  function pad(value) {
    return String(value).padStart(2, "0");
  }

  function monthName(date) {
    return date.toLocaleString(undefined, { month: "long", year: "numeric" });
  }

  function readableDate(date) {
    return date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric", year: "numeric" });
  }

  $("prev-month").addEventListener("click", function () {
    cursor = new Date(cursor.getFullYear(), cursor.getMonth() - 1, 1);
    render();
  });
  $("next-month").addEventListener("click", function () {
    cursor = new Date(cursor.getFullYear(), cursor.getMonth() + 1, 1);
    render();
  });
  $("today").addEventListener("click", function () {
    selectedDate = toDateKey(new Date());
    cursor = monthStart(new Date());
    $("event-date").value = selectedDate;
    render();
  });
  $("add-event").addEventListener("click", addEvent);
  $("event-title").addEventListener("keydown", function (event) {
    if (event.key === "Enter") addEvent();
  });

  load().catch(function (error) {
    $("agenda-empty").hidden = false;
    $("agenda-empty").textContent = "Failed to load: " + error.message;
  });
})();
