export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const { eval: e } = globalThis as Record<string, any>;
  return e("1 + 1");
}
