type RandomInput = {
  choices: string[];
};

export async function main(ctx: any, input: RandomInput) {
  const random = ctx.random.next();
  const index = Math.floor(random * input.choices.length);
  const choice = input.choices[index];

  ctx.ui.render({
    type: "Stack",
    testId: "random-root",
    direction: "v",
    children: [
      { type: "Text", testId: "random-title", text: "Seeded Random", variant: "title" },
      { type: "Text", testId: "random-choice", text: `Choice: ${choice}` }
    ]
  });

  return { ok: true, value: { random, index, choice } };
}
