// Collaborative todo backend — CLI-only (apps/todo-cli-collaborate).
//
// Todos live in one Loro **List** CRDT container ("todos"), so two replicas that
// add items offline MERGE on sync with no lost writes — where a kv `seq` counter
// would have both reuse id #1 and clobber each other.
//
// The app is just a `description` + an `actions` table: each action keeps its
// metadata (summary/args/returns) and its `run(args, usage)` handler together.
// The runtime synthesizes everything else — verb dispatch, the `__actions__`
// self-description (with the id/name from manifest.json), the usage lines, and
// the unknown-verb help — so there is a single source of truth.

var crdt = ctx.resource.crdt;
var LIST = "todos";

// The todos, in order, as an array of strings.
function items() {
  return crdt.listAll(LIST);
}

var description = "A CRDT-backed todo list. Items merge across replicas with no lost writes.";

var actions = {
  add: {
    summary: "Add a todo item.",
    args: [{ name: "text", required: true, summary: "the item text (may be several words)" }],
    returns: 'a confirmation line, e.g. "added: buy milk"',
    run: function (args, usage) {
      var text = args.join(" ").trim();
      if (text === "") return usage();
      crdt.listPush(LIST, text);
      return "added: " + text;
    }
  },

  list: {
    summary: "List every todo with its 1-based number.",
    args: [],
    returns: 'newline-separated "#<n> <text>" lines, or "(no todos)"',
    run: function () {
      var all = items();
      if (all.length === 0) return "(no todos)";
      return all.map(function (text, i) { return "#" + (i + 1) + " " + text; }).join("\n");
    }
  },

  done: {
    summary: "Remove a todo by its number.",
    args: [{ name: "number", required: true, summary: "the 1-based number shown by `list`" }],
    returns: 'a confirmation line, e.g. "done #1 buy milk"',
    run: function (args, usage) {
      var n = parseInt(args[0], 10);
      if (isNaN(n) || n < 1) return usage();
      var all = items();
      if (n > all.length) return "no todo #" + n;
      var text = all[n - 1];
      // crdt indices are 0-based and passed as strings (args go through verbatim).
      crdt.listDel(LIST, String(n - 1));
      return "done #" + n + " " + text;
    }
  }
};
