export async function main(_ctx: unknown, _input: unknown): Promise<unknown> {
  // Capturing benign locals by value (object value, array element, call arg)
  // must not trip the forbidden-global value-position check.
  const evaluate = (x: number) => x + 1;
  const o = { run: evaluate };
  const fns = [evaluate];
  return o.run(1) + fns[0](2);
}
