export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const make = globalThis["Function"]("return 1");
  return make();
}
