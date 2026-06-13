# Commit Review: f09d427e

Reviewed commit: `f09d427e collab(codex): conformance vector format spec (T020)`

## Findings

No actionable findings. The new conformance vector format doc is consistent with the T020 handoff: it binds runtime vectors to `RunRecord::replay_fingerprint()`, calls out byte-identical versus tolerated fields, and explicitly marks CPU/memory suspension as `error_code_only` until QuickJS-WASM/JSC interruption details are stable. Existing `forge/fixtures/conformance/*.json` files also match the documented top-level shape and parse cleanly.

## Verification

- `git show --check f09d427e`
- `for f in forge/fixtures/conformance/*.json; do python3 -m json.tool "$f" >/dev/null || exit 1; done`
