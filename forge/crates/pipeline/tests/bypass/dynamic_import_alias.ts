export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const i = import;
  return i("./escape.ts");
}
