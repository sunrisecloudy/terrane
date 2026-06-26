# Codex working agreements

> **ACTIVE WORK IS v1 (2026-06-12).** The normative spec for all new work is
> **`prd-merged/`** (see `docs/00_V1_PIVOT.md`). The v1 implementation lives in
> **`forge/`** as a Rust workspace.

## v1 working agreement (`forge/`, normative)

- Spec of record: `prd-merged/00-master-prd.md` + sub-PRDs 01-09; decisions in
  `prd-merged/DECISIONS.md`.
- Workspace: `forge/` (Rust). Crate map: `prd-merged/01-core-runtime-prd.md` §2.
- Per crate: `cargo test -p forge-<crate>` green and
  `cargo clippy -p forge-<crate> -- -D warnings` clean before commit.
- Reuse `forge-domain` types (`CoreError`, ids, `RecordEnvelope`, `Manifest`,
  `RunRecord`); do not redefine them.
- Keep pure-logic crates (`domain`, `schema`, `policy`, `ui`, pipeline core)
  `wasm32`-clean; native-only deps such as `rusqlite` and JS engines are
  target-gated behind `cfg(not(target_arch = "wasm32"))`.
- Return `CoreError`; no panic or `unwrap` on real paths (tests may `unwrap`).
- Never `git add -A` and never commit `forge/target/` (gitignored).
- Collaboration handoffs live in `task-between-claude-and-codex/`; commit
  reviews live in `review/`.

## Testing expectations

- After editing Forge crates, run the crate-specific tests and clippy gate.
- After editing shared Forge/runtime behavior, run the relevant workspace tests,
  `cargo clippy --workspace --all-targets --locked -- -D warnings`, and the
  replay demo when determinism could be affected.
- After editing public contracts or tooling, run the matching Node checks under
  `tools/`.
- After editing native bridge code, re-run the contract or source checks that
  prove the platform still matches the Forge/reference-host contract.

## Architecture preference

Keep business/domain logic deterministic and replayable. Async, native, and
platform effects live at the shell edge. Apps request effects through narrow,
permission-checked bridge or host-call surfaces.

## Where to look

- v1 PRD and decisions: `prd-merged/`.
- Runtime/host specs: `forge/spec/`.
- Forge workspace: `forge/`.
- Public contract export: `docs/35_PUBLIC_CONTRACT_EXPORT.md`.
- Release and CI: `docs/12_RELEASE_AND_CI.md`.
- Implementation status: `IMPLEMENTATION_STATUS.md`.
