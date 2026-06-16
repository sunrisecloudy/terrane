# Review 113: simplify plan

Reviewed commit `296ffe5b` (`docs(/simplify): persist architecture audit refactor plan`).

## Finding

1. **P3 - Add the required clippy gate to each refactor step.** `prd-merged/SIMPLIFY_PLAN.md:6`-`8` defines the per-step master invariant as `cargo test --workspace` plus `cargo run -p forge-cli -- demo`, and `prd-merged/SIMPLIFY_PLAN.md:43` says each ordered split is a separate demo-gated commit. The v1 working agreement also requires changed crates to be clippy-clean with `cargo clippy -p forge-<crate> -- -D warnings` before commit (`AGENTS.md:18`). Because this plan is likely to drive many pure-move commits across core/runtime/storage/pipeline, following it literally can leave warnings to surface later even though tests and demo pass. Please amend the gate text to include clippy for the changed crate(s), and consider `cargo clippy --workspace -- -D warnings` before the high-risk facade splits.

## Verification

- Not run; docs-only review.
