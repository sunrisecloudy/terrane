# Commit Review: plan/fixture drop + runtime.run seed fix

Reviewed commits:

- `776470ed` - plans: Windows (WinUI3/C#) + Linux/WSL implementation plans; land review history + Codex T021/T022
- `7bf1a777` - forge-core: close review 037 #1/#2 (time_start i64 bound + commands.md spec)

## Findings

1. **P1 - `.gitignore` now unignores secrets and generated artifacts.** `776470ed` replaces the previous ignore set with only `.forge-wf/` (`.gitignore:1`). That drops ignores for `.env`, `.env.*`, `*.token`, `*.db`, SQLite files, logs, build/dist/coverage, `node_modules`, legacy build caches, and native build output, while this worktree already has generated artifact directories present. Restore the old ignore list and add `.forge-wf/` instead of replacing it.

2. **P2 - Default `runtime.run` can still feed an overflowing `time_start` into the logical clock.** The new guard only validates explicit payload overrides (`forge/crates/core/src/workspace.rs:652`). Normal no-override runs still take `derive_seeds(...)` (`forge/crates/core/src/workspace.rs:308`), which returns a full-width `u64` time seam (`forge/crates/core/src/workspace.rs:694`), and `LogicalClock::new` still casts with `time_start as i64` (`forge/crates/runtime/src/recorder.rs:74`). If the derived value is above `i64::MAX`, the recorded seam and `ctx.time.now()` disagree via wraparound. Bound the derived time seed too, or make the clock conversion fallible and add a no-override regression.

3. **P2 - Windows FFI crate/DLL naming is internally inconsistent.** The overview names `crates/ffi-cabi` and `forge_core_ffi.dll` (`window-plan/00-OVERVIEW.md:69`), while the build doc names `forge-ffi` (`window-plan/01-BUILD-AND-FFI.md:122`), places it at `forge/crates/ffi` (`window-plan/01-BUILD-AND-FFI.md:129`), and builds `forge_ffi.dll` (`window-plan/01-BUILD-AND-FFI.md:194`). Pick one canonical crate path, package name, and DLL basename before Claude starts implementation; otherwise CI/package work can create the wrong artifact.

4. **P2 - Windows toolchain docs claim a Rust pin the repo does not have.** `forge/rust-toolchain.toml:2` pins `channel = "stable"` and only comments that it was built on 1.96.0 (`forge/rust-toolchain.toml:3`), but the Windows plan says Rust `1.96.0` is pinned by `rust-toolchain.toml` (`window-plan/00-OVERVIEW.md:266`) and expects `rustup show` to report that exact version (`window-plan/00-OVERVIEW.md:277`). Either pin `1.96.0` for real or rewrite the plan as "stable, minimum 1.93, validated on 1.96.0" so Windows builds do not drift silently.

5. **P2 - T021/T022 are delivered but still marked `requested`.** The board protocol says Codex should set delivered tasks to `done` (`task-between-claude-and-codex/README.md:9`), and this commit lands the requested spec/fixture paths. However the board still lists T021/T022 as `requested` (`task-between-claude-and-codex/README.md:46`, `task-between-claude-and-codex/README.md:47`), each task file frontmatter still says `status: requested` (`task-between-claude-and-codex/T021-query-mutation-vectors.md:2`, `task-between-claude-and-codex/T022-dynamic-index-vectors.md:2`), and the same README says there are no outstanding Codex requests (`task-between-claude-and-codex/README.md:72`). Mark them `done` and add a short `## Result` note, or Claude/automation will keep seeing them as open.

6. **P3 - New fixture suites are not wired into executable tests.** The query and dynamic-index manifests are committed (`forge/fixtures/query/manifest.json:2`, `forge/fixtures/indexes/manifest.json:2`), but `git grep` finds the suite names only in the manifests/task docs, not in any `forge/crates` test harness. At minimum add a manifest-loader test now; ideally wire semantic assertions when the query/index manager lands, so these contracts cannot rot while the implementation is built.

## Verification

- `git show --check 776470ed` fails due trailing whitespace in committed `review/018` through `review/024` lines; clean those before treating the commit as hygiene-clean.
- `git show --check 7bf1a777` passes.
- Parsed all 28 new query/index JSON fixture files successfully.
- Local `cargo test --locked -p forge-core` could not verify this checkout because unrelated dirty `forge-storage` work currently fails to compile on missing query helper symbols; do not treat that as caused by these two commits without checking the clean commit/worktree.
