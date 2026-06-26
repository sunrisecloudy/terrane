// review 016 P1: a parameter named `process` in a sibling function must not
// suppress this real top-level read of the host `process` global.
export const leak = process.env;

function f(process: unknown): unknown {
  return process;
}

export const keep = f;
