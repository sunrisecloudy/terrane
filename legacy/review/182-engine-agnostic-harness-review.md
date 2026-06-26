# Review 182 - Engine-agnostic conformance harness (043b2c3c)

Findings:

- [P2] Make the engine-injection test observable. `second_engine_runs_the_same_corpus_byte_identically` is described as proving the harness is not hard-wired to `QuickJsEngine` (`forge/crates/runtime/tests/conformance_engines.rs:287-303`), but `AdapterEngine` just delegates to an inner `QuickJsEngine` (`forge/crates/runtime/tests/conformance_engines.rs:306-327`). If `record_run_with_engine` later ignored its `engine` parameter and constructed `QuickJsEngine` internally, both corpus tests would still pass with the same fingerprints. Add a tiny sentinel `JsEngine` test that returns a unique `AppResult` or `RuntimeError` and assert `record_run_with_engine` / `replay_with_engine` surface that sentinel; keep the corpus adapter as a compatibility smoke test if useful.

- [P2] Do not let the CR-12 spec read as the whole release-blocking suite yet. The normative PRD requires CR-12 to cover every host API, limit behavior, and deterministic-replay case on QuickJS-native, QuickJS-WASM, and JSC (`prd-merged/01-core-runtime-prd.md:59`, `prd-merged/01-core-runtime-prd.md:78-81`). The new spec paraphrases CR-12 down to byte-identical `main(ctx, input)` output (`forge/spec/cross-engine-conformance.md:8-11`) and the harness manifest intentionally has no storage/db grants and only pure-compute plus seeded-clock vectors (`forge/crates/runtime/tests/conformance_engines.rs:86-107`). Either label this document/test as the JS-language/determinism sub-corpus, or add the missing host API and limit-behavior cross-engine vectors so future M0b work does not mistake this for complete CR-12 coverage.

Checks:

- `cargo test -p forge-runtime --test conformance_engines --offline`
- `cargo test -p forge-runtime --test containment date_wallclock_is_neutralized_in_deterministic_mode --offline`
- `cargo test -p forge-runtime --test determinism --offline`
