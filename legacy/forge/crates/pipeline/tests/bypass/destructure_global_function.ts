export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const { Function: F } = globalThis as Record<string, any>;
  return new F("return 1");
}
