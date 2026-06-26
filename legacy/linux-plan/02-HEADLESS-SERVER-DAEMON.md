# 02 — The embedded server / daemon (`forge-server`, mostly planned)

**Audience:** an engineer on a Linux box or WSL distro who will build, package, and operate the
Forge sync server as a headless daemon.

**Scope honesty up front.** As of this writing the workspace at `forge/` contains **eleven crates and
no `server` crate**:

```
forge/crates/{domain, storage, crdt, schema, policy, runtime, pipeline, ui, core, cli, testkit}
```

There is **no `sync` crate and no `server` crate yet** — both are listed in the target layout of
`prd-merged/01-core-runtime-prd.md §2` (`sync/` = client + server protocol, `server/` =
`forge-server` cloud + embedded) but neither exists on disk. So everything in this document that
talks about WebSocket frames, daemons, pairing, mDNS, relay, and Docker is a **forward-looking
implementation plan**, not a description of running code. The plan is *grounded* in two things that
**do** exist today:

1. The **SS-1..SS-22 spec** in `prd-merged/03-sync-server-prd.md` (the contract this server must meet).
2. The **working M0a core** — `forge demo` runs `TS → SWC → QuickJS → ctx → SQLite → UI tree →
   deterministic replay` headlessly on this machine right now. The CRDT, storage, and policy
   primitives the server needs are already implemented and tested.

Every subsection below is tagged with a status marker:

- **[AVAILABLE-NOW]** — code exists in `forge/crates/*` today and the server reuses it directly.
- **[M2 / PLANNED]** — net-new code to be written; the milestone marker follows the PRD roadmap
  (embedded server is an **M2** deliverable; `prd-merged/03 §9` lists "desktop hosts a browser
  client" as the **M2 exit**).
- **[v1.x / FLAGGED]** or **[v2]** — explicitly deferred by the spec.

Linux's v1 role is the **headless path**: build + test the Rust workspace, run the CLI harness, and —
**when `forge-server` lands** — run the embedded sync server as a systemd daemon / Docker container.
No GUI; GTK4/libadwaita is explicitly post-GA (`PS-13`, decision D5).

---

## 1. Where the server crate plugs into the existing workspace

### 1.1 Two crates to add — `sync` then `server` **[M2 / PLANNED]**

`prd-merged/01 §2` already names both. Add them as workspace members in
`forge/Cargo.toml` (today the `members` list ends at `crates/testkit`):

```toml
# forge/Cargo.toml  — append to [workspace].members
members = [
  "crates/domain", "crates/storage", "crates/crdt", "crates/schema",
  "crates/policy", "crates/runtime", "crates/pipeline", "crates/ui",
  "crates/core", "crates/cli", "crates/testkit",
  "crates/sync",        # NEW: client + server protocol (PRD 03), transport-agnostic frames
  "crates/server",      # NEW: forge-server daemon — embedded + cloud deployments
]
```

```toml
# forge/Cargo.toml  — append to [workspace.dependencies]
forge-sync   = { path = "crates/sync" }
forge-server = { path = "crates/server" }
```

**Why two crates, not one.** `SS-3` requires frames to be **transport-agnostic by design** (a v2
device-to-device pipe reuses the frame types). So the wire protocol — frame structs, handshake state
machine, version-vector exchange logic — lives in **`sync`** with *no* tokio / no socket dependency,
exactly like `policy` today is "pure logic with no I/O" and stays `wasm32` clean. The **`server`**
crate owns the runtime: the tokio listener, TLS, mDNS, relay client, systemd integration, the binary.
This split lets the WASM web client (PRD 06 PS-10) link `sync` without dragging in tokio.

### 1.2 Crate dependency graph (planned)

```text
forge-server (bin + lib)  ── tokio, tokio-tungstenite, rustls, mdns-sd, axum (admin UI)
      │
      ├── forge-sync       ── frame types + handshake FSM + VV diff (NO I/O)   [PLANNED]
      ├── forge-core       ── WorkspaceCore: command/event facade              [AVAILABLE-NOW]
      ├── forge-crdt       ── CrdtDoc / Loro export_snapshot + import          [AVAILABLE-NOW]
      ├── forge-policy     ── PolicyEngine RBAC + capability gates             [AVAILABLE-NOW]
      ├── forge-storage    ── Store: SQLite KV/oplog/crdt_chunks/runs          [AVAILABLE-NOW]
      └── forge-domain     ── ActorContext, Role, CoreError, ids               [AVAILABLE-NOW]
```

The arrows that say AVAILABLE-NOW are the leverage: the hard parts (CRDT merge convergence, RBAC
gates, durable SQLite substrate) already exist and are tested. The server is mostly **plumbing
network frames into calls those crates already expose.**

### 1.3 What exists today that the server reuses verbatim

| Server need (SS spec) | Existing crate / API today | File |
|---|---|---|
| Per-doc CRDT merge, version vectors | `forge-crdt` `CrdtDoc::{export_snapshot, import}` over Loro 1.13.1; merge is commutative/idempotent | `forge/crates/crdt/src/lib.rs` |
| Durable op storage / chunks | `forge-storage` `oplog`, `crdt_chunks`, `crdt_snapshots`, `runs` tables; WAL + `synchronous=NORMAL` | `forge/crates/storage/src/lib.rs` |
| Server-enforced RBAC (SS-7) | `forge-policy` `PolicyEngine::check` (actor-role gate + budget + capability subcheck); `revoke` fail-closed | `forge/crates/policy/src/lib.rs` |
| Actor identity on every op (SS-7, CR-A3) | `forge_domain::ActorContext`, `Role` (Owner/Maintainer/Editor/Runner/Viewer/Auditor) | `forge/crates/domain` |
| Apply a remote op as a command | `forge_core::WorkspaceCore::handle(CoreCommand) -> CoreResponse` | `forge/crates/core/src/lib.rs` |
| Typed, non-panicking errors across the seam | `CoreError::{SyncError, PermissionDenied, ...}` (CR-A4) | `forge/crates/domain` |

> **Loro version-vector exchange grounding:** `forge-crdt`'s doc comment states updates "carry their
> own version vectors" and importing the same snapshot twice is a no-op. That is precisely the
> property `SS-1`'s "version-vector/frontier exchange" needs. The server's job is to *transmit* the
> frontier and request only the missing chunk — not to re-implement merge.

---

## 2. The WebSocket sync protocol (`SS-1` / `SS-2`) **[M2 / PLANNED]**

### 2.1 Transport (`SS-1`)

- **WebSocket over TLS 1.3.** Binary frames, **≤ 256 KB** each (hard cap; larger payloads chunked).
  Connection is **resumable** (session token + last-acked frontier replayed on reconnect).
- Three logical channels multiplexed on one socket:
  1. **sync** — per-document CRDT updates (the Loro frontier exchange).
  2. **presence** — cursors, online members (ephemeral, never persisted).
  3. **control** — membership, doc registry, permission notices.
- Rust stack: `tokio` + `tokio-tungstenite` (WebSocket) + `rustls` (TLS 1.3, no OpenSSL system dep —
  important for small static Linux binaries and Docker `scratch`/`distroless`).

### 2.2 Frame types (`SS-2`) — define in `forge-sync`, transport-agnostic (`SS-3`)

```rust
// forge/crates/sync/src/frame.rs   [PLANNED]
// Transport-agnostic per SS-3: these enums know nothing about WebSockets.
// Forward-compatible per SS-5 / DL-9: #[serde(other)] for unknown kinds,
// and we PRESERVE unknown fields rather than dropping them.

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Frame {
    Hello(Hello),                       // protocol version
    Capabilities(Capabilities),         // feature flags both sides support
    FrontierSummary(FrontierSummary),   // per-doc Loro version vector / frontier
    ChunkRequest(ChunkRequest),         // "send me ops since this frontier"
    ChunkResponse(ChunkResponse),       // opaque Loro update blob (<=256KB; split if larger)
    SnapshotOffer(SnapshotOffer),       // full-doc snapshot offer when behind by > N ops
    SnapshotResponse(SnapshotResponse),
    LiveUpdate(LiveUpdate),             // a freshly committed op, pushed to subscribers
    Ack(Ack),                           // durable-ack only AFTER storage write (SS-12 analog)
    ConflictNotice(ConflictNotice),     // SS-10 semantic conflict surfaced to client UI
    PermissionDenied(PermissionDenied), // SS-7 RBAC rejection -> client treats as conflict
    ResyncRequired(ResyncRequired),     // peer too far behind / corrupt frontier
}

#[derive(Serialize, Deserialize)]
pub struct FrontierSummary {
    pub doc_id: DocId,
    /// Opaque Loro version vector bytes. Server diffs this against its own
    /// VV to compute the missing op set; it does NOT parse Loro internals here.
    pub frontier: Vec<u8>,
}
```

### 2.3 Handshake state machine (`SS-2`, P-06 ordering)

Implement as an explicit FSM in `forge-sync` (unit-testable with zero sockets — same discipline as
the `policy` crate today):

```text
  protocol version   →  Hello / Hello
  peer identity      →  Capabilities (carries device token / invite proof, SS-6)
  feature caps       →  Capabilities negotiation (SS-5: unknown caps skipped)
  role claims        →  server resolves ActorContext via forge-policy            [reuses policy]
  known frontiers    →  FrontierSummary per subscribed doc
  chunk req/resp     →  ChunkRequest → ChunkResponse (loop until frontiers equal)
  ack                →  Ack after forge-storage commit                            [reuses storage]
  live subscribe     →  server streams LiveUpdate on new commits
```

**Forward compatibility (`SS-5`):** version negotiated at `Hello`; unknown frame kinds skipped via
`#[serde(other)]`; **unknown fields preserved** (mirrors the `DL-9` rule the workspace already
honors). Servers older than N-2 minors show a must-update banner but keep syncing.

### 2.4 Offline-first convergence (`SS-4`)

Clients queue CRDT updates durably in `crdt_chunks`/oplog (already exist in `forge-storage`) and
reconcile on reconnect. **Acceptance gate (`SS-4`): 1k pending ops converge p95 < 2 s.** This is
testable in-process **today's style** — the M0 model is "client and server in one test process"
(`SS §1`, `prd-merged/03 §9` M0 line), so the first sync test is a tokio integration test with two
`WorkspaceCore`s and a partition simulator, **no real socket required**.

### 2.5 Per-document Loro version-vector exchange — concrete flow **[M2, on AVAILABLE-NOW crdt]**

```text
1. Client sends FrontierSummary{ doc_id, frontier = local_vv_bytes }.
2. Server loads the doc's Loro state from forge-storage crdt_snapshots/chunks.
3. Server computes the delta:  needed = server_vv \ client_vv.
   (Loro gives us export-by-version; forge-crdt already exposes export_snapshot;
    we extend it with export_from(version) — small addition to CrdtDoc.)
4. Server replies ChunkResponse{ doc_id, update_blob } (split into <=256KB frames).
5. Client calls CrdtDoc::import(blob) — idempotent, commutative (already guaranteed).
6. Symmetric in the other direction. Loop until frontiers equal -> Ack.
```

The only **new `forge-crdt` method** required is `export_from(&self, since: &[u8]) -> Result<Vec<u8>>`
(delta export by version vector). Loro supports this; it is a thin wrapper, same translate-every-Loro-
error-to-`CoreError::SyncError` discipline the crate already uses.

---

## 3. Server-enforced RBAC (`SS-7`, `SS-9`) **[partly AVAILABLE-NOW]**

This is the spec's hardest invariant and it is **already half-built**.

- `SS-7`: *every* remote operation validated against actor identity, role, resource, operation,
  capability grants, and schema compatibility **before application** — "CRDT convergence is never a
  substitute for authorization."
- The `forge-policy` crate **already** enforces the actor-role gate, the budget/rate gate, and the
  capability subcheck via `PolicyEngine::check`, and **already** does immediate revocation
  (`PolicyEngine::revoke`, CR-4). It is pure logic, no I/O.

**Server wiring (PLANNED):** when a `LiveUpdate`/`ChunkResponse` arrives, the server resolves the
peer's `ActorContext` from the device token (§4), then calls `PolicyEngine::check` **before** handing
the op to `WorkspaceCore::handle`. On rejection:

```text
denied -> emit Frame::PermissionDenied{ doc_id, op_id, reason }
       -> append to audit log (SS-22: NO document content in logs)
       -> client surfaces it as a sync conflict in UI (SS-7)
```

**`SS-9` permission monotonicity** ("removing a grant never increases access anywhere in the sync
path") is a **property test** to add to `forge-policy` / `testkit`. The policy crate is already pure
and deterministic, so a `proptest`-style monotonicity check is a natural fit and is **release-blocking**
per the PRD acceptance list.

> Honesty note: `forge-policy`'s own header documents that **three of the seven SC-10 gates are still
> `AllowAll` stubs in M0a** (workspace-policy, run-profile, platform-permission). For a *server*, the
> workspace-policy gate especially must become real before embedded-server GA. That work is part of
> the M2 server task, not a free reuse.

---

## 4. Device-token pairing (`SS-6`) **[M2 / PLANNED]**

- **No anonymous LAN access** (`SS-6`). No login is ever required for local-only use (decision D8),
  but *pairing a device to an embedded server* always mints a workspace-scoped **device token**.
- Pairing flow: server displays a **QR code or short numeric code**; the joining device proves
  possession; server mints a role-scoped device token (`SS-6`) and records the pinned self-signed
  cert exchanged at pairing (**`SS-21`: no TOFU** — the cert is exchanged in-band during pairing).
- On a **headless Linux server there is no screen to show a QR**, so the CLI must print pairing
  material to the terminal:

```text
$ forge-server pair --role editor --ttl 15m
  Pairing code:  4192-7731
  QR (also):     https://<lan-ip>:8787/pair?code=4192-7731   (LAN only)
  Cert pin (SHA-256): 9f:2a:...:c1   <- the device must see this exact value
  Expires: 15m. One device. Ctrl-C to cancel.
```

- Membership (`SS-8`): expiring, role-scoped invite links; **removal revokes tokens and triggers
  client purge** of the workspace copy on next contact. Maps onto the existing
  `sync.invite` / `sync.accept_invite` commands already listed in `forge/spec/commands.md`
  (status "later", roles Owner / invited-actor) — the command surface is reserved; the
  implementation is M2.

Crates: tokens signed with `ed25519-dalek`; QR rendering via `qrcode` (text/ANSI output on headless).

---

## 5. Reach: mDNS on LAN + outbound relay (`SS-16`) **[M2 / PLANNED]**

### 5.1 mDNS / Bonjour advertisement

- Advertise **presence only, never access** (`SS-16`). Service type e.g. `_forge-sync._tcp`,
  TXT records `{ workspace_hint, proto_ver, requires_pairing=1 }`.
- Crate: `mdns-sd` (pure-Rust, no Avahi/system dep — keeps the Docker image and static binary clean).
- On Linux desktops Avahi may already own `:5353`; `mdns-sd` coexists. **In WSL2, mDNS on the LAN does
  not work out of the box** (WSL2 is NAT'd behind the Windows host) — call this out: on WSL, LAN
  discovery is unreliable; use the **relay** or an explicit host:port, or run with WSL mirrored
  networking (`networkingMode=mirrored` in `.wslconfig`, Windows 11 22H2+) for closer-to-native LAN
  behavior. Document this as a known WSL caveat, not a bug.

### 5.2 Outbound relay tunnel

- Remote access via **outbound** relay to the cloud relay (`SS-16`): **no port-forwarding**; the relay
  forwards **TLS ciphertext** and never sees plaintext. **Direct-connection upgrade** when a direct
  path is reachable.
- Self-hosters can **disable relay entirely** (LAN/VPN only) — config flag `relay.enabled = false`.
- Relay is **outbound-only** (`SS-21`), which is what makes it firewall-friendly: no inbound ports
  needed for relay mode (see §9 ufw notes).

### 5.3 Availability honesty (`SS-18`)

The embedded server syncs only while the host is awake; clients display home-server status. On a
Linux box run as a systemd service this is mostly moot (servers stay up), but the **status must still
be reported**. The **v1.x flagged** "cloud ciphertext mailbox for offline embedded servers"
(`SS-18`) is **[v1.x / FLAGGED]** — out of scope for the first daemon.

---

## 6. Backups & export (`SS-17`) **[M2 / PLANNED, on AVAILABLE-NOW storage]**

- Optional embedded-server role: **backup/export scheduler** (`SS-17`, DL-24 archives).
- Because a workspace is a **single SQLite file** (`forge-storage` opens one `rusqlite::Connection`
  on the portable workspace file), backup is straightforward and **partly available now**:
  - **Consistent online backup:** use SQLite's online backup API / `VACUUM INTO`, *not* a raw `cp`,
    because WAL mode is on (`journal_mode=WAL`). A `cp` of a live WAL DB can be torn.
  - Schedule via the server's internal scheduler (cron-like) **or** rely on a systemd timer (§7.4) —
    the simplest v1 path is a systemd timer invoking `forge-server backup`.
- Backup target: `${DATA_DIR}/backups/<workspace_id>-<utc>.fdb` (the `.fdb` = forge workspace file).
- Restore: `forge-server restore <file>` validates frontiers before swapping in (mirror of the
  migration "target verifies frontiers" step, `SS-20`).

```bash
# Online, WAL-safe backup of a single workspace file  [planned CLI; technique works today]
forge-server backup --workspace <id> --out /var/lib/forge/backups/
# equivalently, the raw SQLite technique the server uses internally:
sqlite3 /var/lib/forge/data/<id>.fdb "VACUUM INTO '/var/lib/forge/backups/<id>-$(date -u +%Y%m%dT%H%M%SZ).fdb'"
```

---

## 7. Linux ops: running it as a systemd service **[M2 / PLANNED]**

### 7.1 Build prerequisites (apt / dnf)

The current toolchain pin is `forge/rust-toolchain.toml` → `channel = "stable"` with a note that
`libsqlite3-sys 0.38+` needs `cfg_select!` (**stable ≥ 1.93**, built on **1.96.0**). So Linux/WSL
must have a **recent stable Rust** (1.96.0+). Install via `rustup`, not the distro's old `rustc`.

**Debian / Ubuntu / WSL-Ubuntu:**

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential pkg-config \
  ca-certificates curl git \
  libssl-dev          # only if a dep ever needs OpenSSL; rustls path avoids it
# rusqlite uses features=["bundled"] -> SQLite is COMPILED IN, no libsqlite3-dev needed.
# A C compiler (gcc, from build-essential) IS needed to build the bundled SQLite.
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
rustup toolchain install 1.96.0   # or honor rust-toolchain.toml's `stable`
```

**Fedora / RHEL / Rocky:**

```bash
sudo dnf install -y gcc gcc-c++ make pkgconf-pkg-config ca-certificates curl git
# (rustup as above; bundled SQLite means no sqlite-devel required)
```

> **Why no `libsqlite3-dev`:** `forge-storage/Cargo.toml` pins
> `rusqlite = { version = "0.40.1", features = ["bundled"] }`. SQLite is statically compiled into the
> binary. You only need a working C compiler (`build-essential` / `gcc`), which is why
> `build-essential` is in the apt list. This is also what keeps the Docker image and the self-host
> single binary truly self-contained.

### 7.2 Build the server

```bash
cd ~/forge                 # the workspace root (contains the top-level Cargo.toml)
cargo build --release -p forge-server        # once the crate exists [M2]
# today, the equivalent smoke build that DOES work:
cargo build --release -p forge-cli && ./target/release/forge demo   # M0a spine, AVAILABLE-NOW
```

### 7.3 Config file

Config precedence: CLI flags > env (`FORGE_*`) > config file > defaults. File at
`/etc/forge/server.toml` (system) or `${XDG_CONFIG_HOME}/forge/server.toml` (user).

```toml
# /etc/forge/server.toml   [PLANNED schema]
[server]
bind = "0.0.0.0"          # use 127.0.0.1 for relay-only / loopback-admin
port = 8787               # WebSocket sync port
data_dir = "/var/lib/forge/data"     # workspace .fdb files + crdt chunks live here
log_level = "info"        # structured logs; NEVER document content (SS-22)

[tls]
# SS-21: TLS 1.3 everywhere incl. LAN; pinned self-signed cert exchanged at pairing.
mode = "self_signed"      # "self_signed" | "provided"
cert = "/etc/forge/tls/cert.pem"   # generated on first run if self_signed
key  = "/etc/forge/tls/key.pem"

[relay]
enabled = true            # false = LAN/VPN only, no cloud relay (SS-16 self-hoster opt-out)
endpoint = "wss://relay.forge.example"   # outbound only (SS-21)

[mdns]
enabled = true            # advertise presence only (SS-16); set false on WSL/NAT'd hosts

[backup]
enabled = true
schedule = "daily"        # the server scheduler OR rely on the systemd timer in §7.4
keep = 14                 # retain last N backups
dir = "/var/lib/forge/backups"
```

Environment-variable overrides (handy in containers): `FORGE_PORT`, `FORGE_DATA_DIR`,
`FORGE_BIND`, `FORGE_RELAY_ENABLED`, `FORGE_RELAY_ENDPOINT`, `FORGE_LOG_LEVEL`.

### 7.4 systemd unit files

**Service** — `/etc/systemd/system/forge-server.service`:

```ini
[Unit]
Description=Forge embedded sync server (forge-server)
Documentation=https://github.com/your-org/forge
After=network-online.target
Wants=network-online.target

[Service]
Type=notify                       # forge-server calls sd_notify(READY=1) once listening
NotifyAccess=main
ExecStart=/usr/local/bin/forge-server run --config /etc/forge/server.toml
ExecReload=/bin/kill -HUP $MAINPID
Restart=on-failure
RestartSec=2

# Run unprivileged
User=forge
Group=forge
# State dir -> /var/lib/forge  (systemd creates + chowns it)
StateDirectory=forge
ConfigurationDirectory=forge
RuntimeDirectory=forge

# Hardening (server handles untrusted network input)
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
PrivateTmp=true
ReadWritePaths=/var/lib/forge
ProtectKernelTunables=true
ProtectControlGroups=true
RestrictAddressFamilies=AF_INET AF_INET6 AF_UNIX
RestrictNamespaces=true
LockPersonality=true
MemoryDenyWriteExecute=true        # NOTE: QuickJS is interpreter-only (rquickjs, no JIT) so W^X is fine.
SystemCallFilter=@system-service
SystemCallErrorNumber=EPERM
CapabilityBoundingSet=             # no capabilities needed; port 8787 is unprivileged

[Install]
WantedBy=multi-user.target
```

> `MemoryDenyWriteExecute=true` is safe **because the native engine is QuickJS via `rquickjs`, which
> is a bytecode interpreter with no JIT** — unlike JSC. This is a real benefit of the Linux/headless
> path running the QuickJS spine: it can be locked down harder than a JSC host could.

Create the service user:

```bash
sudo useradd --system --no-create-home --shell /usr/sbin/nologin forge
sudo install -m 0755 target/release/forge-server /usr/local/bin/forge-server
sudo systemctl daemon-reload
sudo systemctl enable --now forge-server
systemctl status forge-server
journalctl -u forge-server -f        # structured logs; verify NO doc content appears (SS-22)
```

**Backup timer** — `/etc/systemd/system/forge-backup.service` + `.timer`:

```ini
# forge-backup.service
[Unit]
Description=Forge nightly backup
[Service]
Type=oneshot
User=forge
ExecStart=/usr/local/bin/forge-server backup --all --config /etc/forge/server.toml
```

```ini
# forge-backup.timer
[Unit]
Description=Run Forge backup nightly
[Timer]
OnCalendar=*-*-* 03:30:00
Persistent=true
[Install]
WantedBy=timers.target
```

```bash
sudo systemctl enable --now forge-backup.timer
systemctl list-timers forge-backup.timer
```

### 7.5 WSL caveat for systemd

WSL2 supports systemd only when enabled in `/etc/wsl.conf`:

```ini
# /etc/wsl.conf  (then: wsl --shutdown from Windows, and reopen)
[boot]
systemd=true
```

Without it, run the daemon directly (`forge-server run --config ...`) under `tmux`/`nohup` or
`run_in_background`. Either way the binary is the same; only the supervisor differs.

---

## 8. Docker image (`SS-19`) **[M2 / PLANNED]**

`SS-19`: "same crate as a single binary + Docker image; **SQLite first, Postgres at scale**."

### 8.1 Dockerfile (multi-stage, static-ish, distroless)

```dockerfile
# ---- build stage ----
FROM rust:1.96-bookworm AS build
WORKDIR /src
# Bundled SQLite needs a C compiler; the rust image already has it.
COPY . .
RUN cargo build --release -p forge-server

# ---- runtime stage ----
# distroless: no shell, no package manager, tiny attack surface.
# rustls (not OpenSSL) means no libssl in the runtime image.
FROM gcr.io/distroless/cc-debian12
COPY --from=build /src/target/release/forge-server /usr/local/bin/forge-server
# Data dir is a volume so the SQLite workspace files survive container restarts.
VOLUME ["/var/lib/forge"]
EXPOSE 8787
USER 65532:65532                 # nonroot uid from distroless
ENTRYPOINT ["/usr/local/bin/forge-server"]
CMD ["run", "--config", "/etc/forge/server.toml"]
```

> If you ever need an even smaller image, the rustls + bundled-SQLite combination means you can target
> `scratch` with a fully static `x86_64-unknown-linux-musl` build (`rustup target add
> x86_64-unknown-linux-musl`; musl needs `musl-tools` on Debian). Start with distroless; go musl/scratch
> only if image size becomes a gate.

### 8.2 Build & run

```bash
docker build -t forge-server:dev -f Dockerfile .
docker run -d --name forge \
  -p 8787:8787 \
  -v forge-data:/var/lib/forge \
  -v /etc/forge:/etc/forge:ro \
  forge-server:dev
docker logs -f forge
```

### 8.3 docker-compose (SQLite-first, Postgres optional at scale per `SS-19`)

```yaml
# docker-compose.yml
services:
  forge:
    image: forge-server:dev
    ports: ["8787:8787"]
    volumes:
      - forge-data:/var/lib/forge
      - ./etc-forge:/etc/forge:ro
    environment:
      FORGE_BIND: "0.0.0.0"
      FORGE_PORT: "8787"
      FORGE_RELAY_ENABLED: "false"     # LAN/VPN-only self-host default
    restart: unless-stopped
    # --- Postgres is NOT used for embedded/self-host SQLite-first. It is the
    #     managed-cloud (SS-11) / "Postgres at scale" (SS-19) path. Uncomment to
    #     pilot the cloud topology; embedded servers stay on SQLite. [PLANNED]
  # db:
  #   image: postgres:16
  #   environment: { POSTGRES_PASSWORD: forge, POSTGRES_DB: forge }
  #   volumes: [ "pg-data:/var/lib/postgresql/data" ]
volumes:
  forge-data:
  # pg-data:
```

**SQLite vs Postgres decision (`SS-19`, `SS-11`):** the **embedded / self-host daemon is SQLite-only**
(one `.fdb` file per workspace, exactly the `forge-storage` model that exists today). Postgres is the
**managed-cloud** path (`SS-11`: Postgres for accounts/membership/metadata, object storage for
chunks). Do **not** put Postgres in the self-host path; it adds an operational dependency the PRD
explicitly avoids for embedded. Mark Postgres support **[M2+ / cloud-only]**.

---

## 9. Firewall (ufw) notes **[ops, applies once server runs]**

The relay is **outbound-only** (`SS-16`/`SS-21`), so **relay-only deployments need no inbound rule at
all**. Inbound rules are needed only for **direct LAN/VPN** sync.

```bash
# Direct LAN sync: open the sync port to the local subnet only (NOT the whole internet)
sudo ufw allow from 192.168.1.0/24 to any port 8787 proto tcp comment 'forge-sync LAN'

# mDNS discovery on LAN (only if mdns.enabled = true and you are NOT on WSL/NAT)
sudo ufw allow from 192.168.1.0/24 to any port 5353 proto udp comment 'forge mDNS'

# Relay-only (no inbound): make sure outbound 443/wss is allowed (usually default-allow)
sudo ufw default deny incoming
sudo ufw default allow outgoing
sudo ufw enable
sudo ufw status verbose
```

- **Never** `ufw allow 8787` unscoped — that exposes the sync port to the whole internet. Pairing +
  TLS pinning + RBAC still gate access, but defense-in-depth says scope to the subnet/VPN.
- **WSL2:** `ufw` inside the distro governs the *Linux* netns, but inbound LAN reachability is
  controlled by the **Windows host firewall + WSL NAT**. To reach a WSL-hosted server from the LAN you
  must add a Windows `netsh portproxy` rule or use mirrored networking. Document: prefer **relay** on
  WSL.

---

## 10. Self-host single-binary build (`SS-19`) **[M2 / PLANNED]**

The whole point of bundled SQLite + rustls is that the self-host artifact is **one file with no
runtime dependencies**.

```bash
# Native single binary (glibc)
cargo build --release -p forge-server
file target/release/forge-server        # ELF 64-bit, dynamically linked to glibc only

# Fully static single binary (no glibc dependency) for "copy anywhere" self-host:
rustup target add x86_64-unknown-linux-musl
sudo apt-get install -y musl-tools
cargo build --release -p forge-server --target x86_64-unknown-linux-musl
ldd target/x86_64-unknown-linux-musl/release/forge-server   # -> "not a dynamic executable"
```

Distribution shape (mirrors how `forge demo` is already self-contained — the CLI embeds its demo
applet via `include_str!`, no runtime filesystem layout needed):

```text
forge-server-vX.Y.Z-x86_64-linux/
  forge-server                 # the static binary
  server.toml.example          # copy to /etc/forge/server.toml
  forge-server.service          # the systemd unit from §7.4
  README.md                    # quickstart: useradd, install, pair, ufw
```

Subcommands the single binary must expose **[PLANNED]**:

```text
forge-server run     --config <path>          # the daemon (sd_notify-aware)
forge-server pair    --role <r> --ttl <dur>   # headless device pairing (§4)
forge-server backup  [--all|--workspace <id>] # WAL-safe online backup (§6)
forge-server restore <file>                   # frontier-verified restore (§6)
forge-server status                           # local-only status page data (SS-22)
forge-server admin   [--bind 127.0.0.1:9090]  # local admin UI (SS-19), loopback by default
```

The **admin UI** (`SS-19`) is a small `axum`-served local page bound to loopback (`127.0.0.1`) — the
"embedded local-only status page" of `SS-22` (RED metrics, per-doc sync-lag gauges, **no document
content**). Mark **[M2 / PLANNED]**.

---

## 11. Status summary — what's real vs planned

| Capability | Spec | Status | Where |
|---|---|---|---|
| TS→SWC→QuickJS→ctx→SQLite→UI→replay spine | CR-12 | **AVAILABLE-NOW** | `forge demo`, `forge/crates/{pipeline,runtime,core,ui,storage}` |
| Loro CRDT export/import, idempotent merge | DL-1/9, SS-1 | **AVAILABLE-NOW** | `forge/crates/crdt` |
| Durable oplog / crdt_chunks / runs (WAL) | DL-4/23, SS-4 | **AVAILABLE-NOW** | `forge/crates/storage` |
| RBAC actor-role + capability + revoke gates | SS-7, CR-4 | **AVAILABLE-NOW (3 of 7 SC-10 gates)** | `forge/crates/policy` |
| `sync` crate: frames + handshake FSM + VV diff | SS-1/2/3/5 | **M2 / PLANNED** | `crates/sync` (new) |
| `server` crate: tokio/tungstenite/rustls daemon | SS-15/19 | **M2 / PLANNED** | `crates/server` (new) |
| `export_from(version)` delta export | SS-1 | **M2 / PLANNED (thin Loro wrapper)** | `crates/crdt` (add method) |
| Device-token pairing (QR/short-code, cert pin) | SS-6/21 | **M2 / PLANNED** | `crates/server` |
| mDNS LAN advertise | SS-16 | **M2 / PLANNED** | `crates/server` (`mdns-sd`) |
| Outbound relay tunnel | SS-16 | **M2 / PLANNED** | `crates/server` |
| Backups (online WAL-safe) + scheduler | SS-17 | **M2 / PLANNED (technique works now)** | `crates/server` + systemd timer |
| systemd service + hardening unit | SS-15 ops | **M2 / PLANNED (unit drafted here)** | §7 |
| Docker image (distroless, SQLite-first) | SS-19 | **M2 / PLANNED (Dockerfile drafted here)** | §8 |
| Single static binary (musl) | SS-19 | **M2 / PLANNED** | §10 |
| Permission-monotonicity property test | SS-9 | **M2 / PLANNED (policy is already pure)** | `crates/policy`+`testkit` |
| Admin / status UI (loopback axum) | SS-19/22 | **M2 / PLANNED** | `crates/server` |
| Postgres at scale | SS-11/19 | **M2+ / CLOUD-ONLY (not self-host)** | managed cloud |
| Cloud ciphertext mailbox for offline server | SS-18 | **v1.x / FLAGGED** | deferred |
| Device-to-device direct transport | SS-3 | **v2** | frames reserved, not built |
| Linux GUI (GTK4/libadwaita) | PS-13, D5 | **POST-GA** | out of scope |

---

## 12. Acceptance checks (explicit, runnable)

**A. AVAILABLE-NOW baseline (must pass on Linux/WSL today, no server crate):**

```bash
cd ~/forge
cargo test --workspace                 # all existing crate + e2e tests green
cargo build --release -p forge-cli
./target/release/forge demo            # prints run_ok, replay byte-identical fingerprint
```
*Pass = the headless spine builds and the deterministic replay matches on this Linux box.* This is the
proof the server has a solid core to plug into.

**B. M2 server acceptance (when `crates/server` lands) — map 1:1 to `prd-merged/03 §9`:**

1. **In-process round-trip (M0 line of §9):** a tokio integration test with two `WorkspaceCore`s and a
   partition simulator → zero divergence. `cargo test -p forge-server in_process_partition`.
2. **1k-pending-ops convergence (`SS-4`):** queue 1000 ops offline, reconnect → p95 < 2 s, frontiers
   equal. Bench in `testkit`.
3. **Unauthorized remote op (`SS-7`):** a Viewer attempts a write op over the wire → server emits
   `PermissionDenied` **before** `WorkspaceCore::handle`, logs it (no doc content), client surfaces a
   conflict. Asserted in `forge-server` integ test.
4. **Permission monotonicity (`SS-9`):** property test green (release-blocking).
5. **Embedded-on-default-hardware (`SS §9` M2 exit):** server serves **10 active members, p95 LAN sync
   < 100 ms**, and hosts a browser client.
6. **7-day soak (`SS §9`):** 50 workspaces, randomized partitions/restarts/reordered+duplicated
   messages → **zero divergence, zero acked-write loss.**

**C. Linux ops acceptance (this document's deliverables):**

```bash
# service comes up and reports ready via sd_notify
sudo systemctl start forge-server && systemctl is-active forge-server      # -> active
journalctl -u forge-server | grep -i "document content"                    # -> MUST be empty (SS-22)
# headless pairing prints code + cert pin
sudo -u forge forge-server pair --role viewer --ttl 5m | grep -E 'Pairing code|Cert pin'
# WAL-safe backup produces a valid SQLite file
sudo -u forge forge-server backup --all && sqlite3 /var/lib/forge/backups/*.fdb 'PRAGMA integrity_check;'  # -> ok
# docker image runs as nonroot and listens
docker run --rm -p 8787:8787 forge-server:dev run --config /etc/forge/server.toml &
ss -ltnp | grep 8787                                                       # -> LISTEN
# firewall scoped, not wide open
sudo ufw status | grep 8787                                                # -> "from 192.168.x.0/24", never "Anywhere"
# static binary has no dynamic deps
ldd target/x86_64-unknown-linux-musl/release/forge-server                  # -> not a dynamic executable
```

---

## 13. Build order for the M2 server work (recommended)

1. **`crates/sync`** — frame enums + handshake FSM + VV-diff logic, **no I/O**, unit-tested like
   `policy`. Add `export_from` to `forge-crdt`.
2. **In-process sync test** — two cores + partition sim (satisfies the §9 M0 line; no socket).
3. **`crates/server` skeleton** — tokio + tokio-tungstenite + rustls listener; wire `forge-policy`
   check **before** `WorkspaceCore::handle`.
4. **Pairing + device tokens (`SS-6`)** and **cert pinning (`SS-21`)**.
5. **mDNS (`SS-16`)** then **relay client (`SS-16`)**.
6. **Backups (`SS-17`)**, **admin/status page (`SS-19/22`)**.
7. **Packaging:** systemd unit (§7) → Docker (§8) → static musl single binary (§10).
8. **Property/soak tests (`SS-9`, §9 soak)** — release gates.

> Throughout: the discipline already visible in the codebase holds — every Loro/SQLite/network error
> maps to a typed `CoreError`, **no panic crosses the FFI/network seam** (CR-A4, CR-13), shells/peers
> mutate state **only** through commands (CR-A1), and **no document content ever enters logs**
> (`SS-22`).
