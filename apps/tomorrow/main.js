var kv = ctx.resource.kv;
var STATE_KEY = "tomorrow/state/v1";

function defaultDays() {
  var themes = [
    ["Begin softly", "Notice what would make tomorrow feel kinder."],
    ["Make a little room", "Protect one small pocket for what matters."],
    ["Follow curiosity", "Let one interesting thing interrupt the ordinary."],
    ["Choose enough", "A shorter, honest plan can carry the day."],
    ["Offer warmth", "Make one interaction gentler than it had to be."],
    ["Keep what helped", "Notice a pattern worth carrying forward."],
    ["Look back kindly", "Complete the week without grading yourself."]
  ];
  return themes.map(function (theme, index) {
    return {
      day: index + 1,
      title: theme[0],
      preview: theme[1],
      items: [],
      reflection: "",
      encouragement: "",
      carriedInsight: "",
      complete: false
    };
  });
}

function initialState() {
  return {
    version: 1,
    started: false,
    intention: "",
    customIntent: "",
    selectedDay: 1,
    answers: { energy: "steady", anchor: "", space: "a little" },
    days: defaultDays(),
    settings: {
      harness: "templates",
      verifiedHarnesses: [],
      calendar: { status: "disconnected", approved: false },
      gmail: { status: "disconnected", approved: false }
    },
    nextItemId: 1,
    createdAt: ""
  };
}

function readState() {
  try {
    var raw = kv.get(STATE_KEY);
    if (!raw) return initialState();
    var parsed = JSON.parse(raw);
    if (!parsed.days || parsed.days.length !== 7) return initialState();
    return parsed;
  } catch (_) {
    return initialState();
  }
}

function writeState(state) {
  kv.set(STATE_KEY, JSON.stringify(state));
  return state;
}

function output(value) {
  return JSON.stringify(value);
}

function clean(value, limit) {
  return String(value == null ? "" : value).trim().slice(0, limit || 240);
}

function number(value, fallback) {
  var parsed = parseInt(value, 10);
  return isFinite(parsed) ? parsed : fallback;
}

function currentDay(state) {
  return state.days[Math.max(0, Math.min(6, state.selectedDay - 1))];
}

function templatePlan(state) {
  var intent = state.customIntent || state.intention || "a kinder tomorrow";
  var anchor = clean(state.answers.anchor, 120) || "one thing that would feel quietly worthwhile";
  var space = state.answers.space || "a little";
  var energy = state.answers.energy || "steady";
  return [
    {
      id: state.nextItemId++,
      type: "task",
      title: "Begin with " + anchor,
      time: energy === "low" ? "When you feel ready" : "A gentle first step",
      note: "Keep it small enough to begin.",
      done: false
    },
    {
      id: state.nextItemId++,
      type: "pause",
      title: "Leave " + space + " space for the unexpected",
      time: "Sometime in the middle",
      note: "This is room, not an assignment.",
      done: false
    },
    {
      id: state.nextItemId++,
      type: "reflection",
      title: "Notice one sign of " + intent,
      time: "Toward evening",
      note: "No score, just a moment to remember.",
      done: false
    }
  ];
}

function actions() {
  return {
    app: "tomorrow",
    title: "Tomorrow",
    actions: [
      { verb: "state", summary: "Read the local seven-day journey.", args: [], returns: "JSON state" },
      { verb: "start_journey", summary: "Create day one's template plan.", args: [{ name: "detailsJson", required: true }], returns: "JSON state" },
      { verb: "select_day", summary: "Select one of seven journey days.", args: [{ name: "day", required: true }], returns: "JSON state" },
      { verb: "add_item", summary: "Add a user-entered task or meeting.", args: [{ name: "itemJson", required: true }], returns: "JSON state" },
      { verb: "edit_item", summary: "Edit a user-entered plan item.", args: [{ name: "id", required: true }, { name: "itemJson", required: true }], returns: "JSON state" },
      { verb: "reorder_item", summary: "Move a plan item at the user's request.", args: [{ name: "id", required: true }, { name: "direction", required: true }], returns: "JSON state" },
      { verb: "toggle_item", summary: "Mark a plan item complete or open.", args: [{ name: "id", required: true }], returns: "JSON state" },
      { verb: "save_reflection", summary: "Save an evening reflection locally.", args: [{ name: "reflectionJson", required: true }], returns: "JSON state" },
      { verb: "save_settings", summary: "Save local settings without connecting services.", args: [{ name: "settingsJson", required: true }], returns: "JSON state" },
      { verb: "common.receive", summary: "Receive an interop payload into a local, review-only inbox.", args: [{ name: "kind", required: true }, { name: "payloadJson", required: true }], returns: "JSON acknowledgement" },
      { verb: "common.list", summary: "List journey days as addressable items.", args: [], returns: "JSON array" },
      { verb: "common.get", summary: "Read one journey day by id.", args: [{ name: "id", required: true }], returns: "JSON item or typed not-found" }
    ]
  };
}

function handle(input) {
  var verb = String(input[0] || "");
  var args = input.slice(1);
  if (verb === "__actions__") return output(actions());

  var state = readState();
  if (verb === "state") return output(state);

  if (verb === "common.receive") {
    var inboxId = "inbox-" + String(state.nextItemId++);
    kv.set("tomorrow/" + inboxId, output({
      id: inboxId,
      kind: clean(args[0], 32) || "json",
      payload: clean(args[1], 4000),
      status: "received_for_review"
    }));
    writeState(state);
    return output({ ok: true, id: inboxId, status: "received_for_review", scheduled: false, sent: false });
  }

  if (verb === "common.list") {
    return output(state.days.map(function (day) {
      return { id: "day-" + day.day, title: "Day " + day.day + " · " + day.title, kind: "journey-day" };
    }));
  }

  if (verb === "common.get") {
    var commonId = clean(args[0], 80);
    var commonDay = number(commonId.replace("day-", ""), -1);
    if (commonDay < 1 || commonDay > 7) {
      return output({ ok: false, error: { code: "NotFound", id: commonId } });
    }
    return output(state.days[commonDay - 1]);
  }

  if (verb === "start_journey") {
    var details = {};
    try { details = JSON.parse(args[0] || "{}"); } catch (_) {}
    var intention = clean(details.intention, 80);
    var custom = clean(details.customIntent, 120);
    if (!intention && !custom) return output({ ok: false, error: "Choose an intention first." });
    state.started = true;
    state.intention = intention || "own";
    state.customIntent = custom;
    state.answers = {
      energy: clean(details.energy, 24) || "steady",
      anchor: clean(details.anchor, 120),
      space: clean(details.space, 40) || "a little"
    };
    state.createdAt = clean(details.createdAt, 40);
    state.selectedDay = 1;
    state.days[0].items = templatePlan(state);
    writeState(state);
    return output(state);
  }

  if (verb === "select_day") {
    state.selectedDay = Math.max(1, Math.min(7, number(args[0], 1)));
    writeState(state);
    return output(state);
  }

  if (verb === "add_item") {
    var item = {};
    try { item = JSON.parse(args[0] || "{}"); } catch (_) {}
    var title = clean(item.title, 140);
    if (!title) return output({ ok: false, error: "Give this plan item a name." });
    currentDay(state).items.push({
      id: state.nextItemId++,
      type: ["task", "meeting", "pause", "reflection"].indexOf(item.type) >= 0 ? item.type : "task",
      title: title,
      time: clean(item.time, 80),
      note: clean(item.note, 240),
      done: false
    });
    writeState(state);
    return output(state);
  }

  if (verb === "edit_item") {
    var editId = number(args[0], -1);
    var changes = {};
    try { changes = JSON.parse(args[1] || "{}"); } catch (_) {}
    var items = currentDay(state).items;
    for (var i = 0; i < items.length; i++) {
      if (items[i].id === editId) {
        if (clean(changes.title, 140)) items[i].title = clean(changes.title, 140);
        items[i].type = ["task", "meeting", "pause", "reflection"].indexOf(changes.type) >= 0 ? changes.type : items[i].type;
        items[i].time = clean(changes.time, 80);
        items[i].note = clean(changes.note, 240);
      }
    }
    writeState(state);
    return output(state);
  }

  if (verb === "reorder_item") {
    var moveId = number(args[0], -1);
    var direction = args[1] === "down" ? 1 : -1;
    var plan = currentDay(state).items;
    var from = -1;
    for (var p = 0; p < plan.length; p++) if (plan[p].id === moveId) from = p;
    var to = from + direction;
    if (from >= 0 && to >= 0 && to < plan.length) {
      var moving = plan.splice(from, 1)[0];
      plan.splice(to, 0, moving);
    }
    writeState(state);
    return output(state);
  }

  if (verb === "toggle_item") {
    var toggleId = number(args[0], -1);
    currentDay(state).items.forEach(function (item) {
      if (item.id === toggleId) item.done = !item.done;
    });
    writeState(state);
    return output(state);
  }

  if (verb === "save_reflection") {
    var reflection = {};
    try { reflection = JSON.parse(args[0] || "{}"); } catch (_) {}
    var day = currentDay(state);
    day.reflection = clean(reflection.text, 1000);
    day.complete = !!day.reflection;
    day.encouragement = reflection.encouragement
      ? "You met this day as it was. That is enough to carry something gentle forward."
      : "";
    if (state.selectedDay < 7 && day.reflection) {
      state.days[state.selectedDay].carriedInsight =
        "From yesterday: " + day.reflection.slice(0, 160);
    }
    writeState(state);
    return output(state);
  }

  if (verb === "save_settings") {
    var settings = {};
    try { settings = JSON.parse(args[0] || "{}"); } catch (_) {}
    state.settings.harness = "templates";
    state.settings.verifiedHarnesses = [];
    if (settings.calendar === "disconnected") state.settings.calendar = { status: "disconnected", approved: false };
    if (settings.gmail === "disconnected") state.settings.gmail = { status: "disconnected", approved: false };
    writeState(state);
    return output(state);
  }

  return output({ ok: false, error: "Unknown action: " + verb });
}
