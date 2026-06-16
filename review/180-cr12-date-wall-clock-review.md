# Review 180 - CR-12 Date wall-clock gap (186d8c03)

Findings:

- [P1] Neutralize the built-in Date wall clock before treating the conformance corpus as deterministic. The new CR-12 spec says the determinism rules are runtime-enforced and explicitly names `Date.now()` / zero-arg `new Date()` as wall-clock sources that must not be used (`forge/spec/cross-engine-conformance.md:49-61`), but the runtime still builds the full standard-library realm with `Date` available (`forge/crates/runtime/src/engine.rs:332-341`) and only hardens `eval` plus `Math.random` during `install_ctx` (`forge/crates/runtime/src/engine.rs:1206-1210`). That leaves a deterministic script able to return `Date.now()`, `Date()`, or `new Date().getTime()` with no recorded `ctx.time.now` call, so two runs with the same `random_seed`/`time_start` can produce different `replay_fingerprint()` values and bypass CR-1's "no clock except injected host functions" rule. The new date vector does not catch this because it uses the correct seeded seam (`forge/fixtures/conformance-engines/date_under_seeded_clock.json:60`). Add engine-level Date hardening that preserves `new Date(ms)` / Date arithmetic but makes unseeded reads throw (or routes them through `ctx.time` in a recorded way), and add a containment/conformance vector that attempts `Date.now()`, `Date()`, and zero-arg `new Date()`.

Checks:

- `cargo test -p forge-runtime --test conformance_engines --offline`
- `cargo test -p forge-runtime --test containment math_random_is_neutralized_in_deterministic_mode --offline`
