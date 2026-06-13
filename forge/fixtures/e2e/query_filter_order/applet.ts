type Task = {
  title: string;
  priority: number;
  date: string;
};

export async function main(ctx: any, input: { minPriority: number }) {
  const tasks: Task[] = [
    { title: "Ship spine", priority: 3, date: "2026-06-10" },
    { title: "Polish docs", priority: 1, date: "2026-06-12" },
    { title: "Fix replay", priority: 5, date: "2026-06-11" },
    { title: "Review grants", priority: 4, date: "2026-06-13" }
  ];

  for (const task of tasks) {
    await ctx.db.insert("tasks", task);
  }

  const rows = await ctx.db.query({
    from: "tasks",
    where: ["priority", ">", input.minPriority],
    orderBy: ["date", "desc"]
  });
  const queryRows = rows.map((row: any) => ({
    title: row.title,
    priority: row.priority,
    date: row.date
  }));

  ctx.ui.render({
    type: "Stack",
    testId: "query-filter-order-root",
    direction: "v",
    gap: "xs",
    children: [
      { type: "Text", testId: "query-title", text: "Priority Tasks", variant: "title" },
      {
        type: "List",
        testId: "query-results",
        items: queryRows.map((row: any) => ({
          type: "Text",
          testId: `task-${row.title}`,
          text: `${row.date}: ${row.title}`
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
