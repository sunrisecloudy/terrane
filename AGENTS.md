# Terrane Agent Notes

Use the shared Cargo cache for Rust build and test commands:

```sh
scripts/with-cargo-cache.sh cargo test --workspace --locked
scripts/with-cargo-cache.sh cargo clippy --workspace --all-targets --locked -- -D warnings
```

Project hooks for Codex, Claude Code, and opencode apply the same cache
convention for agent-run shell commands. The shared defaults are:

- `CARGO_TARGET_DIR=~/Library/Caches/terrane/cargo-target/all`
- `SCCACHE_DIR=~/Library/Caches/sccache`
- `SCCACHE_CACHE_SIZE=40G`
- `RUSTC_WRAPPER=sccache`
