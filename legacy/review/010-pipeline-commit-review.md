# 010 - Pipeline Commit Review

Reviewed commit: `612bfa6` (`forge-pipeline: TS->JS type-strip + static policy scan`)

Nice slice, Claude. SWC is now in-core/offline, corpus integration is wired, and the pipeline crate builds for `wasm32-unknown-unknown`. A few issues look worth fixing before this becomes the front-door contract for the runtime.

## Findings

- **P1 - Pipeline `code_hash` is not the hash that runtime records.** `forge-pipeline::Program` computes `sha256:` over transpiled JS and documents it as the `RunRecord.code_hash` (`forge/crates/pipeline/src/lib.rs:43-55`, `79-82`), but the runtime still constructs its own `Program` and records `fnv1a64:` from `Program::code_hash()` (`forge/crates/runtime/src/lib.rs:59-77`, `forge/crates/runtime/src/runner.rs:111-115`). So a real TS -> SWC -> runtime run will not preserve the pipeline provenance hash this commit promises. Unify the program type or let runtime accept the pipeline-produced hash, then add an integration test that `compile(ts)` -> runtime run stores exactly the same `sha256:` hash.

- **P1 - Static policy scan has bypassable eval/Function/process/fetch forms.** The AST checks only reject direct callees like `eval(...)`, `Function(...)`, `fetch(...)`, `require(...)`, and direct constructors like `new Function(...)` / `new XMLHttpRequest()` (`forge/crates/pipeline/src/scan.rs:217-259`). Member reads only flag properties named `process`, `require`, `__proto__`, or `Object.prototype` (`forge/crates/pipeline/src/scan.rs:147-170`), and the text backstop only catches an identifier immediately followed by `(` (`forge/crates/pipeline/src/scan.rs:279-325`). That misses common spellings such as `globalThis.eval("1")`, `(0, eval)("1")`, `const e = eval; e("1")`, `const F = Function; new F("...")`, `process.env`, and `globalThis["fetch"]("https://x")`. Since CR-13/SC-1 require static rejection and the current QuickJS engine still exposes eval/Function, add tests for alias/member/computed forms and either reject reads of these dangerous globals or resolve simple aliases before allowing execution.

- **P2 - Static imports remain an unhandled gap between CR-10 and the runtime loader.** The transpiler intentionally emits ES-module JS and preserves exports (`forge/crates/pipeline/src/lib.rs:74-92`, test at `forge/crates/pipeline/src/lib.rs:116-123`), but the runtime evaluates source as a global script after string-replacing export declarations (`forge/crates/runtime/src/engine.rs:237-248`, `436-448`). No current step rejects or resolves static `import ... from ...`, even though CR-10 allows multi-file local imports. For M0a, either reject all static imports with a clear error, or implement the local-import bundling/resolution seam before feeding JS to `QuickJsEngine`.

- **P3 - The text backstop can reject benign comments/strings.** `contains_ident_call` scans raw source bytes, not tokens (`forge/crates/pipeline/src/scan.rs:279-325`), so text like `const msg = "eval(";` or a comment containing `Function(` can become a `PermissionDenied` even when the AST is clean. That will make generated apps fail for explanatory strings or fixture data. Prefer AST-only checks for parseable source, or make the backstop token/comment/string aware.

## Verification

- `cargo test --locked -p forge-pipeline` passed.
- `cargo build --locked -p forge-pipeline --target wasm32-unknown-unknown` passed.
- `cargo test --locked` passed for the forge workspace.
- `cargo clippy --locked --workspace --all-targets -- -D warnings` passed.
