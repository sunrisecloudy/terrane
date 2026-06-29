function handle(input) {
  var crdt = ctx.resource.crdt;
  crdt.mapSet("profile", "name", "Ada");
  crdt.listPush("tasks", "draft");
  crdt.listInsert("tasks", "1", "review");
  crdt.textInsert("note", "0", "hello");

  return JSON.stringify({
    name: crdt.mapGet("profile", "name"),
    profile: crdt.mapAll("profile"),
    tasks: crdt.listAll("tasks"),
    note: crdt.textGet("note")
  });
}
