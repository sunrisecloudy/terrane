# Cross-engine conformance (CR-12) — byte-identical output across JS engines

> Spec. The behavioral contract is `forge/fixtures/conformance-engines/` (the
> `manifest.json` + divergence-prone case vectors) driven by the engine-agnostic
> harness `forge/crates/runtime/tests/conformance_engines.rs`, plus the realm
> determinism hardening in `forge-runtime` (`src/engine.rs`).

prd-merged/01-core-runtime-prd.md **CR-12**: *"Cross-engine conformance: the SAME
program run via `main(ctx, input)` must produce BYTE-IDENTICAL deterministic
output — the return value AND the recorded host-call trace — across JS engines.
This is an M0b-exit / release blocker."*

This is the contract that makes the engine **pluggable** (CR-2). The runtime runs
applets through the [`JsEngine`](../crates/runtime/src/lib.rs) trait; today the
only implementation is `QuickJsEngine` (rquickjs), but the conformance corpus is
written against the trait, not the implementation, so a second engine
(JavaScriptCore, a QuickJS-WASM build) can be dropped in and held to exactly the
same vectors. An engine is conformant iff every vector in
`fixtures/conformance-engines/` produces the same observable output it produces on
QuickJS.

## 1. The determinism contract

The **observable output** of a run is captured by
`RunRecord::replay_fingerprint()` (`forge/crates/domain/src/run.rs`): the code
hash, input, seeds, the **ordered host-call trace** (each call's method, args, and
response), the evaluated permission snapshot, and the **outcome** (the returned
`AppResult` or the failing `CoreError`). Two engines are conformant on a vector iff
their fingerprints are **byte-identical**.

Byte-identical means, concretely:

- **Return value** — `main`'s resolved `{ ok, value }` must be JSON-equal, with the
  same number formatting, the same property order, and the same string bytes.
- **Host-call trace** — the same `ctx.*` calls in the same order with the same args
  and the same responses. (Seam reads — `ctx.time.now()`, `ctx.random.next()` — are
  served from the shared Rust recorder, so they are identical by construction; an
  engine never sources time or randomness itself.)
- **Outcome shape** — a completed result is compared in full; a failure is compared
  by its portable `CoreError` code/kind (see §3 Normalization for why the *message*
  is not always portable).

Determinism is the precondition for cross-engine equality, and it has teeth on a
single engine too: the harness re-records each vector and asserts the fingerprint
is unchanged (run-to-run stability), then replays the recorded run and asserts
record→replay identity. A vector that is non-deterministic on one engine could
never be byte-identical on two.

### Determinism rules (enforced by the runtime, not trusted to the applet)

- **No wall-clock.** `ctx.time.now()` is a *logical* clock seeded from `time_start`
  that advances by one tick per read (`recorder.rs::LogicalClock`). `Date.now()` /
  `new Date()` with no argument read wall-clock and must not be used; a portable
  applet derives all time from `ctx.time.now()` (see `date_under_seeded_clock`).
- **No engine randomness.** The only randomness is `ctx.random.next()`, a seeded
  SplitMix64 stream (`recorder.rs::SplitMix64`) served from the recorder.
  `Math.random` is **neutralized** in the deterministic realm: the engine replaces
  it with a throwing stub (`engine.rs::disable_nondeterministic_random`), non-
  writable / non-configurable, so an applet cannot reach an unseeded entropy source
  (which would diverge between QuickJS and JSC and break replay). See
  `math_determinism_no_random`.
- **Deterministic ordering.** JSON object key order, `Object.keys`/`for-in`
  enumeration order, and `Map`/`Set` iteration order are all spec-defined and must
  match (see `json_stringify_ordering_escaping`, `property_enumeration_order`,
  `map_set_iteration_order`). Host-side, the replay fingerprint serializes objects
  with sorted keys, so the *trace* is canonical regardless of insertion order.

## 2. Divergence-prone areas (the corpus)

Engines historically diverge in ~15 areas; each is a vector in
`fixtures/conformance-engines/`. Every fixture declares a `divergenceArea`, a
`divergenceRisk` (how likely two engines disagree), and an `equivalence` class
(`required_identical` vs `normalized`, see §3).

| Vector | Divergence area | Risk | Equivalence |
|---|---|---|---|
| `number_formatting_edges` | Number→string: `toFixed` rounding, `-0`, large/precise ints | high | required_identical |
| `string_locale_and_radix` | `toString(radix)`, locale-independent comparison | medium | required_identical |
| `date_under_seeded_clock` | `Date` over the seeded logical clock (never wall-clock) | high | required_identical |
| `json_stringify_ordering_escaping` | `JSON.stringify` key ordering + control/unicode escaping | high | required_identical |
| `unicode_string_normalization` | UTF-16 units vs code points, `normalize()`, surrogate / WTF-8 edges | high | required_identical |
| `array_sort_stability` | `Array.sort` stability (ES2019 stable sort) | medium | required_identical |
| `regexp_unicode_lookbehind` | RegExp `/u`, `\p{…}`, lookbehind, named groups | high | required_identical |
| `map_set_iteration_order` | `Map`/`Set` insertion-order iteration (delete+re-add) | medium | required_identical |
| `error_message_normalized` | Error `.message` / `.stack` shape | high | **normalized** |
| `try_finally_microtask_order` | `try/finally` + `await`/microtask interleaving | high | required_identical |
| `typed_array_arraybuffer` | TypedArray / `ArrayBuffer` / `DataView` (endianness, clamping) | medium | required_identical |
| `bigint_arithmetic` | `BigInt` arithmetic, `asUintN`/`asIntN`, string serialization | medium | required_identical |
| `property_enumeration_order` | `[[OwnPropertyKeys]]` order (integer keys then insertion) | high | required_identical |
| `parse_int_float_edges` | `parseInt`/`parseFloat` radix, junk, `Infinity`, empty | medium | required_identical |
| `math_determinism_no_random` | `Math` determinism; `Math.random` neutralized | high | required_identical |
| `recursion_stack_limit` | Deep recursion trips the stack limit identically | high | **normalized** |

Highest divergence risk (the areas a second engine is most likely to break on
first): **JSON/property/enumeration ordering**, **unicode + WTF-8 string edges**,
**RegExp `/u` + lookbehind + named groups**, **number/`-0`/`toFixed` formatting**,
**microtask ordering**, and **the stack-limit trip point** (engine-specific depth —
hence normalized).

## 3. Host-boundary normalization

Some observable shapes are *legitimately* engine-specific and must be **normalized
at the host boundary** before comparison, or the byte-identical contract would be
impossible to satisfy. A vector's `equivalence` field declares which class it is:

- **`required_identical`** — the output must be byte-for-byte equal on every engine.
  This is the default; most divergence areas have an ECMAScript-mandated answer, so
  a conformant engine simply produces it.
- **`normalized`** — the *raw* engine output is implementation-defined, so the
  applet (and the host) observe only a **normalized projection** of it. The vector
  pins the normalized projection, never the raw shape.

Only two vectors are `normalized`; the normalization rules are:

1. **Error `.stack` is never observable.** A stack trace's frames, line/column
   numbers, and formatting are entirely engine-specific. A conformant applet must
   not put `.stack` in its return value or pass it across a `ctx.*` boundary; it
   returns at most `error.name` + a **stable, applet-authored** `message`. See
   `error_message_normalized`.
2. **Built-in error `.message` text is not portable.** The message a built-in
   throws (e.g. a `TypeError` on a null property access, a `RangeError` from
   `toFixed(101)`) differs between engines. Vectors normalize to the error
   **`.name`** (`"TypeError"`, `"RangeError"`) — which *is* spec-defined — not the
   raw message. At the run boundary the engine already surfaces an uncaught throw
   as a `CoreError::RuntimeError` whose code (`"RuntimeError"`) is portable while
   its detail text is not (mirrored by `conformance-vector-format.md`'s
   "CPU/memory suspension detail text" carve-out). See `error_message_normalized`.
3. **The stack-limit depth is not observable.** The exact recursion depth at which
   an engine overflows is implementation-defined. The portable contract is only
   that unbounded recursion throws a **catchable `RangeError`** (never a host/FFI
   crash — the runtime caps the C stack via `set_max_stack_size` so the overflow
   becomes a JS exception). A conformant applet observes `error.name === "RangeError"`,
   never the depth. See `recursion_stack_limit`.

**Why `Math.random` is `required_identical`, not `normalized`.** It is tempting to
file `math_determinism_no_random` under normalization, but it is the opposite case:
`Math.random` is **neutralized by the host**, not left implementation-defined.
`typeof Math.random` stays `"function"` (so feature-detection is identical across
engines) but the host installs *the same* throwing stub
(`engine.rs::disable_nondeterministic_random`) in every realm, so a call throws an
`Error` with `error.name === "Error"` **by construction** — the same bytes on
QuickJS and on a future JSC. The applet projects only `error.name` (`"Error"`),
which the host pins, so the observable is byte-identical, not engine-defined.
Contrast `recursion_stack_limit` (rule 3): there the engine still picks the depth,
so the host can only normalize the *outcome* (`RangeError`) and not the raw event —
which is the actual `normalized` case. `math_determinism_no_random` is therefore
`required_identical`. See `math_determinism_no_random`.

Everything *not* covered by rules 1–3 is `required_identical`: number formatting,
JSON ordering/escaping, enumeration/iteration order, RegExp results, typed-array
byte layout (via an explicit-endianness `DataView`), BigInt arithmetic, the
host-neutralized `Math.random` throw, and microtask/`try-finally` ordering are all
either ECMAScript-mandated or host-pinned and must match exactly.

## 4. Fixture format

Each `fixtures/conformance-engines/<case>.json` extends the M0a conformance vector
shape (`conformance-vector-format.md`) with the cross-engine metadata:

| Field | Meaning |
|---|---|
| `case` | Stable snake_case id; the file name matches it. |
| `divergenceArea` | The divergence-prone area this vector probes. |
| `divergenceRisk` | `high` / `medium` — likelihood two engines disagree here. |
| `equivalence` | `required_identical` or `normalized` (§3). |
| `source.body` / `source.codeHash` | The exact JS run via `main(ctx, input)` and its canonical `sha256:` hash. |
| `seeds` | `random_seed` + `time_start` for the deterministic seams. |
| `expected.hostTrace` | Readable ordered `(method, args)` list (responses ride the fingerprint). |
| `expected.outcome` | The completed `AppResult` (or normalized failure). |
| `expected.replayFingerprint` | The exact `RunRecord::replay_fingerprint()` — the byte-identical contract. |
| `tolerance.mode` | `byte_identical` for the whole corpus (normalization happens in the *source*, so the projected output is exact). |

The vectors were generated by running each source through the real record spine,
so the baked-in `expected` block is exactly what the engine deterministically
produces — never a hand-typed guess. The harness re-derives it on every run and
fails on any drift.

## 5. The harness is engine-agnostic

`forge/crates/runtime/tests/conformance_engines.rs` loads the corpus and, for each
vector, drives `record_run` (which runs `main` through the `JsEngine` trait),
asserting:

1. the produced `replay_fingerprint` is byte-identical to `expected` (the
   cross-engine contract);
2. re-recording is byte-identical (run-to-run determinism);
3. record→replay is byte-identical (replay identity).

Because the harness only ever touches the `JsEngine` seam, wiring a second engine
is purely additive: implement the trait, point the harness at it, and the same
corpus holds it to byte-identical behavior. A corpus-honesty guard asserts every
declared `manifest.json` case is exercised, so a new vector fails the suite until
its expectation is baked in.

## 6. Deferred infra: the real JavaScriptCore backend

The second engine itself — a real **JavaScriptCore** (or QuickJS-WASM) `JsEngine`
implementation — is **deferred infrastructure**, not part of this corpus. Linking
JSC requires a system framework (`JavaScriptCore.framework` on Apple platforms /
the WebKit JSC library elsewhere) and an FFI shim that is out of scope here. What
this work delivers is the framework that makes adding it *safe and verifiable*:

- the **engine-agnostic corpus** (`fixtures/conformance-engines/`) of the ~15
  divergence-prone areas a second engine is most likely to break on;
- the **determinism hardening** (`Math.random` neutralized, seeded clock/RNG,
  canonical trace ordering) that makes every vector byte-identical and replayable on
  the existing engine, so divergence on the second engine is unambiguously the
  second engine's fault;
- the **host-boundary normalization rules** (§3) that flag exactly which shapes a
  second engine is allowed to differ on (error `.stack`/`.message`, stack-limit
  depth) versus which it must reproduce bit-for-bit.

When the JSC backend lands, it implements `JsEngine`, the harness runs the
**unchanged** corpus against it, and any byte difference is a conformance failure
the corpus localizes to a specific divergence area. M0b exit requires that result
to be green on both engines.
