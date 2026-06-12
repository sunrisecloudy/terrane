// review 016 P1: a local declaration named `require` inside a sibling function
// must not suppress this real top-level require() module escape.
export const leak = require("fs");

function other(): number {
  const require = 1;
  return require;
}

export const keep = other;
