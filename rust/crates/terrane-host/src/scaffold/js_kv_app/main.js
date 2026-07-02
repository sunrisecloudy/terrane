/* EDIT GUIDE: this backend is a working generic item store. Rename "item" to
   your domain noun, extend the record built in addItem, and add verbs above
   the ADD marker in handle. handle(input) dispatches on input[0]; input is an
   array of strings; always return a string (JSON.stringify for structured
   data). */

/* KEEP: defensive KV helpers — missing keys throw, these normalize that. */
function kvGetOrNull(kv, key) {
  try {
    return kv.get(key);
  } catch (err) {
    if (String(err).indexOf("not found") !== -1) {
      return null;
    }
    throw err;
  }
}

function kvRmIfPresent(kv, key) {
  try {
    kv.rm(key);
  } catch (err) {
    if (String(err).indexOf("not found") === -1) {
      throw err;
    }
  }
}

/* KEEP: index helpers — item ids live in one JSON array under "item_ids". */
function loadIds(kv) {
  var raw = kvGetOrNull(kv, "item_ids");
  if (raw == null) return [];
  try { return JSON.parse(raw); } catch (err) { return []; }
}

function saveIds(kv, ids) {
  kv.set("item_ids", JSON.stringify(ids));
}

function nextId(kv) {
  var raw = kvGetOrNull(kv, "next_id");
  var n = raw == null ? 1 : parseInt(raw, 10) || 1;
  kv.set("next_id", String(n + 1));
  return "item-" + n;
}

/* REPLACE: the stored record. Add the fields your app needs (date, tags, …). */
function addItem(kv, text) {
  var id = nextId(kv);
  var item = { id: id, text: text };
  kv.set("item:" + id, JSON.stringify(item));
  var ids = loadIds(kv);
  ids.push(id);
  saveIds(kv, ids);
  return item;
}

function listItems(kv) {
  var ids = loadIds(kv);
  var items = [];
  for (var i = 0; i < ids.length; i++) {
    var raw = kvGetOrNull(kv, "item:" + ids[i]);
    if (raw == null) continue;
    try { items.push(JSON.parse(raw)); } catch (err) { continue; }
  }
  return items;
}

function removeItem(kv, id) {
  kvRmIfPresent(kv, "item:" + id);
  saveIds(kv, loadIds(kv).filter(function (x) { return x !== id; }));
}

function handle(input) {
  var verb = input[0] || "";
  var kv = ctx.resource.kv;

  if (verb === "__actions__") {
    return JSON.stringify({
      app: __APP_ID_JSON__,
      title: __APP_NAME_JSON__,
      description: "Generated JS kv app over a generic item store.",
      actions: [
        { verb: "add", summary: "Add one item.", args: [{ name: "text", required: true, summary: "item text" }], returns: "the stored item as JSON" },
        { verb: "list", summary: "List all items.", args: [], returns: "JSON array of items" },
        { verb: "get", summary: "Read one item.", args: [{ name: "id", required: true, summary: "item id" }], returns: "item JSON or (missing)" },
        { verb: "remove", summary: "Delete one item.", args: [{ name: "id", required: true, summary: "item id" }], returns: "removed" },
        { verb: "clear", summary: "Delete every item.", args: [], returns: "cleared" }
      ]
    });
  }

  if (verb === "add") {
    var text = input.slice(1).join(" ");
    if (!text) return JSON.stringify({ ok: false, error: "add needs text" });
    return JSON.stringify(addItem(kv, text));
  }

  if (verb === "list") {
    return JSON.stringify(listItems(kv));
  }

  if (verb === "get") {
    var raw = kvGetOrNull(kv, "item:" + (input[1] || ""));
    return raw == null ? "(missing)" : raw;
  }

  if (verb === "remove") {
    removeItem(kv, input[1] || "");
    return "removed";
  }

  if (verb === "clear") {
    var ids = loadIds(kv);
    for (var i = 0; i < ids.length; i++) {
      kvRmIfPresent(kv, "item:" + ids[i]);
    }
    kvRmIfPresent(kv, "item_ids");
    return "cleared";
  }

  /* ADD: more verbs above this line; document them in __actions__ too. */
  return "unknown verb: " + verb;
}
