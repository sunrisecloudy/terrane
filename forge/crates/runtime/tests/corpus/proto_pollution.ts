export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const target: Record<string, unknown> = {};
  target.__proto__ = { polluted: true };
  return target;
}
