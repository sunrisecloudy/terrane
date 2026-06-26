export async function main(ctx: any, input: { message: string }) {
  await ctx.db.insert("audit_log", {
    message: input.message,
    level: "info"
  });

  ctx.ui.render({
    type: "Text",
    testId: "should-not-render",
    text: "This should not render"
  });

  return { ok: true, value: { inserted: true } };
}
