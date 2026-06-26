# M0b WASM target-feasibility findings (for the final report)

Method: `CARGO_TARGET_DIR=/tmp/forge-wasm-target` for every build (native
`forge/target` untouched). Targets `wasm32-unknown-unknown` + `wasm32-wasip1`
already installed. Toolchain: Apple clang (no wasm LLVM backend, no WASI sysroot).

## Per-crate result

| Crate | wasm32-unknown-unknown | Blocker |
|---|---|---|
| forge-domain | CLEAN | — (sha2 pure Rust) |
| forge-schema | CLEAN | — |
| forge-policy | CLEAN | — |
| forge-secrets | CLEAN | — |
| forge-ui | CLEAN | — |
| forge-testkit | CLEAN | — |
| forge-signing | CLEAN | ed25519-dalek/base64/sha2 all default-features=false, pure Rust |
| forge-crdt | CLEAN | loro 1.13.1 is wasm-native |
| forge-pipeline | CLEAN | swc_core 68 pure Rust |
| forge-runtime | CLEAN | rquickjs (QuickJS C) already `#[cfg(not(target_arch="wasm32"))]`-gated; JsEngine trait + replay logic remain |
| forge-storage | **BLOCKED** | rusqlite "bundled" SQLite C amalgamation |
| forge-sync | BLOCKED (via storage) | — |
| forge-core | BLOCKED (via storage) | — |
| forge-cli | BLOCKED (via storage) | — |
| forge-ffi | BLOCKED (via storage) | — |

## Single blocker: rusqlite bundled SQLite C (gates 5 of 15 crates)

- **wasm32-unknown-unknown:** libsqlite3-sys auto-swaps to `sqlite-wasm-rs`, which
  `cc`-compiles SQLite and dies at `No available targets are compatible with triple
  "wasm32-unknown-unknown"` — the system Apple clang has no wasm LLVM backend. Even
  with wasm-LLVM it yields an in-memory wasm SQLite needing a JS-side OPFS/IndexedDB VFS.
- **wasm32-wasip1:** rusqlite keeps the normal C amalgamation; dies at
  `sqlite3.c: fatal error: 'stdio.h' file not found` — no WASI libc sysroot (wasi-sdk).
  WASI gives a real filesystem, so wasi-sdk + on-disk SQLite is the more plausible path.

Note: rquickjs (QuickJS C) would be a second hard C blocker but is already cfg-gated
out of wasm. loro + swc — often assumed problematic — are fully wasm-clean here.

## M0b WASM story (assessment)

The entire pure-logic + crypto/CRDT/compiler layer of Forge compiles to
`wasm32-unknown-unknown` today, unmodified: domain, schema, policy, secrets, ui,
testkit, signing (Ed25519 verify), crdt (loro), pipeline (swc TS→JS + AST scan),
and runtime (the `JsEngine` abstraction minus its native QuickJS impl). The single
wall is forge-storage's bundled SQLite C, which transitively blocks the persistence/
command/event spine (sync/core/cli/ffi). The QuickJS runtime story is already handled
architecturally — the engine is trait-abstracted + native-gated, so a browser host
supplies a wasm `JsEngine` behind the existing trait. Getting a real browser/WASI
Forge host would take, roughly: (1) replace/feature-gate the SQLite backend
(wasm32-wasip1 + wasi-sdk for on-disk SQLite, or sqlite-wasm-rs + OPFS VFS for the
browser, or abstract `Store` over a non-SQLite backend the way runtime abstracts the
JS engine); (2) install the missing wasm toolchain bits (wasm-capable clang/LLVM
and/or wasi-sdk); (3) provide a wasm `JsEngine` impl. Crypto, CRDT, schema-migration,
and TS-compile layers need zero work — wasm-ready now.
