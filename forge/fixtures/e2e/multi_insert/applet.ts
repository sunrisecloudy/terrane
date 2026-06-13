type Item = {
  name: string;
  qty: number;
};

type MultiInsertInput = {
  items: Item[];
};

export async function main(ctx: any, input: MultiInsertInput) {
  const ids: string[] = [];
  for (const item of input.items) {
    ids.push(await ctx.db.insert("inventory", item));
  }

  const rows = await ctx.db.list("inventory");
  ctx.ui.render({
    type: "Stack",
    testId: "inventory-root",
    direction: "v",
    gap: "xs",
    children: [
      { type: "Text", testId: "inventory-title", text: "Inventory", variant: "title" },
      {
        type: "List",
        testId: "inventory-list",
        items: rows.map((row: any) => ({
          type: "Text",
          testId: `inventory-${row.name}`,
          text: `${row.name}: ${row.qty}`
        }))
      }
    ]
  });

  return { ok: true, value: { ids, count: rows.length } };
}
