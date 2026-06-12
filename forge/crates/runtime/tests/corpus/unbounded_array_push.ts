export async function main(_ctx: unknown, _input: unknown): Promise<number> {
  const values: string[] = [];
  while (true) {
    values.push("x".repeat(1024));
  }
}
