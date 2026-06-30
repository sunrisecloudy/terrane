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

function handle(input) {
  var kv = ctx.resource.kv;
  var rawIds = kvGetOrNull(kv, "event_ids");
  var ids = rawIds ? JSON.parse(rawIds) : [];

  if (input[0] === "add") {
    var id = "event:" + (ids.length + 1);
    kv.set(id, input.slice(1).join(" "));
    ids.push(id);
    kv.set("event_ids", JSON.stringify(ids));
    return JSON.stringify(ids);
  }

  return JSON.stringify(ids.map(function (id) {
    return { id: id, value: kvGetOrNull(kv, id) };
  }));
}
