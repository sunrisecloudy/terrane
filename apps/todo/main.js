// Todo backend for Terrane (UI + CLI).
//
// One kv key per fact, so each mutation is exactly one recorded kv.* event —
// Option-A replay rebuilds todos by folding those events, never by re-running
// this JS:
//
//   seq        -> highest id ever allocated, as a decimal string
//   item:<id>  -> the todo text for that id
//
// The app is a `description` + an `actions` table; the runtime synthesizes
// `handle`, the `__actions__` self-description, usage, and unknown-verb help. The
// UI (index.html) calls these verbs via window.terrane.invoke — `items` returns
// the list as JSON for rendering.

var kv = ctx.resource.kv;

var SEQ_KEY = "seq";
var ITEM_PREFIX = "item:";

// Read the id counter (0 if unset). Stored as a string; parse defensively.
function readSeq() {
  var raw = kv.get(SEQ_KEY);
  if (raw == null) {
    return 0; // missing key reads back as null/undefined
  }
  var n = parseInt(raw, 10);
  return isNaN(n) || n < 0 ? 0 : n;
}

// Every live todo as [{ id, text }, …], sorted by id.
function readItems() {
  var all = kv.all();
  var items = [];
  for (var key in all) {
    if (!Object.prototype.hasOwnProperty.call(all, key)) continue;
    if (key.indexOf(ITEM_PREFIX) !== 0) continue;
    var id = parseInt(key.slice(ITEM_PREFIX.length), 10);
    if (isNaN(id)) continue;
    items.push({ id: id, text: all[key] });
  }
  items.sort(function (a, b) {
    return a.id - b.id;
  });
  return items;
}

var description = "A simple todo list with a web UI (kv-backed).";

var actions = {
  add: {
    summary: "Add a todo item.",
    args: [{
      name: "text",
      required: true,
      summary: "the item text (may be several words)",
    }],
    returns: 'a confirmation line, e.g. "added #1 buy milk"',
    run: function (args, usage) {
      var text = args.join(" ").trim();
      if (text === "") return usage();
      var id = readSeq() + 1;
      kv.set(SEQ_KEY, String(id));
      kv.set(ITEM_PREFIX + id, text);
      return "added #" + id + " " + text;
    },
  },

  list: {
    summary: "List every todo with its id.",
    args: [],
    returns: 'newline-separated "#<id> <text>" lines, or "(no todos)"',
    run: function () {
      var items = readItems();
      if (items.length === 0) return "(no todos)";
      return items.map(function (it) {
        return "#" + it.id + " " + it.text;
      }).join("\n");
    },
  },

  done: {
    summary: "Remove a todo by its id.",
    args: [{ name: "id", required: true, summary: "the #id shown by `list`" }],
    returns: 'a confirmation line, e.g. "done #1"',
    run: function (args, usage) {
      var id = parseInt(args[0], 10);
      if (isNaN(id)) return usage();
      var key = ITEM_PREFIX + id;
      if (kv.get(key) == null) return "no todo #" + id;
      kv.rm(key);
      return "done #" + id;
    },
  },

  items: {
    summary: "The live todos as a JSON array (for the UI).",
    args: [],
    returns: 'a JSON array, e.g. [{"id":1,"text":"buy milk"}]',
    run: function () {
      return JSON.stringify(readItems());
    },
  },
};
