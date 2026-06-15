# 043 - Codex independent Claude-review fix pass

Date: 2026-06-15
Branch: `forge-m0a`
Agent: Codex, working independently from Claude

## Scope

Codex re-read the Claude review archive with subagents focused on server/FFI,
native/platform, and final closure reviews. This pass fixed the locally
actionable blockers and left only external/device/large-product-slice items as
explicit follow-up work.

## Local fixes committed

- `e5a2adf7 forge-server: harden bridge auth`
  - Token-gates `/bridge` and event draining.
  - Supports bearer and `x-forge-server-token` auth.
  - Derives `actor` and `workspace_id` server-side before dispatch.
  - Adds body-size cap, panic containment, structured 401/413 handling, and
    non-loopback bind guard.
- `28f240bc forge-ffi: prove file-backed reopen`
  - Adds a C-ABI persistence regression proving an applet install/run survives
    close and reopen against the same SQLite file.
- `f7e31c48 tests: compile forge ffi from c`
  - Extends the reference-host FFI packaging test with a generated C program
    that includes `forge_ffi.h`, links the release staticlib, calls exported
    symbols, and executes.
- `1b445021 native-ios: force-load forge ffi staticlib`
  - Adds `TERRANE_IOS_FORGE_FFI_STATICLIB` SwiftPM wiring and force-loads the
    exact `libforge_ffi.a` archive for iOS static-link proof.
  - Adds an opt-in device static-link test.
- `e28633ab docs: reclassify legacy zig references`
  - Marks v0.4 Zig-era docs as superseded legacy where they still contain
    historical core details.
  - Records the mobile applet-JS caveat in `IMPLEMENTATION_STATUS.md` instead
    of letting mobile appear silently complete.
- `598441a7 forge-ffi: update lockfile for tests`
  - Commits the lockfile update for the FFI persistence test dependency.
- `b9b16e8b tests: track mutable server bridge command`
  - Updates the reference-host source guard for the new mutable server command
    binding introduced by server-side actor/workspace derivation.

## Verification

- `cargo test --workspace --locked` - pass.
- `cargo clippy --workspace --all-targets --locked -- -D warnings` - pass.
- `cargo run -q -p forge-cli -- demo` - pass, including
  `REPLAY IDENTICAL: true`.
- `node --no-warnings tools/check-repo.mjs` - pass.
- `node --test --test-reporter=dot --no-warnings tools/reference-host/test/*.test.js`
  - pass when run outside the file sandbox so Node can bind loopback and
    SwiftPM can write its normal user caches.
- `TERRANE_IOS_DEVICE_STATIC_LINK=1 node --test --no-warnings tools/reference-host/test/ios-native-build.test.js`
  - pass.

## Remaining non-local or product-scope items

- Remote CI and real Linux/Windows host smoke evidence still need to run on
  their target environments.
- Mobile applet JavaScript execution remains a CR-12/mobile engine slice, not a
  small review fix. iOS/Android now report this as structured
  `PlatformUnavailable` behavior and `IMPLEMENTATION_STATUS.md` calls it out.
- The worktree still contains pre-existing unrelated local edits and untracked
  Claude/review artifacts. Codex did not stage or modify those except for this
  new closure note and the committed fix slices listed above.
