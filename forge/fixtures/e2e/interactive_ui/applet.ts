// Interactive UI event-dispatch applet (prd-merged/05 UI-4, prd-merged/01 CR-6).
//
// The keystone interactive loop, in applet form, driven through the facade by the
// `ui.dispatch_event` command. `main` renders the initial view; each EXPORTED
// named function is a UI event handler addressable by its name — the `ActionRef`
// that a rendered control's `onTap` / `onChange` carries (the T034 dispatch key).
// The renderer sends that ActionRef back with an event payload and the runtime
// re-enters the matching handler in a fresh, one-shot, capability-gated realm.
//
// State persists ONLY through `ctx.storage` / `ctx.db` — never an in-memory global.
// The realm is one-shot per dispatch, so each handler READS the current state,
// MUTATES it, WRITES it back, and re-renders. That is exactly what makes the loop
// deterministic and replayable: re-dispatching the same event sequence reproduces
// the same trace + final tree byte-for-byte. The handler returns its new UI tree
// as its `value`; the facade diffs that tree against the last-known tree to the
// next UI patch.
//
// Two state families are demonstrated:
//   * a counter in `ctx.storage` (the `+` / `-` Buttons → onTap), and
//   * a label in `ctx.storage` mirrored as a `notes` record via `ctx.db` (the
//     TextField → onChange, and the "Save note" Button → onTap), so the fixture
//     exercises BOTH host effect kinds inside event handlers.

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
//   Button.onTap       -> "increment" / "decrement" / "saveNote"
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
      { type: "Button", testId: "save", label: "Save note", onTap: "saveNote" },
    ],
  };
}

export async function main(ctx: any, _input: unknown) {
  const count = await readCount(ctx);
  const label = await readLabel(ctx);
  ctx.ui.render(view(count, label));
  return { ok: true, value: view(count, label) };
}

// onTap handler addressed by ActionRef "increment". Reads → mutates → writes →
// re-renders. The realm is one-shot, so the new count is read back from storage.
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

// onChange handler addressed by ActionRef "setLabel". A non-string value is a
// typed validation error (the payload contract for a TextField change is a
// string), so an invalid payload is a clean rejection with the prior view intact.
export async function setLabel(ctx: any, event: LabelEvent) {
  if (typeof event.value !== "string") {
    throw new Error("invalid event payload: value must be a string");
  }
  const label = event.value;
  await ctx.storage.set("app/label", label);
  const count = await readCount(ctx);
  ctx.ui.render(view(count, label));
  return { ok: true, value: view(count, label) };
}

// onTap handler addressed by ActionRef "saveNote". Persists the current label as
// a `notes` record via ctx.db (the db-write-before-render path), then re-renders.
export async function saveNote(ctx: any, _event: unknown) {
  const label = await readLabel(ctx);
  await ctx.db.insert("notes", { text: label, done: false });
  const count = await readCount(ctx);
  ctx.ui.render(view(count, label));
  return { ok: true, value: view(count, label) };
}
