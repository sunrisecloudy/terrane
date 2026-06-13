type CounterInput = {
  key?: string;
};

export async function main(ctx: any, input: CounterInput) {
  const key = input.key ?? "app/counter";
  const previousRaw = await ctx.storage.get(key);
  const previous = previousRaw === null ? 0 : Number(previousRaw);
  const next = previous + 1;

  await ctx.storage.set(key, String(next));
  const keys = await ctx.storage.list("app/");

  ctx.ui.render({
    type: "Stack",
    testId: "counter-root",
    direction: "v",
    gap: "xs",
    children: [
      { type: "Text", testId: "counter-title", text: "Counter", variant: "title" },
      { type: "Text", testId: "counter-value", text: `Count: ${next}` },
      { type: "Text", testId: "counter-keys", text: `Keys: ${keys.join(",")}` }
    ]
  });

  return { ok: true, value: { key, previous, next, keys } };
}
