# Linux milestones & acceptance gates

**Project:** Forge (codename) — Linux/WSL headless path
**Scope of this doc:** the ordered, implementable milestones the Linux/WSL target ships through, each with concrete deliverables, exact acceptance checks, and a mapping back to `prd-merged/` requirements.
**Audience:** an engineer running this on WSL2 (Ubuntu 22.04/24.04) or a bare Linux box.

> **Linux's v1 role is the HEADLESS path.** Build + test the Rust workspace, run the
> `forge` CLI harness as the dev loop, and — when `forge-server` lands — run the embedded
> sync server as a daemon (systemd + Docker). **There is no Linux GUI in v1** (decision D5,
> PS-13). The GTK4/libadwaita desktop app is an explicitly separate **post-GA track**,
> sketched in §"Post-GA: GTK GUI track" and gated out of every milestone below.
> WSL is a **first-class build/run target**, not an afterthought.

The Linux milestones are numbered **L0…L4**. They are deliberately decoupled from the
product-wide M-milestones (`prd-merged/00` §11) but track them: **L0/L1 ride on M0a (the
spine that exists today); L2 maps to the M0b sync seam; L3/L4 map to M2's embedded-server
GA quality on Linux.**

| Linux milestone | One-line goal | Rides on | Primary PRD reqs |
|---|---|---|---|
| **L0** | WSL/Linux build green: workspace builds + tests, `forge demo` replays | M0a (done) | CR-12, §10 M0; PS-4 |
| **L1** | CLI harness is the dev loop: install/run/replay applets from a real applet dir | M0a→M0b | PS-5, CR-A2, CR-8/CR-9 |
| **L2** | `forge-server` crate scaffold + in-process client↔server sync test on Linux | M0b | SS-1/SS-2/SS-4, §9 M0; CR §2 (`sync/`, `server/`) |
| **L3** | Embedded server **daemon** (systemd + Docker) serving a LAN client | M2 | SS-15/SS-16/SS-19, SS-21/SS-22 |
| **L4** | Self-host packaging (single binary + Docker image published) + Linux CI runner | M2→M5 | SS-19, PS-13; §9 acceptance; PRD 09 gates |

Each milestone's "Exit gate" is a single command (or short script) that returns exit 0.
Treat it as the CI gate for that milestone.

---

## Baseline: what already runs on this machine today

The `forge/` Rust workspace **already builds and runs the M0a spine headlessly**. This is
the literal starting point L0 hardens for Linux/WSL; nothing below assumes greenfield.

- **Toolchain** (`forge/rust-toolchain.toml`): `channel = "stable"`, pinned by comment to
  **≥ 1.93** (libsqlite3-sys 0.38+ needs `cfg_select!`); validated on **1.96.0**.
- **Workspace** (`forge/Cargo.toml`, resolver 2): 11 crates —
  `domain, storage, crdt, schema, policy, runtime, pipeline, ui, core, cli, testkit`.
  There is **no `sync/` or `server/` crate yet** (L2 scaffolds them per CR §2).
- **Native C deps that gate the Linux build** (this is the whole risk surface):
  - `forge-storage` → `rusqlite = { version = "0.40.1", features = ["bundled"] }` — compiles
    SQLite from vendored C; **needs a C compiler**, not a system libsqlite3.
  - `forge-runtime` → `rquickjs = "0.12"` **only** under
    `target.'cfg(not(target_arch = "wasm32"))'` — vendors QuickJS C; **needs a C compiler**.
    rquickjs is deliberately **not** compiled for `wasm32-unknown-unknown`.
- **The spine the CLI drives** (`forge/crates/cli/src/lib.rs`):
  `TS → SWC transpile (forge-pipeline) → QuickJS realm (forge-runtime) → capability ctx
  (forge-policy) → SQLite write (forge-storage) → UI tree patch (forge-ui) → deterministic
  RunRecord → replay (byte-identical)`. Entry point: `forge demo` runs the embedded
  `examples/notes-lite/` applet and **exits non-zero if the run fails or replay diverges**.
- **wasm posture:** `wasm32-unknown-unknown` + `wasm32-wasip1` targets are installed; the
  pure crates (`domain, schema, ui, pipeline`) are wasm-clean; `runtime`/`storage` are
  native-only. The **wasm *runtime* backend (QuickJS-WASM) is a known gap** — see Risks.

---

## L0 — WSL/Linux build green

**Goal:** a clean WSL2/Linux checkout builds the whole workspace, passes the test suite,
and `forge demo` prints the spine report with `REPLAY IDENTICAL: true`. This is the
"the machine can build Forge" gate and the prerequisite for every later milestone.

### Deliverables

1. **Documented host bootstrap** for Debian/Ubuntu (WSL default) and Fedora/RHEL.

   Ubuntu/Debian (WSL2 default — Ubuntu 22.04/24.04):
   ```bash
   sudo apt-get update
   sudo apt-get install -y \
       build-essential \   # gcc, g++, make — required by rusqlite & rquickjs vendored C
       clang \             # rquickjs/bindgen path prefers clang; libclang for bindgen
       libclang-dev \      # bindgen needs libclang at build time
       pkg-config \
       cmake \             # some transitive C deps probe cmake
       git \
       curl ca-certificates
   ```
   Fedora/RHEL/CentOS-stream:
   ```bash
   sudo dnf install -y \
       gcc gcc-c++ make \
       clang clang-devel llvm-devel \
       pkgconf-pkg-config cmake git curl ca-certificates
   ```
   Rust (both distros) — install via rustup so `rust-toolchain.toml` auto-selects the pin:
   ```bash
   curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
   . "$HOME/.cargo/env"
   rustup target add wasm32-unknown-unknown wasm32-wasip1
   ```
   > rustup reads `forge/rust-toolchain.toml` and pins to stable ≥ 1.93 automatically on the
   > first `cargo` invocation inside the repo. Do **not** rely on the distro `rustc`
   > (Ubuntu 22.04 ships 1.75 — too old for `cfg_select!`).

2. **A `linux-plan/scripts/l0-verify.sh`** (or doc-block) that runs the gate end-to-end.
3. **README/CONTRIBUTING note** that WSL2 is supported and clangd/libclang is mandatory
   (the single most common WSL build failure is missing `libclang-dev`).

### Exact acceptance check (Exit gate L0)

Run from the repo root; **all three commands must exit 0** and the last must print the
replay line:
```bash
# 1. whole workspace + wasm-clean crates compile
cargo build --workspace --locked
cargo build -p forge-pipeline -p forge-ui -p forge-domain -p forge-schema \
    --target wasm32-unknown-unknown --locked

# 2. full test suite green (unit + the cli e2e spine test)
cargo test --workspace --locked

# 3. the spine actually replays deterministically end-to-end
cargo run -p forge-cli --bin forge -- demo | tee /tmp/forge-demo.out
grep -q '^REPLAY IDENTICAL: true$' /tmp/forge-demo.out
```
A one-shot gate:
```bash
set -euo pipefail
cargo build --workspace --locked
cargo test  --workspace --locked
cargo run -p forge-cli --bin forge -- demo | grep -q '^REPLAY IDENTICAL: true$'
echo "L0 GREEN"
```

### PRD mapping

- `prd-merged/01` **CR-12** + §10 *"M0: full loop … green headlessly via the CLI harness
  on macOS/**Linux**/WASM CI targets"* — Linux is a named M0 CI target; L0 is that target
  going green on a real Linux/WSL box.
- `prd-merged/06` **PS-4** (per-platform conformance gates: engine conformance, data
  fixtures, platform smoke of the demo workspace) — L0 is the Linux "platform smoke".
- `prd-merged/00` §11 **M0a** exit (*"Spine demo green in CI on macOS/Linux and WASM"*).

---

## L1 — CLI harness usable as the dev loop

**Goal:** turn `forge` from a one-shot `demo` into the **real dev loop** the PRD promises
(PS-5): install an applet **from a directory on disk**, run it with arbitrary input, list
the resulting records, and replay a recorded run — all headless, all offline. Wire it to a
**sample applet dir** so a developer's edit→run→inspect cycle works without writing Rust.

### Deliverables

1. **Subcommands on the `forge` binary** (extend `forge/crates/cli/src/main.rs`; the heavy
   lifting already exists as library fns in `forge/crates/cli/src/lib.rs` —
   `install`, `handle`, `run_demo`, `list_records`). Replace the hand-rolled arg match with
   a small parser (`clap` is acceptable here; it is shell-only, no business logic, PS-1/CR-A1
   are not violated because the CLI still calls the core only via `CoreCommand`). Surface:
   ```text
   forge demo                                   # unchanged: embedded notes-lite + replay
   forge new <dir>                              # scaffold an applet dir (manifest + src/main.ts)
   forge install <applet-dir> [--workspace F]   # applet.install from disk → a workspace .sqlite
   forge run <applet-dir|applet-id> --input '<json>' [--workspace F]   # runtime.run
   forge records <collection> [--workspace F]   # query.execute, prints rows
   forge replay <run-id> [--workspace F]        # runtime.replay, asserts byte-identical
   forge open <workspace.sqlite>                # workspace.open metadata dump
   ```
   These map 1:1 onto already-implemented commands (`spec/commands.md` rows marked **M0a**:
   `workspace.open, applet.install, runtime.run, runtime.replay, query.execute,
   record.put/patch, schema.apply_change`).

2. **Persistent on-disk workspace.** Today `WorkspaceCore::in_memory("ws-demo")` is the only
   path the CLI uses. Add `WorkspaceCore::open(path: &Path)` wiring so `--workspace
   ./scratch.sqlite` opens/creates a real portable SQLite workspace file
   (`forge-storage::Store` already opens on a path; this is plumbing, not new storage code).
   This makes runs survive across invocations — the actual dev loop.

3. **A sample applet directory** committed at `linux-plan/sample-applets/notes-lite/`
   (or reuse `forge/examples/notes-lite/`), with a 5-line README showing the loop:
   ```bash
   forge install examples/notes-lite --workspace ./scratch.sqlite
   forge run     notes-lite --input '{"title":"Buy milk"}' --workspace ./scratch.sqlite
   forge records notes --workspace ./scratch.sqlite          # → the stored note
   forge replay  <run-id-from-run> --workspace ./scratch.sqlite
   ```

4. **Non-zero exit semantics preserved**: any `CoreError` (rejected eval, capability denial,
   limit exceeded) maps to a non-zero exit with the typed error printed (the `handle()`
   helper already converts a non-`ok` `CoreResponse` into its `CoreError`).

### Exact acceptance check (Exit gate L1)

```bash
set -euo pipefail
WS=$(mktemp -d)/scratch.sqlite
BIN="cargo run -q -p forge-cli --bin forge --"

# install from a real dir, run with input, read it back, replay it
$BIN install examples/notes-lite --workspace "$WS"
RUN_JSON=$($BIN run notes-lite --input '{"title":"Buy milk"}' --workspace "$WS" --json)
echo "$RUN_JSON" | grep -q '"ok":true'
RUN_ID=$(echo "$RUN_JSON" | python3 -c 'import sys,json;print(json.load(sys.stdin)["run_id"])')

# the SQLite write persisted across process boundaries
$BIN records notes --workspace "$WS" | grep -q 'Buy milk'

# deterministic replay of the persisted run is byte-identical
$BIN replay "$RUN_ID" --workspace "$WS" | grep -q 'replays_identically: true'

# a hostile applet is contained (exit non-zero, typed error)
$BIN install ./linux-plan/sample-applets/evil-eval --workspace "$WS" && {
  echo "FAIL: eval applet should have been rejected"; exit 1; } || true
echo "L1 GREEN"
```
Plus a Rust integration test under `forge/crates/cli/tests/` that drives the same flow
against a `tempfile`-backed workspace (mirrors the existing
`run_demo_drives_the_whole_spine` test but through the on-disk path).

### PRD mapping

- `prd-merged/06` **PS-5** verbatim: *"create workspace, install applet from disk, run
  pipeline stages, simulate UI events, assert golden trees, … replay deterministic runs.
  It is the SDK's ancestor."* L1 is PS-5 reaching usable shape on Linux.
- `prd-merged/01` **CR-A2** command catalog (the M0a-marked commands) and **CR-8/CR-9**
  (deterministic run records + replay).
- `prd-merged/00` §6 (template-first strategy) — the CLI harness is the reference shell every
  later platform implements against.

---

## L2 — `forge-server` crate scaffold + in-process client↔server sync test

**Goal:** stand up the **sync seam** on Linux. Per CR §2 the workspace is missing two crates
— `sync/` (client+server protocol) and `server/` (`forge-server`: cloud + embedded). L2
scaffolds them and proves the **M0b headless guarantee**: *client and server run in one test
process and converge under simulated partition* (SS §9 / §10 "M0").  No network sockets yet —
this is the in-process round-trip that makes sync CI-testable from day one.

### Deliverables

1. **Two new crates** added to `forge/Cargo.toml` members (matching CR §2 names):
   ```text
   crates/sync/      # client + server sync protocol (PRD 03): frame types, handshake,
                     #   Loro frontier exchange, chunk request/response, ack
   crates/server/    # forge-server: an in-process Server that owns workspace docs and
                     #   speaks the sync frames; the embedded/cloud deployment seam
   ```
   `forge-sync` depends on `forge-domain` + `forge-crdt` (Loro is already a dependency of
   `forge-crdt`, exposed only through the `CrdtDoc` trait — `export_snapshot`/`import`, which
   is exactly the frontier/chunk primitive SS-1 needs). `forge-server` depends on
   `forge-sync` + `forge-core` + `forge-policy` (RBAC must be enforced **server-side** before
   apply — SS-7).

2. **Transport-agnostic frame types** (SS-2/SS-3): a `Frame` enum with the SS-2 message kinds
   — `Hello, Capabilities, FrontierSummary, ChunkRequest, ChunkResponse, SnapshotOffer,
   SnapshotResponse, LiveUpdate, Ack, ConflictNotice, PermissionDenied, ResyncRequired`.
   Serde-encodable; **no socket/tokio dependency in the frame layer** so the same frames ride
   any transport later (SS-3). An `in_process` transport = a pair of channels.

3. **Server-side RBAC gate** (SS-7/SS-9): every inbound `LiveUpdate`/`ChunkResponse` is
   validated through `forge-policy` against the actor's role **before** the update is folded
   into the doc; a rejection emits `PermissionDenied`, never a silent apply. CRDT convergence
   is never a substitute for authorization.

4. **The in-process sync test** (`forge/crates/server/tests/sync_in_process.rs`): two
   `WorkspaceCore`s (peer A = client, peer B = server-hosted), each makes concurrent
   `record.put`s, frames are exchanged through the in-process transport with a **simulated
   partition** (drop/delay/reorder/duplicate a window of frames), and after reconnect both
   materialize a **byte-identical** projection (SS-4/SS-9; leans on `forge-crdt`'s existing
   "concurrent edits converge byte-identically" property, DL-9).

### Exact acceptance check (Exit gate L2)

```bash
set -euo pipefail
cargo build -p forge-sync -p forge-server --locked

# the headless in-process round-trip + partition sim must converge with zero divergence
cargo test -p forge-server --test sync_in_process -- --nocapture

# the wasm-clean crates stay wasm-clean (frame layer must not pull tokio/sockets)
cargo build -p forge-sync --target wasm32-unknown-unknown --locked

# permission-monotonicity / RBAC-before-apply property test is green
cargo test -p forge-server rbac_before_apply -- --nocapture
echo "L2 GREEN"
```

### PRD mapping

- `prd-merged/03` **SS-1** (per-document CRDT sync, Loro version-vector/frontier exchange),
  **SS-2** (handshake + message kinds), **SS-3** (transport-agnostic frames), **SS-4**
  (offline-first queue + reconcile), **SS-7/SS-9** (server-side RBAC before apply,
  permission monotonicity), **§9** *"M0: client↔server in-process round-trip with partition
  simulation green headlessly"*.
- `prd-merged/01` **CR §2** crate layout — L2 is where `sync/` and `server/` finally exist.
- `prd-merged/00` §11 **M0b** (*"in-process client↔server sync"*).

---

## L3 — Embedded server daemon (systemd + Docker) serving a LAN client

**Goal:** wrap `forge-server` as a **real daemon process on Linux** that a separate client
(the `forge` CLI on another host, or a browser client) connects to over the **network** and
syncs a workspace with. This is the first time sync leaves a single process. It is the Linux
realization of the embedded server (SS-15..19), run **headless** as a systemd unit and as a
Docker container.

### Deliverables

1. **A `forge-serverd` binary** (a `[[bin]]` in `crates/server/`, or a thin `crates/serverd`)
   that:
   - opens/creates a workspace SQLite file (reuses L1's on-disk `WorkspaceCore`),
   - binds a **WebSocket/TLS** listener (SS-1: WS, TLS 1.3, binary frames ≤ 256 KB,
     resumable) using `tokio` + `tokio-tungstenite` + `rustls`; the SS-2/SS-3 frame types
     from L2 are serialized onto the socket unchanged,
   - enforces **device-token pairing** admission (SS-6: QR/short-code; **no anonymous LAN
     access**) and **server-side RBAC** (SS-7, carried over from L2),
   - exposes a **local-only status endpoint** (SS-22) and structured logs with **no document
     content** (SS-22),
   - advertises presence over **mDNS** on LAN (SS-16: advertises presence only, never grants
     access).
   - **TLS even on LAN** with a pinned self-signed cert exchanged at pairing — no TOFU
     (SS-21).

2. **A systemd unit** `packaging/systemd/forge-serverd.service`:
   ```ini
   [Unit]
   Description=Forge embedded sync server
   After=network-online.target
   Wants=network-online.target

   [Service]
   Type=notify
   User=forge
   Group=forge
   ExecStart=/usr/local/bin/forge-serverd \
       --workspace /var/lib/forge/workspace.sqlite \
       --listen 0.0.0.0:8443 \
       --tls-cert /var/lib/forge/cert.pem --tls-key /var/lib/forge/key.pem
   Restart=on-failure
   RestartSec=2
   # hardening (SS-21/SS-22, PRD 07): least privilege
   NoNewPrivileges=true
   ProtectSystem=strict
   ProtectHome=true
   ReadWritePaths=/var/lib/forge
   PrivateTmp=true

   [Install]
   WantedBy=multi-user.target
   ```
   With `WSL caveat`: on WSL2 there is no systemd by default on older images; document the
   `systemd=true` switch in `/etc/wsl.conf` (Win11 + recent WSL supports it), and provide a
   `nohup forge-serverd …` / tmux fallback for images without systemd.

3. **A Dockerfile** (multi-stage; the C-build deps live only in the builder stage):
   ```dockerfile
   # ---- builder ----
   FROM rust:1.96-bookworm AS build
   RUN apt-get update && apt-get install -y --no-install-recommends \
       clang libclang-dev pkg-config cmake && rm -rf /var/lib/apt/lists/*
   WORKDIR /src
   COPY . .
   RUN cargo build --release -p forge-server --bin forge-serverd --locked

   # ---- runtime (slim; no toolchain, no libclang) ----
   FROM debian:bookworm-slim
   RUN apt-get update && apt-get install -y --no-install-recommends \
       ca-certificates && rm -rf /var/lib/apt/lists/*
   COPY --from=build /src/target/release/forge-serverd /usr/local/bin/
   VOLUME /var/lib/forge
   EXPOSE 8443
   ENTRYPOINT ["/usr/local/bin/forge-serverd"]
   CMD ["--workspace","/var/lib/forge/workspace.sqlite","--listen","0.0.0.0:8443"]
   ```
   > Because `rusqlite`/`rquickjs` vendor their C and link statically, the **runtime image
   > needs no SQLite/QuickJS system packages** — only `ca-certificates` for TLS. This is the
   > payoff of the bundled-C choice and what keeps the image slim. (See Risks for the
   > glibc-vs-musl / Alpine note.)

4. **A LAN smoke harness**: start the daemon (systemd or `docker run`), pair a device token,
   then drive the `forge` CLI from a second host/container as a sync client against it.

### Exact acceptance check (Exit gate L3)

```bash
set -euo pipefail
# build + run the daemon in a container
docker build -t forge-serverd:l3 .
docker run -d --name forge-l3 -p 8443:8443 -v forge-data:/var/lib/forge forge-serverd:l3

# pair a device token (no anonymous access — SS-6)
TOKEN=$(forge pair --server wss://localhost:8443 --code <short-code>)

# a second client syncs a workspace over the network; a record made here…
forge run notes-lite --input '{"title":"from client A"}' --server wss://localhost:8443 --token "$TOKEN"

# …appears on the server's workspace, and an unauthorized op is rejected before apply
forge records notes --server wss://localhost:8443 --token "$TOKEN" | grep -q 'from client A'
forge run notes-lite --input '{"title":"x"}' --server wss://localhost:8443 --token "$VIEWER_TOKEN" \
  2>&1 | grep -q 'permission_denied'   # Viewer cannot write (SS-7)

# server logs contain NO document content (SS-22)
docker logs forge-l3 | grep -q 'from client A' && { echo "FAIL: doc content in logs"; exit 1; } || true
echo "L3 GREEN"
```
Plus a **mini-soak** (scaled-down SS §9): 2 clients, randomized partition/restart for 10 min
→ zero divergence, zero acked-write loss. (Full 7-day 50-workspace soak is an M2 product gate,
not an L3 gate.)

### PRD mapping

- `prd-merged/03` **SS-15** (embedded server: configurable port, status, access logs,
  role-based admission), **SS-16** (mDNS LAN advertise + relay reach), **SS-1** (WS/TLS
  transport), **SS-6** (device-token pairing, no anonymous LAN), **SS-21** (TLS-everywhere,
  pinned self-signed at pairing, no TOFU), **SS-22** (local status page, no doc content in
  logs). **SS-19** *"same crate … as a daemon"*.
- `prd-merged/06` **PS-13** (*"Same `server` crate as a CLI/daemon (SS-19): self-host sync,
  backups, …"*).
- `prd-merged/03` §9 *"Embedded server … serves N active members"* (Linux realization).

---

## L4 — Self-host packaging + CI on a Linux runner

**Goal:** make the headless server **distributable** — a single self-contained binary and a
published Docker image — and put the whole Linux path under **CI on a Linux runner** so L0…L3
stay green forever. This is the "ship the self-host story" milestone (SS-19) and the Linux
slice of the M5 GA quality bar.

### Deliverables

1. **Single-binary release artifacts** for the two mainstream Linux ABIs:
   - `x86_64-unknown-linux-gnu` (glibc; the default, smallest build effort), and
   - `x86_64-unknown-linux-musl` (fully static; runs on Alpine and minimal containers).
     The musl build needs `musl-tools` and a musl-capable C path for the vendored SQLite/
     QuickJS:
     ```bash
     sudo apt-get install -y musl-tools clang
     rustup target add x86_64-unknown-linux-musl
     CC=clang cargo build --release -p forge-server --bin forge-serverd \
         --target x86_64-unknown-linux-musl --locked
     ```
   - `aarch64-unknown-linux-gnu` for ARM servers/Raspberry Pi (cross or native runner).
   Each binary ships with a SHA-256 sum and (post-signing) a detached signature.

2. **A published, multi-arch Docker image** (`linux/amd64` + `linux/arm64`) built with
   `docker buildx`, tagged by release version, pushed to a registry (GHCR/Docker Hub).
   Reuses the L3 multi-stage Dockerfile.

3. **An admin/backup story** wired into the binary (SS-19 / SS-17): `forge-serverd backup
   <out.tar>` and `restore`, plus the SQLite workspace file *is itself* the portable backup
   (the export format spec, DL-24). Document a cron/systemd-timer backup.

4. **Linux CI workflow** (`.github/workflows/` — a new `linux-ci.yml`; the repo already has a
   `release.yml`) running on `ubuntu-latest`:
   - installs the host deps (clang/libclang-dev/build-essential),
   - runs the **L0 gate** (`build --workspace`, `test --workspace`, `forge demo` replay),
   - runs the **L2 gate** (in-process sync test),
   - builds the gnu + musl release binaries and the Docker image,
   - on a tag, publishes the binaries + image (the L4 artifacts).
   This is the Linux entry of the *"M0 exit: … green on macOS/Linux/WASM CI targets"* gate.

### Exact acceptance check (Exit gate L4)

```bash
set -euo pipefail
# static musl binary builds and has no dynamic deps (runs anywhere)
CC=clang cargo build --release -p forge-server --bin forge-serverd \
    --target x86_64-unknown-linux-musl --locked
file target/x86_64-unknown-linux-musl/release/forge-serverd | grep -q 'statically linked'
ldd  target/x86_64-unknown-linux-musl/release/forge-serverd 2>&1 | grep -q 'not a dynamic executable'

# the published image runs the daemon and a fresh client completes the loop
docker run --rm forge-serverd:release --version
# multi-arch manifest exists
docker buildx imagetools inspect ghcr.io/ORG/forge-serverd:RELEASE | grep -q 'linux/arm64'

# CI proxy: the L0+L2 gates pass on a clean ubuntu-latest container
docker run --rm -v "$PWD":/src -w /src rust:1.96-bookworm bash -c '
  apt-get update && apt-get install -y clang libclang-dev pkg-config cmake >/dev/null &&
  cargo test --workspace --locked &&
  cargo run -p forge-cli --bin forge -- demo | grep -q "^REPLAY IDENTICAL: true$"'
echo "L4 GREEN"
```

### PRD mapping

- `prd-merged/03` **SS-19** (*"Self-host packaging: same crate as a single binary + Docker
  image; SQLite first … backup/restore"*), **SS-17** (backup/export scheduler role).
- `prd-merged/06` **PS-13** (Linux = the self-host server crate; v1, near-free).
- `prd-merged/00` §7 v1 scope (*"Linux headless build: embedded-server CLI"*) and §11 **M5**
  GA quality bar; **PRD 09** quality gates (Linux CI runner keeping the path green).

---

## Post-GA: GTK GUI track (explicitly out of every milestone above)

A native **Linux desktop GUI is NOT in v1** (decision **D5**; **PS-13**: *"No Linux GUI in
v1; revisit GTK4/gtk-rs post-GA"*; master non-goal *"Linux desktop GUI (headless server
only)"*). It is recorded here as a **separate, post-GA track** so it is never confused with
L0–L4:

- **GTK-L1 (post-GA):** a `gtk4`/`libadwaita` (via `gtk4-rs`/`relm4`) thin shell that
  embeds `forge-core` exactly as macOS/Windows do — **renderer + platform services only, no
  business logic** (CR-A1/PS-3). Platform services to provide: secrets via **Secret Service**
  (`libsecret`), file pickers returning **handles** (XDG portals), `forge://` deep links,
  notifications.
- **GTK-L2 (post-GA):** Linux renderer conformance (UI §3 kit) + the demo workspace smoke,
  matching the per-platform conformance gates (PS-4). Open question #3 in PS / master ("Linux
  GUI demand check post-GA") gates whether this track is funded at all.

This track shares the FFI binding strategy (PS-1: UniFFI/C-ABI) with the other desktop shells
and adds **no new core requirements**; it is purely additive after GA.

---

## Risks & mitigations (Linux/WSL-specific)

| # | Risk | Why it bites on Linux/WSL | Mitigation |
|---|---|---|---|
| R1 | **Native C build deps in minimal containers** | `rusqlite` (bundled SQLite) and `rquickjs` (vendored QuickJS) both compile C and need `gcc`/`clang` + `libclang` (bindgen). Alpine/musl uses **musl libc**, not glibc, and lacks `libclang` by default; a naive `FROM alpine` build fails at the bindgen/link step. | Keep build deps in the **builder stage only** (L3 multi-stage Dockerfile); the slim runtime image needs just `ca-certificates`. For static/Alpine targets, build `x86_64-unknown-linux-musl` with `musl-tools` + `CC=clang` (L4). Pin `clang`/`libclang-dev` as a documented hard requirement (L0). The bundled-C choice means the **runtime** image needs no SQLite/QuickJS system packages — the payoff. |
| R2 | **WSL networking for the server** | WSL2 runs in a NAT'd virtual network behind the Windows host; a daemon bound inside WSL is **not reachable from the LAN** by default, and `localhost` forwarding/firewall behavior differs by Windows build. mDNS (SS-16) generally does not cross the WSL2 NAT boundary. | For dev, bind `0.0.0.0` and reach via the WSL2 IP (`ip addr`) or Windows' `localhost` forwarding; document `netsh interface portproxy` for LAN exposure from Windows; document the **relay tunnel** (SS-16, outbound-only) as the WSL-friendly remote path since it needs no inbound port-forwarding. Treat real LAN/mDNS serving as a **bare-Linux / mirrored-mode-WSL** scenario, not default-NAT WSL. Note WSL2 mirrored networking mode (recent Win11) as the cleaner option and link it in the L3 doc. |
| R3 | **The wasm runtime backend gap** | The product's central acceptance test is the spine on **QuickJS-WASM** (CR-12), but `forge-runtime`'s `rquickjs` impl is `#[cfg(not(target_arch = "wasm32"))]` — there is **no wasm JS-engine backend yet**. On the Linux path this only surfaces as the `wasm32-unknown-unknown` build covering *pure* crates (`domain/schema/ui/pipeline`), not `runtime`. The Linux headless server runs the **native** QuickJS, so L0–L4 are unaffected — but the "WASM CI target" half of M0's gate is **not** Linux's to close. | Scope it honestly: L0–L4 gates build/test only the wasm-**clean pure crates** on wasm and the **native** runtime everywhere else. The QuickJS-WASM backend is a **web-shell (M3) deliverable**, tracked there, not in the Linux plan. Keep the `wasm32` build of the pure crates in the L0 gate so the wasm-clean invariant cannot regress on Linux. Document explicitly that "Linux green" ≠ "WASM spine green". |
| R4 | systemd absent on default WSL images | `Type=notify` units won't start; older WSL has no systemd. | Document `systemd=true` in `/etc/wsl.conf` (Win11 + recent WSL); ship a `nohup`/tmux fallback launcher for the daemon; in CI use Docker (no systemd dependency). |
| R5 | glibc version skew across distros for the gnu binary | A gnu binary built on a newer Ubuntu won't run on older RHEL (`GLIBC_X.YZ not found`). | Ship the **musl static** binary (L4) as the portable default; build the gnu binary on the oldest supported base (e.g. `manylinux`/`debian:bullseye`) when a gnu artifact is needed. |
| R6 | TLS/cert provisioning for the embedded daemon | LAN TLS (SS-21) needs a self-signed cert exchanged at pairing — easy to mis-handle into a TOFU hole. | Generate the pinned self-signed cert at first run; **exchange the pin during pairing**, never trust-on-first-use; document `cert.pem`/`key.pem` provisioning in the systemd unit and Docker volume. |

---

### Summary

Five ordered Linux milestones, each a single green gate: **L0** build/test/replay on WSL →
**L1** the CLI as a real on-disk dev loop → **L2** the in-process sync seam (`sync/` +
`server/` crates) → **L3** the embedded server daemon serving a LAN client (systemd + Docker)
→ **L4** self-host packaging (single binary + published image) under Linux CI. The GTK GUI is
a separate post-GA track, deliberately excluded. The live risks are native-C build deps in
minimal containers (R1), WSL NAT networking for the server (R2), and the QuickJS-WASM backend
gap that is a web-shell concern, not Linux's (R3).
