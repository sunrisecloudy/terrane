export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const make = new Function("return globalThis");
  return make();
}
