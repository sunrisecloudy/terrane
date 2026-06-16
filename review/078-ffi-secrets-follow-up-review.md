# Review 078 - FFI and SC-13 follow-up

Reviewed commits: `8147681d`, `107bb127`, `c3ce56d3`, `a647977a`, `a49c34ad`.

## Findings

1. **P1 - The C ABI can create aliased mutable access to `WorkspaceCore`.** `forge_core_handle_command` and `forge_core_drain_events` both rebuild `&mut (*handle).core` from the raw pointer (`forge/crates/ffi/src/lib.rs:148-179`), while `forge_core_close` can drop the same pointer (`forge/crates/ffi/src/lib.rs:291-306`). The C# wrapper serializes calls on one wrapper instance, but the exported ABI itself has no mutex, closed flag, or ref-counted ownership, so two hosts/threads can concurrently call `handle_command`/`drain_events` or race `close` against an in-flight call and enter Rust UB. Put the synchronization/lifetime guard inside `ForgeCoreHandle` (for example `Mutex<WorkspaceCore>` plus an atomic closed state, or a ref-counted handle) so the boundary is safe regardless of the binding layer.

2. **P2 - `forge-secrets` is still not wired into the runtime/core path.** The v1 crate map calls out `crates/secrets/` as the keychain/keystore abstraction (`prd-merged/01-core-runtime-prd.md:20-24`), and the new crate exposes the richer `SecretValue`/`SecretStore::get -> Result<Option<_>>` contract (`forge/crates/secrets/src/lib.rs:84-105`). But `forge-runtime` does not depend on it (`forge/crates/runtime/Cargo.toml:7-12`), redefines its own `SecretStore` and resolver (`forge/crates/runtime/src/net.rs:264-368`), and `forge-core` asks shells for `forge_runtime::SecretStore` factories (`forge/crates/core/src/workspace.rs:112-132`). This leaves the production path on a duplicate, narrower API and makes `forge-secrets` effectively test-only. Please either make runtime/core use `forge-secrets` directly, or remove/rename the crate until it is the real shared contract.

3. **P2 - Windows tests depend on a prebuilt native DLL that the solution never builds.** `NativeMethods` imports `forge_ffi` (`windows/src/Forge.Core/NativeMethods.cs:6-10`), while the test project only copies `forge_ffi.dll` if it already exists under `forge/target/...` (`windows/tests/Forge.Core.Tests/Forge.Core.Tests.csproj:24-27`). The `.sln` only builds the C# projects (`windows/Forge.Windows.sln:5-22`), and there is no MSBuild target/script that runs `cargo build -p forge-ffi` first. On a clean checkout or CI runner, `dotnet test windows/Forge.Windows.sln` can compile the wrapper but then fail at runtime with `DllNotFoundException`. Add a build target or documented CI step that builds the native crate for the active RID and copies the correct artifact before the tests run.

4. **P3 - The C# binding surface is hand-written despite the generated-binding requirement.** PRD PS-1 says shell bindings should be generated (UniFFI for C#, with C ABI adapter only where generated bindings are not mature) (`prd-merged/06-platform-shells-prd.md:8-11`), but the new Windows surface manually mirrors `CoreCommand`, `CoreResponse`, `CoreEvent`, roles, and JSON shape (`windows/src/Forge.Core/CoreDtos.cs:5-88`). This is likely to drift as `forge-domain` evolves. Either switch this layer to generated bindings or clearly mark it as a temporary smoke wrapper with drift tests against the Rust domain schema.

5. **P3 - `forge-secrets` docs still contradict the implemented missing-secret error.** The trait docs say `Ok(None)` is turned into `PermissionDenied` (`forge/crates/secrets/src/lib.rs:93-97`), but the resolver intentionally returns `RuntimeError` for an unknown secret (`forge/crates/secrets/src/lib.rs:219-228`), matching the SC-13 fixture behavior. Please align the doc comment with the actual resolution-failure contract.

6. **P3 - The commit range fails `git diff --check`.** `git diff --check 7deca9a6..a49c34ad` reports new blank lines at EOF in the FFI/Windows files, including `forge/crates/ffi/Cargo.toml:16`, `windows/src/Forge.Core/NativeMethods.cs:87`, and `windows/tests/Forge.Core.Tests/Forge.Core.Tests.csproj:29`. Trim those trailing blank lines so whitespace gates stay clean.

## Verification

- Passed: `cargo test -p forge-secrets -p forge-runtime -p forge-core -p forge-ffi`
- Passed: `cargo clippy -p forge-secrets -p forge-runtime -p forge-core -p forge-ffi --all-targets -- -D warnings`
- Passed: `cargo check -p forge-secrets -p forge-runtime --target wasm32-unknown-unknown`
- Failed: `git diff --check 7deca9a6..a49c34ad` (blank lines at EOF listed above)
- Failed in a throwaway archive: `cargo check -p forge-ffi --target wasm32-unknown-unknown --offline` hit `sqlite-wasm-rs`/clang target setup while building the `forge-core` dependency path; I did not count this as a primary FFI regression because the failure is below `forge-core`/storage rather than in the C ABI code itself.
- Blocked: `dotnet test windows/Forge.Windows.sln --no-restore` hit an MSBuild named-pipe `SocketException (13): Permission denied` in this sandbox before compilation; I stopped the stuck process.

Note: the old review 077 response-leg replay issue appears closed in `a49c34ad`; current `forge/crates/runtime/tests/net.rs:300` covers replaying a redacted response-leg denial as the same error.
