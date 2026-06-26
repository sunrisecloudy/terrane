export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  (Object.prototype as Record<string, unknown>).polluted = true;
  return {};
}
