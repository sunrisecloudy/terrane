# terrane-core

The small, careful core of Terrane — rebuilt from scratch.

Terrane is a local-first platform for personal apps. This repository is a
deliberate reset: instead of growing the platform outward (sync, server, UI,
native hosts, FFI, policy, …), we start from the one thing that is actually
*the system* and add nothing until a real need forces it.

See [ARCHITECTURE.md](ARCHITECTURE.md) for the high-level layer model
(apps ▸ native host ▸ terrane-core ▸ resources), and [docs/APP_API.md](docs/APP_API.md)
for the JavaScript API an app's backend and UI get (drift-guarded by a test).

## The one rule

Everything goes through a single front door and a single shape:

```
argv ──▶ terrane-cli ──▶ Command ──▶ terrane-core ──▶ [Event] ──▶ State
                                          │                         │
                                          └── persist log ──────────┘
                                                    │
                                            replay ─┘  (must reproduce identical State)
```

- The **CLI never touches data directly.** It only speaks Commands to the core.
- The core is **deterministic and replayable**: re-applying the event log
  reproduces identical state. That property is what earns the word *core*.
- Platform effects (sync, network, native shells, servers) are *layers* added
  later, at the edge — never inside the core.

## Layout

```
terrane-core/          # the fresh Rust workspace (the only product code)
  crates/
    terrane-domain/    # vocabulary: Command, Event, Id, Error, State — pure, no I/O
    terrane-core/      # engine: apply(Command) -> [Event] -> State; persist; replay
    terrane-cli/       # front door binary `terrane`
legacy/                # the previous build, swept aside intact as reference only
```

`legacy/` is read-only reference. We mine it for hard-won details (CRDT merge,
canonicalization, conformance cases) and adopt pieces deliberately — it is not a
dependency and not a foundation.

## Build

```sh
cd terrane-core
cargo test
cargo run -p terrane-cli -- help
```
