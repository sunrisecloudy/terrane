// <my-app> backend for Terrane.
//
// One kv key per fact, so each stored fact has its own recorded kv.* event.
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

function kvGetOrNull(kv, key) {
  try {
    return kv.get(key);
  } catch (err) {
    if (String(err).indexOf("not found") !== -1) return null;
    throw err;
  }
}

function kvRmIfPresent(kv, key) {
  try {
    kv.rm(key);
    return true;
  } catch (err) {
    if (String(err).indexOf("not found") !== -1) return false;
    throw err;
  }
}

function readSeq(kv) {
  var raw = kvGetOrNull(kv, SEQ_KEY);
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

function addNote(kv, text) {
  var id = readSeq(kv) + 1;
  kv.set(SEQ_KEY, String(id));
  kv.set(NOTE_PREFIX + id, text);
  return id;
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
      var id = addNote(kv, text);
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
      if (!kvRmIfPresent(kv, key)) return "no note #" + id;
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

  "common.receive": {
    summary: "Receive an inbound payload as a note.",
    args: [
      { name: "kind", required: true },
      { name: "payloadJson", required: true },
    ],
    returns: "JSON acknowledgement or resource-unavailable status",
    run: function (args) {
      var kv = kvOrNull();
      if (!kv) {
        return JSON.stringify({
          ok: false,
          error: { code: "ResourceUnavailable", resource: "kv" },
        });
      }
      var kind = String(args[0] || "json");
      var payload = String(args[1] || "");
      var text = payload;
      try {
        var parsed = JSON.parse(payload);
        if (typeof parsed === "string") text = parsed;
        else if (parsed && typeof parsed.text === "string") text = parsed.text;
        else if (parsed && typeof parsed.title === "string") text = parsed.title;
      } catch (_) {}
      if (text.trim() === "") text = "[" + kind + "]";
      var id = addNote(kv, text);
      return JSON.stringify({ ok: true, id: String(id), kind: kind });
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
      var raw = kv && !isNaN(id) ? kvGetOrNull(kv, NOTE_PREFIX + id) : null;
      if (raw == null) {
        return JSON.stringify({ ok: false, error: { code: "NotFound", id: String(args[0] || "") } });
      }
      return JSON.stringify({ id: String(id), title: raw, kind: "note", text: raw });
    },
  },
};
