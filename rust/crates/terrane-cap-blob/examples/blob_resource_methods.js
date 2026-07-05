function handle() {
  var data = "aGVsbG8=";
  var hash = ctx.resource.blob.put("attachments/hello.txt", data, "text/plain");
  var stat = JSON.parse(ctx.resource.blob.stat("attachments/hello.txt"));
  var list = JSON.parse(ctx.resource.blob.list("attachments/"));
  var bytes = ctx.resource.blob.get("attachments/hello.txt");
  return JSON.stringify({ hash: hash, stat: stat, count: list.length, bytes: bytes });
}
