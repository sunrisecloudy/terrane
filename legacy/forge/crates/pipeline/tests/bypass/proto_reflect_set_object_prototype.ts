export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  Reflect.set(Object.prototype, "polluted", 1);
  return {};
}
