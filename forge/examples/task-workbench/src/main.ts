import type { AppContext, AppResult } from "@forge/std";

type TaskInput = {
  title?: string;
  status?: string;
};

export async function main(ctx: AppContext, input: TaskInput): Promise<AppResult> {
  const title = input.title ?? "Review Forge migration";
  const status = input.status ?? "todo";
  const createdAt = ctx.time.now();

  const task = await ctx.db.insert("tasks", {
    title,
    status,
    createdAt
  });
  await ctx.storage.set("app/last-task-id", String(task.id));

  const openTasks = await ctx.db.query("tasks", {
    from: "tasks",
    where: ["status", "=", status],
    orderBy: [{ field: "createdAt", dir: "asc" }]
  });

  ctx.ui.render({
    type: "Stack",
    testId: "task-workbench-root",
    direction: "v",
    gap: "sm",
    children: [
      { type: "Text", testId: "task-workbench-title", text: "Task Workbench", variant: "title" },
      {
        type: "List",
        testId: "task-workbench-list",
        items: openTasks.map((row: any) => ({
          type: "Text",
          testId: `task-${row.id ?? row.title}`,
          text: `${row.title ?? row.fields?.title} [${row.status ?? row.fields?.status}]`
        }))
      }
    ]
  });

  return { ok: true, value: { inserted: task.id, visible: openTasks.length } };
}
