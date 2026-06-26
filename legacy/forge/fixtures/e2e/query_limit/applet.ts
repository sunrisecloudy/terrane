type QueueItem = {
  title: string;
  score: number;
};

export async function main(ctx: any, input: { limit: number }) {
  const items: QueueItem[] = [
    { title: "Alpha", score: 30 },
    { title: "Beta", score: 10 },
    { title: "Gamma", score: 20 },
    { title: "Delta", score: 40 }
  ];

  for (const item of items) {
    await ctx.db.insert("queue", item);
  }

  const rows = await ctx.db.query("queue", {
    from: "queue",
    orderBy: ["score", "asc"],
    limit: input.limit
  });
  const queryRows = rows.map((row: any) => ({
    title: row.title,
    score: row.score
  }));

  ctx.ui.render({
    type: "Stack",
    testId: "query-limit-root",
    direction: "v",
    gap: "xs",
    children: [
      { type: "Text", testId: "query-limit-title", text: "Top Queue", variant: "title" },
      {
        type: "List",
        testId: "query-limit-results",
        items: queryRows.map((row: any) => ({
          type: "Text",
          testId: `queue-${row.title}`,
          text: `${row.title}: ${row.score}`
        }))
      }
    ]
  });

  return {
    ok: true,
    value: {
      count: queryRows.length,
      query_rows: queryRows,
      replay_identical: true
    }
  };
}
