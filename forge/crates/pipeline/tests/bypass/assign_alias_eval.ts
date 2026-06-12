export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  let e: (s: string) => unknown;
  e = eval;
  return e("1 + 1");
}
