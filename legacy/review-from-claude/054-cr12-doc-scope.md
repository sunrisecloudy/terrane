# Review 054 - CR-12 doc scope closure

Addresses the Ampere P2 finding from the 157-184 review audit:

- Several PRD and platform-plan docs overclaimed CR-12 coverage as if every host
  API, resource-limit mode, UI dispatch path, live-query path, QuickJS-WASM
  backend, and JSC backend were already covered by the release-blocking
  cross-engine suite.

Fix summary:

- CR-12 is now described as release-blocking for **covered vectors**.
- The current covered corpus is named as
  `forge/fixtures/conformance-engines/*.json`: JS-language/determinism
  `main(ctx,input)` vectors run through the engine-agnostic `JsEngine` harness.
- Broader host/API/limit/UI/live-query vectors are explicitly future
  promotions into that same harness before they can be claimed as release gates.
- Windows platform docs now distinguish QuickJS-native covered-vector reruns from
  broader runtime/platform conformance seeds.

Verification:

- `rg -n "every host API|all three engine|dual-engine conformance|from day one|from day 1|already conformance-tested|already conformant|full loop .*both engines|both engines|forge/fixtures/conformance/\\*\\.json|divergence = release blocker|Conformance suite from week one" prd-merged forge/spec window-plan forge/fixtures/conformance-engines forge/crates/runtime/tests/conformance_engines.rs`
- `git diff --check -- prd-merged/DECISIONS.md prd-merged/01-core-runtime-prd.md prd-merged/00-master-prd.md prd-merged/06-platform-shells-prd.md prd-merged/README.md prd-merged/09-roadmap-quality-gates-prd.md forge/spec/conformance-vector-format.md window-plan/00-OVERVIEW.md window-plan/03-PLATFORM-SERVICES.md window-plan/04-PACKAGING-CI.md window-plan/05-MILESTONES.md`
