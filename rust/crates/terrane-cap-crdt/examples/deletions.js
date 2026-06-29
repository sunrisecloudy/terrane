function handle(input) {
  var crdt = ctx.resource.crdt;
  crdt.mapSet("profile", "status", "draft");
  crdt.mapDel("profile", "status");
  crdt.listPush("tasks", "archive me");
  crdt.listDel("tasks", "0");
  crdt.textInsert("note", "0", "stale");
  crdt.textDel("note", "0", "5");
  return JSON.stringify(crdt.mapAll("profile"));
}
