// <my-app> backend for Terrane.
//
// One kv key per fact, so each mutation is exactly one recorded kv.* event —
// Option-A replay rebuilds state by folding those events, never by re-running
// this JS:
//
//   seq        -> highest id ever allocated, as a decimal string
//   note:<id>  -> the note text for that id
//
// The runtime synthesizes `handle`, `__actions__`, usage(), and unknown-verb
// help from the `actions` table, plus scaffold defaults for the required
// common verbs (common.receive / common.list / common.get) — override the
// common.* ones when the app has real items, as below.

var SEQ_KEY = "seq";
var NOTE_PREFIX = "note:";

// Resources are default-deny: a manifest entry only *requests* a namespace.
// Until an admin grants it, ctx.resource.kv is absent — feature-detect, never
// assume. Absent → degrade with a plain string, don't throw.
function kvOrNull() {
  return (ctx.resource && ctx.resource.kv) || null;
}

function readSeq(kv) {
  var raw = kv.get(SEQ_KEY);
  if (raw == null) return 0;
  var n = parseInt(raw, 10);
  return isNaN(n) || n < 0 ? 0 : n;
}

function readNotes(kv) {
  var all = kv.all();
  var notes = [];
  for (var key in all) {
    if (!Object.prototype.hasOwnProperty.call(all, key)) continue;
    if (key.indexOf(NOTE_PREFIX) !== 0) continue;
    var id = parseInt(key.slice(NOTE_PREFIX.length), 10);
    if (isNaN(id)) continue;
    notes.push({ id: id, text: all[key] });
  }
  notes.sort(function (a, b) { return a.id - b.id; });
  return notes;
}

var description = "A minimal kv-backed notes app (template).";

var actions = {
  add: {
    summary: "Add a note.",
    args: [{ name: "text", required: true, summary: "the note text" }],
    returns: 'a confirmation line, e.g. "added #1 hello"',
    run: function (args, usage) {
      var kv = kvOrNull();
      if (!kv) return "kv not granted yet";
      var text = args.join(" ").trim();
      if (text === "") return usage();
      var id = readSeq(kv) + 1;
      kv.set(SEQ_KEY, String(id));
      kv.set(NOTE_PREFIX + id, text);
      return "added #" + id + " " + text;
    },
  },

  rm: {
    summary: "Remove a note by its id.",
    args: [{ name: "id", required: true, summary: "the #id shown by `list`" }],
    returns: 'a confirmation line, e.g. "removed #1"',
    run: function (args, usage) {
      var kv = kvOrNull();
      if (!kv) return "kv not granted yet";
      var id = parseInt(args[0], 10);
      if (isNaN(id)) return usage();
      var key = NOTE_PREFIX + id;
      if (kv.get(key) == null) return "no note #" + id;
      kv.rm(key);
      return "removed #" + id;
    },
  },

  list: {
    summary: "List every note with its id.",
    args: [],
    returns: 'newline-separated "#<id> <text>" lines, or "(no notes)"',
    run: function () {
      var kv = kvOrNull();
      if (!kv) return "kv not granted yet";
      var notes = readNotes(kv);
      if (notes.length === 0) return "(no notes)";
      return notes.map(function (n) { return "#" + n.id + " " + n.text; }).join("\n");
    },
  },

  items: {
    summary: "The live notes as a JSON array (for the UI).",
    args: [],
    returns: 'a JSON array, e.g. [{"id":1,"text":"hello"}]',
    run: function () {
      var kv = kvOrNull();
      if (!kv) return "[]";
      return JSON.stringify(readNotes(kv));
    },
  },

  // Required items interface — every note is addressable as
  // terrane://app/<appId>/item/<id> and resolvable via common.get.
  "common.list": {
    summary: "List notes as addressable items.",
    args: [{ name: "filterJson", required: false }],
    returns: "a JSON array of {id,title,kind}",
    run: function () {
      var kv = kvOrNull();
      if (!kv) return "[]";
      return JSON.stringify(readNotes(kv).map(function (n) {
        return { id: String(n.id), title: n.text, kind: "note" };
      }));
    },
  },

  "common.get": {
    summary: "Read one note as an addressable item.",
    args: [{ name: "id", required: true }],
    returns: "note JSON or typed not-found JSON",
    run: function (args) {
      var kv = kvOrNull();
      var id = parseInt(args[0], 10);
      var raw = kv && !isNaN(id) ? kv.get(NOTE_PREFIX + id) : null;
      if (raw == null) {
        return JSON.stringify({ ok: false, error: { code: "NotFound", id: String(args[0] || "") } });
      }
      return JSON.stringify({ id: String(id), title: raw, kind: "note", text: raw });
    },
  },
};
