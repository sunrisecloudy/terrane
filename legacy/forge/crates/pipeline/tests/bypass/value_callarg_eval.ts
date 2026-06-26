function doThing(f: (s: string) => unknown): unknown {
  return f("1 + 1");
}

export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  return doThing(eval);
}
