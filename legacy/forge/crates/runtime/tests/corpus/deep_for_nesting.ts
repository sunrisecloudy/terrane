export async function main(_ctx: unknown, _input: unknown): Promise<number> {
  let total = 0;
  for (let a = 0; a < 1_000_000; a++) {
    for (let b = 0; b < 1_000_000; b++) {
      total += a + b;
    }
  }
  return total;
}
