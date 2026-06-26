export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  (globalThis as Record<string, unknown>).__forgeEscape = true;
  return globalThis;
}
