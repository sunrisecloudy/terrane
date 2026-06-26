export async function main(ctx: any, _input: unknown) {
  const first = ctx.time.now();
  const second = ctx.time.now();
  const monotone = second > first;

  ctx.ui.render({
    type: "Stack",
    testId: "time-log-root",
    direction: "v",
    gap: "xs",
    children: [
      { type: "Text", testId: "time-title", text: "Time Log", variant: "title" },
      { type: "Text", testId: "time-first", text: `First: ${first}` },
      { type: "Text", testId: "time-second", text: `Second: ${second}` }
    ]
  });

  return { ok: true, value: { first, second, monotone } };
}
