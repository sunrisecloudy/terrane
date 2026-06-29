function handle(input) {
  ctx.resource.kv.set("settings/theme", input[0] || "dark");
  return ctx.resource.kv.get("settings/theme");
}
