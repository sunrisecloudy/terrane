export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  // `eval` is a property of a plain user object, not the global container,
  // so destructuring it captures a benign local, not the real evaluator.
  const handlers = { eval: (x: number) => x + 1 };
  const { eval: run } = handlers;
  return run(41);
}
