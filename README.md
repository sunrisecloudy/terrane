# Terrane

The root Rust workspace for Terrane — rebuilt from scratch.

Terrane is a local-first platform for personal apps. This repository is a
deliberate reset: instead of growing the platform outward (sync, server, UI,
native hosts, FFI, policy, …), we start from the one thing that is actually _the
system_ and add nothing until a real need forces it.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the high-level layer model (apps ▸
host ▸ `terrane-core` engine crate ▸ resources), and
[docs/APP_API.md](docs/APP_API.md) for the JavaScript API an app's backend and
UI get (drift-guarded by a test).

## The one rule

Everything goes through a single front door and a single shape:

```
argv ──▶ terrane-host::cli ──▶ Request ──▶ terrane-core ──▶ [Event] ──▶ State
                                          │                         │
                                          └── persist log ──────────┘
                                                    │
                                            replay ─┘  (must reproduce identical State)
```

- The **CLI never touches data directly.** It only speaks requests to the core.
- The core is **deterministic and replayable**: re-applying the event log
  reproduces identical state. That property is what earns the word _core_.
- Platform effects (sync, network, native shells, servers) are _layers_ added
  later, at the edge — never inside the core.

## Layout

```
Cargo.toml     # root Cargo workspace for all Rust crates and host adapters
rust/
  crates/
    terrane-core/           # shared vocabulary + deterministic engine + host_runtime
    terrane-cap-*/          # standalone capabilities over terrane-cap-interface
    terrane-host/           # host services, `terrane` binary, C ABI, sync, preview, MCP
host/
  cli/                      # CLI adapter package
  mcp/                      # MCP adapter package
  web/                      # web adapter package
apps/                       # JS app bundles (todo, chat, …), each with i18n/<code>.json
i18n/system/                # host/shell chrome translation catalogs, per language
```

## Localization

Terrane detects the user's language (web `Accept-Language` / an in-shell picker;
macOS system language) and localizes the host chrome and apps. Translations are
stored once in a shared **public KV** bucket (`i18n/<code>/<domain>.<key>`) and
reused across every app and platform; apps read the active locale + a message
bundle through `window.terrane` (`getLocale`/`getDir`/`t`). Ship catalogs as
`i18n/system/<code>.json` and `apps/<id>/i18n/<code>.json`; hosts seed them on
startup (or `terrane i18n import <dir>`). Details: [docs/APP_API.md](docs/APP_API.md).
Supported: `en, es, zh-Hans, ar, pt-BR, fr, de, ja, id, th-TH, ko, vi`.

## Build

```sh
cargo test
cargo run -p terrane-host --bin terrane -- help
```

For linked worktrees, use the shared Cargo/sccache environment so Rust build
artifacts are reused across checkouts:

```sh
source scripts/cargo-cache-env.sh
cargo test
```

Or run a single command through the wrapper:

```sh
scripts/with-cargo-cache.sh cargo test --workspace --locked
```

Codex, Claude Code, and opencode have project hooks/plugins that apply this same
cache convention to agent-run shell commands.

See [docs/CARGO_CACHE.md](docs/CARGO_CACHE.md) for the full setup and
troubleshooting runbook.
