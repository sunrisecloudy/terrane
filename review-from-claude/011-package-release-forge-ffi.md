# Review 011 - package release Forge FFI

- **Slice goal:** Cut the standalone core-library release artifact path from legacy `zig-core` to the Rust `forge-ffi` crate, and move the core/server CI lane to Forge commands.
- **Reviewed:** working diff for release packaging, CI workflow, checker expectations, release packaging tests, and release docs.
- **Files changed:** `.github/workflows/ci.yml`, `tools/package-release.mjs`, `tools/reference-host/test/release-packaging.test.js`, `tools/check-repo.mjs`, `tools/README.md`, `docs/12_RELEASE_AND_CI.md`.
- **Review mode:** independent Codex/self-review. Claude Code Opus review remains waived by the user instruction on 2026-06-15 to work independently from Claude Code.
- **Commands run:** `node --test --no-warnings tools/reference-host/test/release-packaging.test.js` -> Forge FFI and Forge server passed, macOS DMG failed in sandbox; rerun unsandboxed -> passed; `node --no-warnings tools/check-repo.mjs` -> passed; `cargo test -p forge-ffi --locked` -> passed.

## Findings

No blocking findings.

- [P2] Native app package builders still copy `libzig_core` / `zig_core.dll`. Resolution: out of scope for this slice because the native host bridges still load the Zig ABI; leave them for per-host forge-ffi porting slices.
- [P2] `--build-forge-ffi` packages the current runner's Cargo host artifact, not the old Zig all-target cross-build matrix. Resolution: acceptable for this cutover step; cross-target/native bundle behavior should be driven by each host port and CI runner once the host bridges move to Forge.
- [P3] The full release-packaging test still needs an unsandboxed rerun for macOS `hdiutil create`. Resolution: reran the exact test unsandboxed; all runnable subtests passed with Linux/Windows skipped on macOS.

## Resolution status

- `tools/package-release.mjs` now exposes `--build-forge-ffi` and emits `forge-ffi-library` artifacts containing `forge_ffi.h` plus the host Cargo library outputs.
- The standalone CI artifact job is renamed to `forge-ffi-release-artifacts` and no longer sets up Zig.
- The main CI contract lane runs Forge workspace tests, clippy, and demo replay instead of `zig-core` / `server` Zig tests.
- `tools/check-repo.mjs` and release packaging tests enforce the new Forge FFI artifact surface.

## Follow-ups

- Port native macOS/Linux/Windows/iOS/Android hosts to `forge-ffi`, then replace the native bundle `libzig_core` / `zig_core.dll` copy steps.
- Decide whether dedicated cross-target Forge FFI artifacts remain useful after host-specific release jobs build their own native libraries.
