export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const fns = [eval];
  return fns.map((f) => f("1 + 1"));
}
