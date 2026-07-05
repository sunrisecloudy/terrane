// Todo backend — CLI-only variant (apps/todo-cli).
//
// Same storage model as apps/todo (one kv key per fact, so each mutation is one
// recorded kv.* event), but no UI: just the text verbs a CLI host drives. The app
// is a `description` + an `actions` table; the runtime synthesizes `handle`,
// `__actions__`, usage, and the unknown-verb help from it.
//
//   seq        -> highest id ever allocated, as a decimal string
//   item:<id>  -> the todo text for that id

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

var description = "A simple todo list (kv-backed; last-writer-wins).";

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

  "common.list": {
    summary: "List todos as addressable items.",
    args: [{ name: "filterJson", required: false }],
    returns: "JSON array of {id,title,kind}",
    run: function () {
      return JSON.stringify(readItems().map(function (it) {
        return { id: String(it.id), title: it.text, kind: "todo" };
      }));
    },
  },

  "common.get": {
    summary: "Read one todo as an addressable item.",
    args: [{ name: "id", required: true }],
    returns: "todo JSON or typed not-found JSON",
    run: function (args) {
      var id = parseInt(args[0], 10);
      var raw = isNaN(id) ? null : kv.get(ITEM_PREFIX + id);
      if (raw == null) {
        return JSON.stringify({ ok: false, error: { code: "NotFound", id: String(args[0] || "") } });
      }
      return JSON.stringify({ id: String(id), title: raw, kind: "todo", text: raw });
    },
  },
};
