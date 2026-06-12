export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const mathlib = { evaluate: (value: number) => value + 1 };
  return mathlib.evaluate(41);
}
