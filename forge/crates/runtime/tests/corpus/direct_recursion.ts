function recurse(depth: number): number {
  return recurse(depth + 1);
}

export async function main(_ctx: unknown, _input: unknown): Promise<number> {
  return recurse(0);
}
