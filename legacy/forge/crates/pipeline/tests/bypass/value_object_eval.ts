export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const o = { run: eval };
  return o.run("1 + 1");
}
