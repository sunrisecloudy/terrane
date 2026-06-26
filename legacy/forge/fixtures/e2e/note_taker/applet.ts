type NoteInput = {
  title: string;
  body: string;
  tags?: string[];
};

export async function main(ctx: any, input: NoteInput) {
  const createdAt = ctx.time.now();
  const record = {
    title: input.title,
    body: input.body,
    tags: input.tags ?? [],
    createdAt
  };

  const id = await ctx.db.insert("notes", record);
  const notes = await ctx.db.list("notes");

  ctx.ui.render({
    type: "Stack",
    testId: "note-taker-root",
    direction: "v",
    gap: "sm",
    children: [
      { type: "Text", testId: "title", text: "My Notes", variant: "title" },
      {
        type: "List",
        testId: "note-list",
        items: notes.map((note: any) => ({
          type: "Text",
          testId: `note-${note.title}`,
          text: `${note.title}: ${note.body}`
        }))
      }
    ]
  });

  return { ok: true, value: { inserted: id, count: notes.length, createdAt } };
}
