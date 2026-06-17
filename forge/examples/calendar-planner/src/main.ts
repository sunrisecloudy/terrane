import type { AppContext, AppResult } from "@forge/std";

type CalendarInput = {
  title?: string;
  date?: string;
  start?: string;
  durationMinutes?: number;
  notes?: string;
};

export async function main(ctx: AppContext, input: CalendarInput): Promise<AppResult> {
  const title = input.title ?? "Planning session";
  const date = input.date ?? "2026-06-16";
  const start = input.start ?? "09:00";
  const durationMinutes = input.durationMinutes ?? 60;
  const notes = input.notes ?? "";
  const createdAt = ctx.time.now();

  const event = await ctx.db.insert("calendar_events", {
    title,
    date,
    start,
    durationMinutes,
    notes,
    createdAt
  });
  await ctx.storage.set("app/last-calendar-event-id", String(event.id));

  const dayEvents = await ctx.db.query("calendar_events", {
    from: "calendar_events",
    where: ["date", "=", date],
    orderBy: [{ field: "start", dir: "asc" }]
  });

  ctx.ui.render({
    type: "Stack",
    testId: "calendar-planner-root",
    direction: "v",
    gap: "sm",
    children: [
      { type: "Text", testId: "calendar-planner-title", text: "Calendar Planner", variant: "title" },
      { type: "Text", testId: "calendar-planner-date", text: date },
      {
        type: "List",
        testId: "calendar-planner-list",
        items: dayEvents.map((row: any) => ({
          type: "Text",
          testId: `calendar-event-${row.id ?? row.title}`,
          text: `${row.start ?? row.fields?.start} ${row.title ?? row.fields?.title}`
        }))
      }
    ]
  });

  return { ok: true, value: { inserted: event.id, visible: dayEvents.length } };
}
