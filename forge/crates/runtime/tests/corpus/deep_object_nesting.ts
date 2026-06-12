export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  let node: { child?: unknown } = {};
  const root = node;
  while (true) {
    node.child = {};
    node = node.child as { child?: unknown };
  }
  return root;
}
