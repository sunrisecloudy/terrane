# Shared Cargo Cache

Terrane uses Git worktrees heavily. A normal Cargo workspace-local `target/`
directory makes every worktree pay a fresh Rust build cost, especially for host
crates that pull in native dependencies. Use the repo cache helpers and agent
hooks so all worktrees share the same compiled-artifact and compiler caches.

## Defaults

The shared setup lives in `scripts/cargo-cache-env.sh` and preserves any values
that are already set by the caller.

| Variable             | Default                                     |
| -------------------- | ------------------------------------------- |
| `CARGO_TARGET_DIR`   | `~/Library/Caches/terrane/cargo-target/all` |
| `SCCACHE_DIR`        | `~/Library/Caches/sccache`                  |
| `SCCACHE_CACHE_SIZE` | `40G`                                       |
| `RUSTC_WRAPPER`      | first `sccache` found on `PATH`             |

## One-Time Machine Setup

Install or update `sccache` with Homebrew:

```sh
brew update
brew upgrade sccache
sccache --version
```

Create the shared cache directories:

```sh
mkdir -p \
  "$HOME/Library/Caches/terrane/cargo-target/all" \
  "$HOME/Library/Caches/sccache"
```

If `sccache --show-stats` reports a client/server version mismatch after an
upgrade, stop the old server and let the new one start:

```sh
sccache --stop-server || true
source scripts/cargo-cache-env.sh
sccache --start-server
sccache --show-stats
```

The stats output should show the Homebrew `sccache` version and:

```text
Cache location  Local disk: ".../Library/Caches/sccache"
Max cache size  40 GiB
```

## Manual Usage

Source the environment before a shell session:

```sh
source scripts/cargo-cache-env.sh
cd rust
cargo test --workspace --locked
```

Or run one command through the wrapper:

```sh
scripts/with-cargo-cache.sh cargo test --workspace --locked
scripts/with-cargo-cache.sh cargo clippy --all-targets -- -D warnings
```

Inspect the active values without running Cargo:

```sh
scripts/with-cargo-cache.sh --print
```

## Agent Hooks

Project-level hooks keep Codex, Claude Code, and opencode on the same cache
convention.

- Codex reads `.codex/hooks.json`. The `PreToolUse` Bash hook calls
  `scripts/agent-hooks/cargo-cache-pretool.py` and rewrites Rust build/test
  commands to source `scripts/cargo-cache-env.sh` before running.
- Claude Code reads `.claude/settings.json` and uses the same `PreToolUse`
  rewriter.
- opencode loads `.opencode/plugins/terrane-cargo-cache.js`, which injects the
  same values through its `shell.env` plugin hook.

The rewriter intentionally targets Cargo commands that compile or test code:
`cargo test`, `cargo clippy`, `cargo check`, `cargo build`, `cargo run`,
`cargo doc`, `cargo bench`, `cargo install`, `cargo nextest`, and
`cargo llvm-cov`. It leaves non-build commands such as `cargo fmt --check` and
`cargo --version` untouched.

Codex may ask for one-time hook trust when it first sees `.codex/hooks.json`.
Trust the hook after reviewing the checked-in script; do not bypass hook trust
for arbitrary unreviewed hooks.

## Validation

Run these after changing the cache setup:

```sh
bash -n scripts/cargo-cache-env.sh scripts/with-cargo-cache.sh
python3 -m py_compile scripts/agent-hooks/cargo-cache-pretool.py
python3 -m json.tool .codex/hooks.json >/dev/null
python3 -m json.tool .claude/settings.json >/dev/null
node --check .opencode/plugins/terrane-cargo-cache.js
opencode debug config
scripts/with-cargo-cache.sh --print
sccache --show-stats
```

To test the Codex/Claude hook payload shape without running Cargo:

```sh
printf '%s\n' \
  '{"tool_input":{"command":"cd rust && cargo test --workspace --locked"}}' \
  | scripts/agent-hooks/cargo-cache-pretool.py \
  | python3 -m json.tool
```

The output should contain an `updatedInput.command` that sources
`scripts/cargo-cache-env.sh` before the original Cargo command.

## Notes

A single shared `CARGO_TARGET_DIR` can make concurrent worktrees wait on Cargo
file locks. That is usually still faster than fully cold worktree builds. If
parallel worktree builds become too noisy, split the target directory by
workspace while keeping the same `sccache` directory.
