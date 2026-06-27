// Collaborative todo backend — CLI-only (apps/todo-cli-collaborate).
//
// Todos live in one Loro **List** CRDT container ("todos"), so two replicas that
// add items offline MERGE on sync with no lost writes — where a kv `seq` counter
// would have both reuse id #1 and clobber each other.
//
// The app is one ACTIONS table: each action keeps its description AND its handler
// together. Everything else (dispatch, the `__actions__` self-description, the
// "unknown verb" help) is derived from that table, so there's a single source of
// truth — add an entry and it's both runnable and discoverable.

var crdt = ctx.resource.crdt;
var LIST = "todos";

// The todos, in order, as an array of strings.
function items() {
  return crdt.listAll(LIST);
}

var META = {
  app: "todo-cli-collaborate",
  title: "Collaborative Todo",
  description: "A CRDT-backed todo list. Items merge across replicas with no lost writes."
};

var ACTIONS = {
  add: {
    summary: "Add a todo item.",
    args: [{ name: "text", required: true, summary: "the item text (may be several words)" }],
    returns: 'a confirmation line, e.g. "added: buy milk"',
    run: function (rest) {
      var text = rest.join(" ").trim();
      if (text === "") return usage("add");
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
    run: function (rest) {
      var n = parseInt(rest[0], 10);
      if (isNaN(n) || n < 1) return usage("done");
      var all = items();
      if (n > all.length) return "no todo #" + n;
      var text = all[n - 1];
      // crdt indices are 0-based and passed as strings (args go through verbatim).
      crdt.listDel(LIST, String(n - 1));
      return "done #" + n + " " + text;
    }
  }
};

// --- generic glue: everything below is derived from ACTIONS + META -----------

// A usage line for one action, built from its declared args.
function usage(verb) {
  var slots = ACTIONS[verb].args.map(function (a) {
    return a.required ? "<" + a.name + ">" : "[" + a.name + "]";
  });
  return "usage: " + verb + (slots.length ? " " + slots.join(" ") : "");
}

// The machine-readable self-description (the MCP `app_actions` tool reads this).
// Derived from ACTIONS, so it can never drift from what `handle` dispatches.
function describe() {
  var actions = Object.keys(ACTIONS).map(function (verb) {
    var a = ACTIONS[verb];
    return { verb: verb, summary: a.summary, args: a.args, returns: a.returns };
  });
  return JSON.stringify({
    app: META.app,
    title: META.title,
    description: META.description,
    actions: actions
  });
}

// Entry point. `input` is the verb's argument array, e.g. ["add","buy","milk"].
function handle(input) {
  var args = input || [];
  var verb = args[0] || "";
  if (verb === "__actions__") return describe();

  var action = ACTIONS[verb];
  if (!action) {
    return "unknown verb: " + verb + " (try " + Object.keys(ACTIONS).join(" | ") + ")";
  }
  return action.run(args.slice(1));
}
