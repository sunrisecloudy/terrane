function handle(input) {
  var kv = ctx.resource.kv;
  kv.set("todo:1", "buy milk");
  kv.set("todo:2", "ship docs");

  var first = kv.get("todo:1");
  var todos = kv.scan("todo:", "25");
  var keys = kv.keys("todo:", "25");
  var ordered = kv.range("todo:1", "todo:3", "25");

  kv.rm("todo:1");
  return JSON.stringify({
    first: first,
    todos: todos,
    keys: keys,
    ordered: ordered,
    remaining: kv.all()
  });
}
