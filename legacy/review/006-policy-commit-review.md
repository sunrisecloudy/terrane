# Review 006 - policy commit 8f54994

Buddy review for Claude on `8f54994 forge-policy: capability + minimal-RBAC engine for ctx.* host calls`.

## Findings

- **P1 - The engine documents itself as the full SC-10 gate but omits required gates.** `PolicyEngine::check` only verifies role, `max_host_calls`, local revocation, manifest capability, and resource scope (`forge/crates/policy/src/lib.rs:171`, `forge/crates/policy/src/lib.rs:180`). The merged PRD requires *all* of actor role, workspace policy, manifest, run profile, platform permission, resource allowlist, and rate/resource limit to pass (`prd-merged/07-security-prd.md:36`; see also `prd-merged/01-core-runtime-prd.md:45`). If runtime treats this as the "gates every ctx.* host call" authority, a denied workspace policy/run profile/platform permission has no place to fail closed. Please either add an explicit decision context for the missing gates, or rename/scope this as the manifest-scope subcheck and make the higher-level policy engine impossible to bypass.

- **P1 - `time`, `random`, and default `ui` are ambient host capabilities.** `HostCall::Time` and `HostCall::Random` return `Ok(())` whenever not locally revoked (`forge/crates/policy/src/lib.rs:265`, `forge/crates/policy/src/lib.rs:275`), and the tests lock that in as "always allowed" (`forge/crates/policy/src/lib.rs:490`). Separately, `Capabilities::default()` grants `ui: true`, so a manifest with no `capabilities` object can still call `ctx.ui.render` (`forge/crates/domain/src/manifest.rs:60`, `forge/crates/domain/src/manifest.rs:69`). That conflicts with zero ambient capability / capability-scoped host APIs (`prd-merged/07-security-prd.md:21`, `prd-merged/01-core-runtime-prd.md:44`). Please add explicit grants or run-profile-controlled allowances for these host calls; deterministic seams can still be cheap, but they should not bypass the same capability decision path.

- **P2 - Bare `*` storage grants are accepted as full-storage access without validation.** `prefix_matches("*", ...)` intentionally matches every key (`forge/crates/policy/src/lib.rs:301`, `forge/crates/policy/src/lib.rs:675`), but neither manifest validation nor policy normalizes/rejects overly broad storage scopes. That makes it easy for generated code to request all per-applet storage and hard for review UI to distinguish "app/*" from "everything". Please validate allowed glob forms, preferably applet-scoped prefixes only, and add negative tests for broad or malformed grants.

## Verification

- `cargo test --locked` from `forge/`: passed.
- `cargo test --locked -p forge-policy` from `forge/`: passed.
- `cargo check --locked --target wasm32-unknown-unknown -p forge-policy` from `forge/`: passed.
- `cargo clippy --locked -p forge-policy -- -D warnings` from `forge/`: passed.
- `cargo check --locked --target wasm32-unknown-unknown` from `forge/`: still fails globally on `rquickjs-sys` and `sqlite-wasm-rs`; not introduced by this commit, but still blocks the full Rust/WASM lane.
