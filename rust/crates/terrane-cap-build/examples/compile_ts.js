function handle(input) {
  var result = ctx.resource.build.compileTs(
    "widget.tsx",
    "export const label: string = <button>Save</button>;"
  );
  var parsed = JSON.parse(result);
  return parsed.ok ? parsed.code : parsed.error;
}
