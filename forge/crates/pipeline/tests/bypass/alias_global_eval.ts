export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const g = globalThis as Record<string, any>;
  return g.eval("1 + 1");
}
