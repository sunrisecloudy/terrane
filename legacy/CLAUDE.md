# Claude Notes: Terrane

## Project Purpose

Terrane is the public local-first platform for AI-generated personal apps and applets. It provides the trusted local runtime, capability sandbox, data layer, bridge or host-call surface, validation, conformance tests, signing rules, release artifacts, and native shell contracts.

The current product direction is v1 Forge: a Rust core with TypeScript applets/scripts, a capability sandbox, Loro-over-SQLite data, sync, deterministic replay, UI component trees, and thin platform shells. The older build-free HTML/CSS/JS package line still exists in runtime, examples, fixtures, and compatibility docs, but new product work should follow the v1 direction unless the task is explicitly about legacy/prototype surfaces.

## Relationship To Terrane Premium

`../terrane-premium` is the private SaaS control plane. It depends on this repo; this repo must not depend on it.

Public Terrane owns generated-app-visible behavior:

- package/app contract rules;
- runtime bridge or host-call semantics;
- schemas, fixtures, and conformance tests;
- local engine and reference behavior;
- SQLite/local persistence contracts;
- public signing/canonicalization rules;
- public contract export and verification tools;
- native host parity requirements.

Terrane Premium owns hosted coordination:

- identity, organizations, teams, roles, devices, and sessions;
- billing, seats, and entitlement snapshots;
- encrypted sync and hosted backups;
- team catalogs, marketplace publishing, review, trust, and revocation;
- cloud signing key custody;
- admin, audit, governance, operations, support, and abuse controls.

When a behavior can be observed by generated apps or applets, implement and document it here first. Premium should consume it through `artifacts/public-contract.json` or a pinned release/source checkout, not through private forks or hidden semantics.

## Source Of Truth

Read these first:

- `AGENTS.md` for active working agreements and testing expectations.
- `docs/00_V1_PIVOT.md` for the v1 supersession notice.
- `prd-merged/00-master-prd.md` for the current product direction.
- `prd-merged/01-core-runtime-prd.md` through `prd-merged/09-roadmap-quality-gates-prd.md` for v1 subsystem requirements.
- `prd-merged/DECISIONS.md` for resolved design choices.
- `forge/spec/` for runtime, host, sync, command, policy, conformance, UI, and data contracts.
- `docs/34_LOCAL_FIRST_OSS_SERVER_AND_SAAS_PRD.md` for the public local engine vs private SaaS split.
- `docs/35_PUBLIC_CONTRACT_EXPORT.md` for downstream contract export rules.
- `IMPLEMENTATION_STATUS.md` for built vs planned work.

The pointer files `docs/00_PRD.md` and `docs/01_ARCHITECTURE.md` are retained for stable links and now point at v1 sources.

## Repository Map

- `forge/` is the normative v1 Rust workspace.
- `forge/crates/core/` is the command/event facade consumed by shells.
- `forge/crates/domain/`, `schema/`, `storage/`, `crdt/`, `sync/`, `runtime/`, `policy/`, `ui/`, `llm/`, `ffi/`, `server/`, `testkit/`, and `cli/` hold the v1 core subsystems.
- `forge/spec/` contains the contract documents that should line up with implementation and tests.
- `runtime-web/` and `tools/reference-host/` are still important for current bridge/package/runtime behavior and conformance evidence.
- `native/` contains macOS, iOS, Android, Windows, and Linux hosts.
- `webapps/examples/` contains legacy build-free example packages used as fixtures and scenarios.
- `artifacts/public-contract.json` is the downstream contract consumed by private repos such as Terrane Premium.
- `tools/export-public-contract.mjs` and `tools/verify-public-contract.mjs` generate and verify that public contract.

## Working Rules

- Treat `prd-merged/` plus `forge/spec/` as the current normative direction.
- Do not add private SaaS concerns to the public local engine.
- Keep business/domain logic deterministic and replayable; platform effects belong at the shell edge.
- Reuse existing Forge domain types and errors instead of redefining them.
- Keep pure logic crates wasm-clean where intended.
- Avoid `unwrap` or panics on real paths; return typed errors.
- Preserve generated-app/app-visible behavior through public docs, schemas, fixtures, and conformance tests.
- Preserve unrelated dirty or untracked work.

## Validation

For Forge changes, prefer focused crate checks while iterating:

```sh
cd forge
cargo test -p forge-<crate>
cargo clippy -p forge-<crate> -- -D warnings
```

For shared runtime or contract changes, broaden the gate:

```sh
cd forge
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p forge-cli -- demo
```

For public contract changes:

```sh
node --no-warnings tools/export-public-contract.mjs --out artifacts/public-contract.json
node --no-warnings tools/verify-public-contract.mjs --contract artifacts/public-contract.json --root .
```

After accepting a public contract change, refresh the Premium pin in `../terrane-premium` intentionally and run its contract verification.
