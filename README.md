# Terrane Rust Workspace

The shared Rust workspace for Terrane — rebuilt from scratch.

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
rust/          # the fresh Rust workspace (the only product code)
  crates/
    terrane-core/           # shared vocabulary + deterministic engine + host_runtime
    terrane-cap-*/          # standalone capabilities over terrane-cap-interface
    terrane-host/           # host services, `terrane` binary, C ABI, sync, preview, MCP
legacy/                # the previous build, swept aside intact as reference only
```

`legacy/` is read-only reference. We mine it for hard-won details (CRDT merge,
canonicalization, conformance cases) and adopt pieces deliberately — it is not a
dependency and not a foundation.

## Build

```sh
cd rust
cargo test
cargo run -p terrane-host --bin terrane -- help
```
