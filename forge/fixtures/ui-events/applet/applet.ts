// Interactive UI event-dispatch applet (prd-merged/05 UI-4, prd-merged/01 CR-6).
//
// The keystone interactive loop, in applet form: `main` renders the initial view;
// each EXPORTED named function is a UI event handler addressable by its name — the
// `ActionRef` carried by a rendered control's `onTap` / `onChange`. The renderer
// sends that ActionRef back with an event payload, and the runtime dispatches the
// matching handler over the same sandbox/limits/host path as a normal run.
//
// State persists ONLY through `ctx.storage` (NOT an in-memory global): the realm
// is one-shot per dispatch, so each handler reads the current value, mutates it,
// and writes it back. This is what makes the loop deterministic and replayable —
// re-dispatching the same event sequence reproduces the exact trace + final tree.

type CounterState = number;
type IncrementEvent = { by?: number }; // onTap payload
type LabelEvent = { value?: string }; // onChange payload

async function readCount(ctx: any): Promise<CounterState> {
  const raw = await ctx.storage.get("app/count");
  return raw === null ? 0 : Number(raw);
}

async function readLabel(ctx: any): Promise<string> {
  const raw = await ctx.storage.get("app/label");
  return raw === null ? "" : String(raw);
}

// The view binds each control's ActionRef to a handler NAME below:
//   Button.onTap   -> "increment" / "decrement"
//   TextField.onChange -> "setLabel"
function view(count: CounterState, label: string) {
  return {
    type: "Stack",
    testId: "root",
    direction: "v",
    gap: "sm",
    children: [
      { type: "Text", testId: "value", text: `Count: ${count}` },
      { type: "Text", testId: "label", text: `Label: ${label}` },
      { type: "Button", testId: "inc", label: "+", onTap: "increment" },
      { type: "Button", testId: "dec", label: "-", onTap: "decrement" },
      { type: "TextField", testId: "name", value: label, label: "Name", onChange: "setLabel" },
    ],
  };
}

export async function main(ctx: any, _input: unknown) {
  const count = await readCount(ctx);
  const label = await readLabel(ctx);
  ctx.ui.render(view(count, label));
  return { ok: true, value: { count, label } };
}

// onTap handler addressed by ActionRef "increment".
export async function increment(ctx: any, event: IncrementEvent) {
  const next = (await readCount(ctx)) + (event.by ?? 1);
  await ctx.storage.set("app/count", String(next));
  const label = await readLabel(ctx);
  ctx.ui.render(view(next, label));
  return { ok: true, value: view(next, label) };
}

// onTap handler addressed by ActionRef "decrement".
export async function decrement(ctx: any, event: IncrementEvent) {
  const next = (await readCount(ctx)) - (event.by ?? 1);
  await ctx.storage.set("app/count", String(next));
  const label = await readLabel(ctx);
  ctx.ui.render(view(next, label));
  return { ok: true, value: view(next, label) };
}

// onChange handler addressed by ActionRef "setLabel".
export async function setLabel(ctx: any, event: LabelEvent) {
  const label = event.value ?? "";
  await ctx.storage.set("app/label", label);
  const count = await readCount(ctx);
  ctx.ui.render(view(count, label));
  return { ok: true, value: view(count, label) };
}
