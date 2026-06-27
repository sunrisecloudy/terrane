// Collaborative todo backend — CLI-only (apps/todo-cli-collaborate).
//
// Same verbs as apps/todo-cli (`add` / `list` / `done`), but stored in a CRDT
// instead of kv. The todos live in one Loro **List** container named "todos".
// Because it's a CRDT, two replicas that add items offline MERGE on sync with no
// lost writes — where the kv version (a single `seq` counter + last-writer-wins)
// would have both replicas reuse id #1 and clobber each other.
//
//   ctx.resource.crdt.listPush("todos", text)   append a todo (collaborative)
//   ctx.resource.crdt.listAll("todos")           -> ordered array of texts
//   ctx.resource.crdt.listDel("todos", index)    remove the item at index

var crdt = ctx.resource.crdt;

var LIST = "todos";

// Every todo, in order, as an array of strings.
function items() {
  return crdt.listAll(LIST);
}

function add(args) {
  var text = args.join(" ").trim();
  if (text === "") {
    return "usage: todo add <text…>";
  }
  crdt.listPush(LIST, text);
  return "added: " + text;
}

function list() {
  var all = items();
  if (all.length === 0) {
    return "(no todos)";
  }
  var lines = [];
  for (var i = 0; i < all.length; i++) {
    // 1-based numbering for humans; `done` takes the same number.
    lines.push("#" + (i + 1) + " " + all[i]);
  }
  return lines.join("\n");
}

function done(args) {
  var raw = args.length > 0 ? args[0] : "";
  var n = parseInt(raw, 10);
  if (isNaN(n) || n < 1) {
    return "usage: todo done <number>";
  }
  var all = items();
  if (n > all.length) {
    return "no todo #" + n;
  }
  var text = all[n - 1];
  // crdt indices are 0-based and passed as strings (the runtime hands JS args
  // through verbatim).
  crdt.listDel(LIST, String(n - 1));
  return "done #" + n + " " + text;
}

// Self-description for hosts/agents (e.g. the MCP `app_actions` tool). The
// reserved `__actions__` verb returns this app's actions as machine-readable
// JSON, so a caller can discover what it can do before invoking anything.
function actions() {
  return JSON.stringify({
    app: "todo-cli-collaborate",
    title: "Collaborative Todo",
    description:
      "A CRDT-backed todo list. Items merge across replicas with no lost writes.",
    actions: [
      {
        verb: "add",
        summary: "Add a todo item.",
        args: [{ name: "text", required: true, summary: "the item text (may be several words)" }],
        returns: "a confirmation line, e.g. \"added: buy milk\""
      },
      {
        verb: "list",
        summary: "List every todo with its 1-based number.",
        args: [],
        returns: "newline-separated \"#<n> <text>\" lines, or \"(no todos)\""
      },
      {
        verb: "done",
        summary: "Remove a todo by its number.",
        args: [{ name: "number", required: true, summary: "the 1-based number shown by `list`" }],
        returns: "a confirmation line, e.g. \"done #1 buy milk\""
      }
    ]
  });
}

// Entry point. `input` is the verb's argument array, e.g. ["add","buy","milk"].
function handle(input) {
  var args = input || [];
  var verb = args.length > 0 ? args[0] : "";
  var rest = args.slice(1);
  switch (verb) {
    case "add":
      return add(rest);
    case "list":
      return list();
    case "done":
      return done(rest);
    case "__actions__":
      return actions();
    default:
      return "unknown verb: " + verb + " (try add | list | done)";
  }
}
