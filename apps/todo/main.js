// Todo backend for Terrane.
//
// The host loads this file into QuickJS once per `terrane host run todo …`,
// exposes a single global `ctx` whose `ctx.resource.kv` methods are
// app-scoped and synchronous, then calls `handle(input)` where `input` is the
// verb's argument array. Whatever string `handle` returns is printed by the
// host.
//
// Storage layout (one kv key per fact, so each mutation is exactly one
// recorded kv.* event — Option-A replay rebuilds todos by folding those
// events, never by re-running this JS):
//
//   seq        -> highest id ever allocated, as a decimal string
//   item:<id>  -> the todo text for that id
//
// Removing a todo (`done`) deletes its item:<id> key. Ids are stable and
// monotonic, so `list` ordering and `done <id>` stay meaningful across runs.

var kv = ctx.resource.kv;

var SEQ_KEY = "seq";
var ITEM_PREFIX = "item:";

// Read the id counter (0 if unset). Stored as a string; parse defensively.
function readSeq() {
  var raw = kv.get(SEQ_KEY);
  if (raw == null) {
    // missing key reads back as null/undefined
    return 0;
  }
  var n = parseInt(raw, 10);
  if (isNaN(n) || n < 0) {
    return 0;
  }
  return n;
}

// Collect every live todo as [{ id: number, text: string }, …], sorted by id.
function readItems() {
  var all = kv.all();
  var items = [];
  for (var key in all) {
    if (!Object.prototype.hasOwnProperty.call(all, key)) {
      continue;
    }
    if (key.indexOf(ITEM_PREFIX) !== 0) {
      continue;
    }
    var id = parseInt(key.slice(ITEM_PREFIX.length), 10);
    if (isNaN(id)) {
      continue;
    }
    items.push({ id: id, text: all[key] });
  }
  items.sort(function (a, b) {
    return a.id - b.id;
  });
  return items;
}

function add(args) {
  var text = args.join(" ").trim();
  if (text === "") {
    return "usage: todo add <text…>";
  }
  var id = readSeq() + 1;
  kv.set(SEQ_KEY, String(id));
  kv.set(ITEM_PREFIX + id, text);
  return "added #" + id + " " + text;
}

function list() {
  var items = readItems();
  if (items.length === 0) {
    return "(no todos)";
  }
  var lines = [];
  for (var i = 0; i < items.length; i++) {
    lines.push("#" + items[i].id + " " + items[i].text);
  }
  return lines.join("\n");
}

function done(args) {
  var raw = args.length > 0 ? args[0] : "";
  var id = parseInt(raw, 10);
  if (isNaN(id)) {
    return "usage: todo done <id>";
  }
  var key = ITEM_PREFIX + id;
  if (kv.get(key) == null) {
    return "no todo #" + id;
  }
  kv.rm(key);
  return "done #" + id;
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
    default:
      return "unknown verb: " + verb + " (try add | list | done)";
  }
}
