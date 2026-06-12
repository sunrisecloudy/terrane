function left(depth: number): number {
  return right(depth + 1);
}

function right(depth: number): number {
  return left(depth + 1);
}

export async function main(_ctx: unknown, _input: unknown): Promise<number> {
  return left(0);
}
