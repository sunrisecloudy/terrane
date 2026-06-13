// notes-lite: the M0a end-to-end spine demo applet.
//
// prd-merged/09 M0a exit (the executable spine) + prd-merged/06 PS-5 (the CLI
// harness drives this). It exercises every link of the jewel:
//   TS -> SWC -> QuickJS -> Rust capability ctx -> SQLite write -> UI tree patch
//   -> deterministic replay, all offline.
//
// In main(ctx, input) it:
//   * inserts one note record into the `notes` collection (title from input),
//   * lists the notes back from the same collection (the SQLite read-back),
//   * renders a vertical Stack with a Text header + a List of note titles,
// and returns { ok: true, value: { count } }. It touches only ctx.db / ctx.ui /
// ctx.time, so it stays deterministic (no wall-clock, no network, no random).

import type { AppContext, AppResult } from "@forge/std";

type NotesInput = {
  title?: string;
};

export async function main(ctx: AppContext, input: NotesInput): Promise<AppResult> {
  const title = input.title ?? "Untitled";
  const createdAt = ctx.time.now();

  // SQLite write: one note record into the `notes` collection.
  await ctx.db.insert("notes", { title, createdAt });

  // Read the notes back so the rendered list reflects committed state.
  const notes = await ctx.db.list("notes");

  // Declarative UI tree: a header + a list of the note titles. The host diffs
  // this against the previous render and emits a UI patch (the tree-patch link).
  ctx.ui.render({
    type: "Stack",
    testId: "notes-lite-root",
    direction: "v",
    gap: "sm",
    children: [
      { type: "Text", testId: "notes-title", text: "Notes", variant: "title" },
      {
        type: "List",
        testId: "notes-list",
        items: notes.map((note: any) => ({
          type: "Text",
          testId: `note-${note.title}`,
          text: String(note.title)
        }))
      }
    ]
  });

  return { ok: true, value: { count: notes.length } };
}
