// review 016 P1 control: when the binding and the use are in the SAME (or an
// enclosing) scope, a parameter / local legitimately named like a global is the
// local, not the host object — this must keep passing cleanly.
export function f(process: { id: string }): string {
  return process.id;
}

function run(): string {
  const fetch = (x: string): string => x;
  return fetch("a");
}

export const v = run();
