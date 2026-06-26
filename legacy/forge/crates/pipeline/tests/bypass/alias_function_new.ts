export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  const F = Function;
  const make = new F("return 1");
  return make();
}
