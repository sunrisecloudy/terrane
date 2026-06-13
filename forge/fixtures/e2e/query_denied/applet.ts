export async function main(ctx: any, _input: unknown) {
  const rows = await ctx.db.query({
    from: "secrets",
    limit: 1
  });

  return { ok: true, value: { query_rows: rows } };
}
