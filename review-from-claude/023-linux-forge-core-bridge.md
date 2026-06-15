# Slice review: Linux Forge core bridge

## Slice goal

Cut the Linux GTK/WebKitGTK host `core.step` path from the legacy
`libzig_core.so` buffer ABI to the Forge FFI CoreCommand ABI, while preserving
the host-derived app id permission check and executable-relative library lookup.

Per user instruction on 2026-06-15, this slice was implemented and reviewed
independently by Codex instead of requesting Claude Code review.

## Commit or working diff reviewed

Working diff before commit for the Linux Forge FFI bridge slice.

## Files changed

- `native/linux/Dockerfile`
- `native/linux/README.md`
- `native/linux/meson.build`
- `native/linux/src/forge_core_bridge.c`
- `native/linux/src/forge_core_bridge.h`
- `native/linux/src/zig_core_bridge.c` deleted
- `native/linux/src/zig_core_bridge.h` deleted
- `native/linux/src/dev_control_plane.c`
- `native/linux/src/web_bridge.c`
- `native/linux/src/web_bridge.h`
- `tools/check-repo.mjs`
- `tools/reference-host/test/linux-native-build.test.js`
- `tools/reference-host/test/linux-dev-control-source.test.js`
- `tools/reference-host/test/linux-docker-helper.test.js`
- `tools/reference-host/test/native-core-timeout-source.test.js`
- `tools/reference-host/test/runtime-capabilities-contract.test.js`

## Commands/tests run

- `node --test --no-warnings tools/reference-host/test/linux-native-build.test.js`
  - All Linux native tests skipped on macOS with "Linux native smoke only runs on Linux hosts".
- `node --test --no-warnings tools/reference-host/test/linux-dev-control-source.test.js`
- `node --test --no-warnings tools/reference-host/test/linux-docker-helper.test.js`
- `node --test --no-warnings tools/reference-host/test/native-core-timeout-source.test.js`
- `node --test --no-warnings tools/reference-host/test/runtime-capabilities-contract.test.js`
- `cargo test -p forge-ffi --locked`
- `node --no-warnings tools/check-repo.mjs`

## Review findings

- No blocker found in the Linux bridge: `core.step` now builds a
  `legacy.core_step` CoreCommand envelope and unwraps the Forge `payload` into
  the generated-app-visible bridge result.
- No blocker found in permission handling: the Linux host still rejects a
  caller-provided mismatched `app` value before calling Forge.
- No blocker found in dynamic loading: `TERRANE_FORGE_FFI_SO`, executable-local
  `libforge_ffi.so`, repo-local debug/release artifacts, and `/usr/local/lib`
  are tried in order.
- Non-blocking note: the Linux packaged release smoke is explicitly skipped
  under launch mode until Phase 2.6 repoints `tools/package-release.mjs` from
  the legacy Linux `libzig_core.so` artifact to the Forge FFI library.
- Non-blocking note: the actual Linux Meson build/smoke was not run in this
  macOS environment; the source tests and checker validate the contract, and the
  Docker smoke now installs Rust and builds Forge FFI into a scratch
  `CARGO_TARGET_DIR` for Linux hosts.

## Resolution status

- All findings resolved or explicitly documented.
- `tools/check-repo.mjs` reports Linux native core status as `core=forge-ffi`.
- Remaining Linux `libzig_core.so` references are limited to the deferred
  package-release smoke path owned by Phase 2.6.

## Follow-up tasks

- Repoint Linux release packaging to stage `libforge_ffi.so` in Phase 2.6.
- Run the Linux native smoke in Docker or on a Linux host once the environment is
  available.
