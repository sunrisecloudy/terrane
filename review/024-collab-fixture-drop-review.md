# Review 024: fixture and spec table drop

Commit reviewed: `03ce5d50` (`collab(codex): land T006-T017 fixtures, corpora, and spec tables`)

## Findings

1. **[P2] Network vectors encode a parallel allowlist grammar instead of the normative `networkPolicy` shape.**  
   The new SC-5 fixture suite is meant to drive network policy behavior (`forge/fixtures/network/manifest.json:2`), but each case defines top-level `allowlist` entries with snake_case fields such as `url`, `max_response_bytes`, `timeout_ms`, and `response_content_types` (`forge/fixtures/network/exact_host_get_allowed.json:3`). The normative package surface is `manifest.networkPolicy.allow[]` with `origin`, `methods`, `allowedHeaders`, `maxRequestBytes`, `maxResponseBytes`, and `timeoutMs` (`docs/24_NETWORK_POLICY.md:21`), and docs/24 explicitly says there is no compatibility fallback for removed network allowlist input (`docs/24_NETWORK_POLICY.md:44`). If future tests consume this fixture grammar directly, they can prove behavior for a shape real app manifests must never use. Please either rewrite the vectors to wrap the actual `networkPolicy` object or add a schema-checked adapter test that maps these helper fields into the docs/24 shape before policy evaluation.

2. **[P2] The new fixture/corpus inventories are not executable guards yet.**  
   The commit lands manifests for high-value suites like prompt injection (`forge/corpus/injection/manifest.json:2`), UI golden trees (`forge/crates/ui/tests/golden/manifest.json:2`), compat (`forge/fixtures/compat/manifest.json:2`), network, signing, migrations, perf, and replay, but the nearest UI crate still has no tests at all (`forge/crates/ui/src/lib.rs:1`; `cargo test --locked -p forge-ui` reported 0 tests). Existing pipeline tests still read the older `crates/runtime/tests/corpus` / `crates/pipeline/tests/bypass` corpora, not `forge/corpus/injection`, and the public contract exporter only includes `tests/fixtures/**` plus `tests/golden`, not `forge/fixtures/**` or `forge/corpus/**` (`tools/export-public-contract.mjs:59`). Add a small manifest integrity harness per suite now, then wire each suite into the owning crate as the implementation lands; otherwise these files can drift silently while CI stays green.

## Notes

- The file inventory is internally consistent: manifest refs match the JSON files in each new suite.
- The valid Ed25519 signing fixtures verify with Node's `crypto.verify`.
- No new filenames appeared in `task-between-claude-and-codex` during this check.

## Verification

- `git show --check 03ce5d50` passed.
- Parsed all 133 JSON files added/changed by the commit.
- Checked suite manifests against their directories; no missing or unreferenced JSON case files.
- `cargo test --locked -p forge-ui` passed, but ran 0 tests.
- `cargo test --locked -p forge-pipeline --test corpus_rejects --test bypass_corpus` passed.
- Verified `valid_signature.json` and `valid_multi_file_package.json` signatures with Node `crypto.verify`.
