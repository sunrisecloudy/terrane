import type { AppContext, AppResult } from "@forge/std";

type ReplayInput = {
  event?: string;
};

export async function main(ctx: AppContext, input: ReplayInput): Promise<AppResult> {
  const event = input.event ?? "demo.run";
  const logicalTime = ctx.time.now();
  const sample = ctx.random.next();

  await ctx.storage.set("app/last-event", event);
  await ctx.db.insert("replay_events", {
    event,
    logicalTime,
    sample
  });
  const events = await ctx.db.list("replay_events");

  ctx.ui.render({
    type: "Stack",
    testId: "core-replay-lab-root",
    direction: "v",
    gap: "sm",
    children: [
      { type: "Text", testId: "core-replay-lab-title", text: "Core Replay Lab", variant: "title" },
      {
        type: "List",
        testId: "core-replay-lab-events",
        items: events.map((row: any) => ({
          type: "Text",
          testId: `replay-event-${row.event ?? row.fields?.event}`,
          text: `${row.event ?? row.fields?.event} @ ${row.logicalTime ?? row.fields?.logicalTime}`
        }))
      }
    ]
  });

  return { ok: true, value: { event, count: events.length, sample } };
}
