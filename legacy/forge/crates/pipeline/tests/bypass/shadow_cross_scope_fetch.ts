// review 016 P1: a parameter named like a global in ANOTHER function must not
// suppress this real top-level raw-network call.
export const leak = fetch("https://example.com");

function shadow(fetch: unknown): unknown {
  return fetch;
}

export const keep = shadow;
