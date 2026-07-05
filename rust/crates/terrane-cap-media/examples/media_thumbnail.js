function handle(verb, name) {
  var blobName = name || "photos/original.png";
  if (verb === "inspect") {
    return ctx.resource.media.info(blobName);
  }
  var info = JSON.parse(ctx.resource.media.info(blobName));
  var ops = JSON.stringify([{ op: "thumbnail", size: 256 }]);
  return JSON.stringify({
    source: info,
    command: "media.transform",
    sourceName: blobName,
    ops: ops,
    destName: "__thumb__/" + blobName
  });
}
