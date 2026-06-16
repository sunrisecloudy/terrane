# Conformance Vector Format

Source of record: `prd-merged/01-core-runtime-prd.md` CR-12 plus the current `RunRecord` and `RecordedCall` contracts in `forge/crates/domain/src/run.rs`.

Conformance vectors are engine-independent descriptions of one observable applet behavior. The same vector must run on QuickJS-native, QuickJS-WASM, and JavaScriptCore once those engines are wired. A conformance run is successful only when the produced `RunRecord` matches the expected observable shape under the vector's declared tolerance. The current CR-12 engine corpus is the `forge/fixtures/conformance-engines/` JS-language/determinism slice; broader host-API, UI-dispatch, live-query, and limit vectors use this format as they are promoted into the cross-engine gate.

## JSON Shape

```json
{
  "vectorVersion": 1,
  "case": "storage_roundtrip",
  "kind": "runtime",
  "description": "storage.set then storage.get",
  "source": {
    "language": "js",
    "body": "export async function main(ctx, input) { ... }\\n",
    "codeHash": "sha256:..."
  },
  "manifest": {
    "entrypoint": "inline.js",
    "min_api": "forge-api@0.1",
    "deterministic": true,
    "capabilities": {},
    "limits": {}
  },
  "input": {},
  "seeds": {
    "random_seed": 1,
    "time_start": 100
  },
  "expected": {
    "hostTrace": [
      { "method": "storage.get", "args": ["app/value"] }
    ],
    "outcome": {
      "status": "completed",
      "result": { "ok": true, "value": {} }
    },
    "replayFingerprint": "{\"calls\":...}"
  },
  "tolerance": {
    "mode": "byte_identical",
    "notes": "No engine-specific differences are permitted."
  }
}
```

## Fields

| Field | Meaning |
|---|---|
| `vectorVersion` | Format version. Start at `1`; increment only for breaking fixture shape changes. |
| `case` | Stable snake_case id. File name should match it. |
| `kind` | `runtime` for applets that execute, `compile_reject` for source rejected before a `RunRecord` exists. |
| `source.language` | `ts` or `js`. Seed vectors may use `js` to avoid coupling the engine suite to SWC while the engine runner is built. |
| `source.body` | The applet source. For `js`, this is the exact code whose SHA-256 is `source.codeHash`. For `ts`, the harness computes the post-SWC hash. |
| `manifest` | Current `forge_domain::Manifest` shape. Keep M0a limits and grants explicit. |
| `input` | The JSON value passed to `main(ctx, input)`. |
| `seeds.random_seed` | Seed for `ctx.random.next()`. |
| `seeds.time_start` | Initial logical clock value for `ctx.time.now()`. |
| `expected.hostTrace` | Ordered `RecordedCall.method` and `RecordedCall.args`. Responses are checked through `replayFingerprint`; this list is for readable runner errors. |
| `expected.outcome` | Either a completed `AppResult` or an error code/kind. Runtime vectors map directly to `RunOutcome`. |
| `expected.replayFingerprint` | The exact `RunRecord::replay_fingerprint()` string for `byte_identical` runtime vectors. |
| `tolerance.mode` | `byte_identical`, `error_code_only`, or `compile_error_code_only`. |

## RunRecord Binding

For `kind = "runtime"`, the harness builds a `RunRecord` with:

- `code_hash` from the transpiled/executable JS;
- `input` from the vector;
- `random_seed` and `time_start` from `seeds`;
- `calls` from the actual run;
- `permissions` from the evaluated `PermissionSnapshot`;
- `outcome` from the actual run.

Then it compares `RunRecord::replay_fingerprint()` against `expected.replayFingerprint` when `tolerance.mode = "byte_identical"`.

Byte-identical fields:

- code hash;
- input;
- random seed and time start;
- ordered host-call methods, args, and responses;
- evaluated permission snapshot;
- completed result or failed `CoreError` kind/detail.

Fields that may legitimately differ:

- `run_id`, because it identifies an invocation and is intentionally excluded from the replay fingerprint;
- compile diagnostic text for `compile_reject`, where only the stable `CoreError` code is portable;
- CPU and memory suspension detail text until all engines expose identical interruption points.

## Limit Tolerance

Host-call budget vectors can be exact because budget accounting lives in the shared Rust host shim. CPU and memory limit vectors are release-critical once promoted into the CR-12 harness, but may need `error_code_only` while QuickJS-WASM and JSC are being wired: the portable requirement is `ResourceLimitExceeded`, no host crash, no successful completion, and no unexpected host effects. Current limit vectors outside `conformance-engines` remain runtime conformance seeds until they are wired through the engine-agnostic harness. Once every engine reports a stable detail string and stop point, those vectors should graduate to `byte_identical`.

## Seed Fixtures

Seed vectors live in `forge/fixtures/conformance/*.json`. They cover pure compute, current `ctx.*` namespaces, deterministic seams, shared host-call limits, engine CPU/memory containment, and compile-time forbidden-construct rejection for the runtime conformance suite. They are not automatically part of CR-12 until each vector is wired through the engine-agnostic cross-engine harness with a portable tolerance.
