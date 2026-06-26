export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  return (globalThis as Record<string, any>)[`eval`]("1 + 1");
}
