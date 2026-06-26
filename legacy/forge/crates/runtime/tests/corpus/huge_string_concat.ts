export async function main(_ctx: unknown, _input: unknown): Promise<number> {
  let value = "x";
  while (true) {
    value += value;
  }
}
