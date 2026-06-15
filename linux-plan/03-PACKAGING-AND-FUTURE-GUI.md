# Packaging + the future GTK GUI

**Project:** Forge (codename) · **Document:** Linux/WSL plan 03 of N · **Target reader:** running on WSL2 (Ubuntu/Debian) or a native Linux box
**Scope of this doc:**
- **Part A (v1, ships now):** how to produce and distribute the *headless* artifacts — a static/portable `forge-cli` binary, a Docker image for the eventual `forge-server` daemon, and the exact way WSL users run both.
- **Part B (post-GA, NOT v1):** a concrete sketch of the eventual GTK4/libadwaita Linux GUI (gtk-rs), how it would consume the same Rust core with **no FFI**, and the Linux desktop distribution formats (Flatpak / AppImage / `.deb` / `.rpm`).

This document is normative for Part A and *planning-only / explicitly post-GA* for Part B. Per **PS-13** and master-PRD §7 non-goals, **there is no Linux desktop GUI in v1** — Linux v1 is the headless embedded-server + build/test/CLI path only. Part B exists so that the GUI is a "implement the renderer + package it" task later, not a redesign.

> **Reality check on the current tree.** The working core under `/forge` today has crates `domain, storage, crdt, schema, policy, runtime, pipeline, ui, core, cli, testkit` (see `forge/Cargo.toml`). The binary that exists is `forge` (from `forge-cli`), and its one real subcommand is `forge demo`. The `server`, `sync`, `ffi`, `llm`, `secrets`, and `audit` crates named in PRD 01 §2 are **not yet scaffolded**. Wherever this doc references `forge-server`, that is the **planned** crate from PRD 03 (SS-19); the packaging machinery below is written so it works for `forge-cli` *today* and for the server binary *when it lands*, with the deltas called out explicitly.

---

## Section 1 — What we package, and why each form exists

| Artifact | Crate / bin | v1? | Consumer | Why this form |
|---|---|---|---|---|
| `forge` CLI (portable native binary) | `forge-cli` → `forge` | **Yes** | Developers/CI on Linux/WSL; the M0 harness (PS-5) | Single self-contained executable: `cargo build` output, statically linkable, no runtime deps beyond glibc/musl. |
| `forge-server` daemon (native binary) | `server` (planned, SS-19) | **Yes (when crate lands)** | Self-hosters running embedded sync (SS-15..19) | Same single-binary story; runs as a systemd service or in Docker. |
| Docker image | wraps `forge-server` | **Yes (when crate lands)** | Self-hosters who want a container; CI smoke | "single binary + Docker image" is the SS-19 self-host packaging requirement. |
| `forge-gui` (GTK4/libadwaita app) | `shell-gtk` (planned, **post-GA**) | **No (post-GA)** | Linux desktop users, eventually | PS-13 revisit; consumes the core by direct Rust calls. |
| Flatpak / AppImage / `.deb` / `.rpm` | wrap `forge-gui` | **No (post-GA)** | Linux desktop distribution | Standard Linux desktop delivery channels. |

The golden rule from PRD 06 §1 holds for every artifact: **the shell contains no business logic.** The CLI, the server, and (later) the GTK GUI are all thin layers over `forge-core`'s `Command/Event/Stream` contract (CR-A1). Packaging never duplicates logic; it only wraps the same binary surface.

---

## PART A — v1 packaging (headless, ships now)

## Section 2 — Build host prerequisites (apt / dnf, exact packages)

The core builds with the pinned toolchain in `forge/rust-toolchain.toml` (`channel = "stable"`, expects `>= 1.93`; the repo was built on `1.96.0` because `libsqlite3-sys 0.38+` needs `cfg_select!`). `rusqlite` is `version = "0.40.1", features = ["bundled"]` — the SQLite **C amalgamation is vendored and compiled from source**, so you need a C toolchain. `rquickjs = "0.12"` (native, gated `cfg(not(target_arch = "wasm32"))`) also compiles C. `swc_core = "68"` is pure Rust.

### 2.1 Install Rust via rustup (do NOT use distro `rustc` — it lags)

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
. "$HOME/.cargo/env"
# rust-toolchain.toml pins the channel; rustup honors it automatically on first build,
# but install it eagerly so the first `cargo` call is fast:
rustup toolchain install stable
rustup target add wasm32-unknown-unknown        # for the wasm32-clean check (CR-15)
```

### 2.2 Debian / Ubuntu / WSL2-Ubuntu (apt)

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential   `# gcc, g++, make — needed to compile vendored SQLite + QuickJS C` \
  pkg-config        `# crate build scripts probe with pkg-config` \
  libssl-dev        `# transitively pulled by future server/sync TLS deps` \
  ca-certificates   `# TLS roots for rustup + any future net` \
  git curl
# For a fully static musl binary (Section 4):
sudo apt-get install -y musl-tools     # provides musl-gcc
rustup target add x86_64-unknown-linux-musl
```

### 2.3 Fedora / RHEL / CentOS-Stream (dnf)

```bash
sudo dnf groupinstall -y "Development Tools"   # gcc, g++, make
sudo dnf install -y pkgconf-pkg-config openssl-devel ca-certificates git curl
# musl static:
sudo dnf install -y musl-gcc musl-libc-static
rustup target add x86_64-unknown-linux-musl
```

### 2.4 WSL2 note

WSL2 is a **first-class build/run target** (master §7, PS-13). On WSL2 the above apt commands run unchanged inside the Linux distro (e.g. Ubuntu). One caveat: **build inside the Linux filesystem, not `/mnt/c`.** Cargo on a `9p`-mounted Windows drive is 5–20× slower and breaks file-watching. Clone/keep the repo under `~/…` (e.g. `~/src/terrane`), not `/mnt/c/...`. Verify:

```bash
df -T ~        # should show ext4 (the WSL VHDX), not 9p / drvfs
```

### 2.5 Acceptance check — host is ready

```bash
cd ~/src/terrane/forge            # adjust to your clone path
rustc --version                   # >= 1.93 (repo built on 1.96.0)
cc --version                      # a working C compiler is present
cargo build --workspace           # vendored SQLite + QuickJS compile; 0 errors
cargo test  --workspace           # all crate tests green, incl. ui golden trees
cargo run -p forge-cli -- demo    # prints the spine report, exits 0
```
The last command is the live proof of the headless path: `TS → SWC → QuickJS → ctx → SQLite → UI tree → deterministic replay`, fully offline.

---

## Section 3 — The portable `forge` CLI binary (dynamic glibc build)

The default, lowest-friction artifact. Dynamically links glibc but statically vendors SQLite + QuickJS (they're compiled into the binary), so the only external dependency is the system libc.

### 3.1 Build

```bash
cd ~/src/terrane/forge
cargo build --release -p forge-cli
ls -lh target/release/forge        # the artifact
file target/release/forge          # ELF 64-bit LSB, dynamically linked, interpreter /lib64/ld-linux...
ldd  target/release/forge          # should list ONLY libc/libm/libgcc/libpthread/libdl — no SQLite, no QuickJS
```

### 3.2 Strip + size

```bash
strip target/release/forge          # drop symbols; typically saves a few MB
ls -lh target/release/forge
```
Optionally add to a release profile in `forge/Cargo.toml` (not present today — this is the recommended addition):

```toml
[profile.release]
opt-level = "z"      # optimize for size; or "3" for speed
lto = true
codegen-units = 1
strip = true         # strip in-build, no separate `strip` step
panic = "abort"      # smaller; acceptable because CR-A4 forbids panics across the FFI boundary anyway
```

> Tracking note: PRD 01 §8 sets **Core binary: < 12 MB native**. The CLI binary is the closest proxy on Linux today; record its stripped size in CI so the budget has a Linux witness.

### 3.3 glibc-version portability caveat

A glibc-linked binary requires the **runtime glibc to be ≥ the build-host glibc**. Build on the *oldest* glibc you intend to support (e.g. build in an `ubuntu:22.04` container, run on 22.04+). If you need "runs on anything", use the musl static build in Section 4 instead.

### 3.4 Acceptance check — portable CLI

```bash
./target/release/forge demo;  echo "exit=$?"     # exit=0
./target/release/forge help                      # prints usage
# Copy to a clean machine/container of the same-or-newer glibc and re-run:
docker run --rm -v "$PWD/target/release/forge:/forge:ro" ubuntu:22.04 /forge demo
```

---

## Section 4 — Fully static portable binary (musl) — the "runs anywhere" artifact

For a binary with **zero shared-library dependencies** (drop it on any x86_64 Linux, including minimal/Alpine containers and locked-down CI), build against `x86_64-unknown-linux-musl`. Because SQLite and QuickJS are vendored C compiled by `cc`, musl statically links them too.

### 4.1 Build

```bash
# Prereqs from §2.2/§2.3 (musl-tools / musl-gcc) installed.
cd ~/src/terrane/forge
rustup target add x86_64-unknown-linux-musl
cargo build --release -p forge-cli --target x86_64-unknown-linux-musl
file target/x86_64-unknown-linux-musl/release/forge
#   -> ELF 64-bit LSB, statically linked
ldd  target/x86_64-unknown-linux-musl/release/forge
#   -> "not a dynamic executable"   <-- this is the win
```

If `cc` doesn't pick up the musl cross-compiler automatically, point it explicitly:

```bash
export CC_x86_64_unknown_linux_musl=musl-gcc
export CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc
cargo build --release -p forge-cli --target x86_64-unknown-linux-musl
```

### 4.2 The reproducible-builder shortcut (`cross`)

For CI, the cleanest reproducible static build can use `cross` so the linker
toolchain is pinned inside a Docker image instead of relying on host packages:

```bash
cargo install cross
cross build --release -p forge-cli --target x86_64-unknown-linux-musl
```

This needs Docker available, which under WSL2 means Docker Desktop's WSL
integration or `dockerd` inside the distro.

### 4.3 arm64

For Raspberry Pi / arm64 servers / Apple-silicon-Linux:

```bash
rustup target add aarch64-unknown-linux-musl
cross build --release -p forge-cli --target aarch64-unknown-linux-musl
```

### 4.4 Acceptance check — static binary

```bash
BIN=target/x86_64-unknown-linux-musl/release/forge
ldd "$BIN" 2>&1 | grep -q "not a dynamic executable" && echo "STATIC OK"
# Prove it on the smallest possible base — no glibc, no shell libs:
docker run --rm -v "$PWD/$BIN:/forge:ro" alpine:3 /forge demo; echo "exit=$?"   # exit=0
docker run --rm -v "$PWD/$BIN:/forge:ro" busybox:stable /forge demo             # also works
```
If `forge demo` exits 0 inside `alpine:3` (which has musl, no glibc) and even `busybox`, the static path is proven.

---

## Section 5 — Docker image for the headless server (SS-19)

PRD 03 SS-19 requires self-host packaging as "**same crate as a single binary + Docker image**". Two recommended Dockerfiles: a multi-stage build for repeatability, and a `FROM scratch` variant that wraps the musl binary from Section 4.

> **Today vs. when the server lands.** Until the `server` crate exists, the image below wraps `forge` (the CLI) so the Docker *pipeline* is proven now (`docker run … forge demo`). When `forge-server` lands, change the built package/bin name and the `ENTRYPOINT`/`CMD` and `EXPOSE` lines as noted inline. Nothing else changes.

### 5.1 Multi-stage Dockerfile (builds from source, glibc base)

`forge/Dockerfile`:

```dockerfile
# ---- build stage ----
FROM rust:1.96-bookworm AS build
WORKDIR /src
# vendored SQLite + QuickJS need a C toolchain (present in the rust image)
COPY . .
# When forge-server lands: change -p forge-cli to -p forge-server
RUN cargo build --release -p forge-cli && \
    strip target/release/forge

# ---- runtime stage (small glibc base) ----
FROM debian:bookworm-slim AS runtime
RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
# Run as non-root; the server writes its workspace/state under /data
RUN useradd --system --uid 10001 --home /data forge && mkdir -p /data && chown forge /data
COPY --from=build /src/target/release/forge /usr/local/bin/forge
USER forge
WORKDIR /data
VOLUME ["/data"]
# Server deltas (SS-1 WebSocket/TLS, SS-15 configurable port):
# EXPOSE 4455
# ENTRYPOINT ["/usr/local/bin/forge"]
# CMD ["serve", "--addr", "0.0.0.0:4455", "--data", "/data"]
# Until then, prove the image with the spine demo:
ENTRYPOINT ["/usr/local/bin/forge"]
CMD ["demo"]
```

### 5.2 `FROM scratch` variant (wraps the musl static binary, smallest image)

Build the static binary first (Section 4), then:

`forge/Dockerfile.scratch`:

```dockerfile
FROM scratch
# musl static binary has no shared-lib deps; scratch needs nothing else but TLS roots
COPY target/x86_64-unknown-linux-musl/release/forge /forge
# CA roots only matter once the server makes outbound TLS (relay, SS-16); copy from any builder:
# COPY --from=build /etc/ssl/certs/ca-certificates.crt /etc/ssl/certs/
# EXPOSE 4455
ENTRYPOINT ["/forge"]
CMD ["demo"]
```
This yields an image essentially the size of the binary (a few MB). Use it once `forge-server` adds the CA-roots copy line for relay TLS.

### 5.3 Build & smoke

```bash
cd ~/src/terrane/forge
docker build -t forge:dev -f Dockerfile .
docker run --rm forge:dev          # runs `forge demo` -> exits 0
# scratch variant (after building the musl binary in §4):
docker build -t forge:scratch -f Dockerfile.scratch .
docker run --rm forge:scratch
```

### 5.4 When the server lands — compose for a self-hoster

A reference `docker-compose.yml` (planning sketch; matches SS-15/SS-16/SS-22 knobs):

```yaml
services:
  forge-server:
    image: forge:dev
    command: ["serve", "--addr", "0.0.0.0:4455", "--data", "/data",
              "--relay", "off"]          # SS-16: LAN/VPN-only; relay opt-in
    ports:
      - "4455:4455"                       # SS-15 configurable port (WebSocket/TLS, SS-1)
    volumes:
      - forge-data:/data                  # workspace SQLite + backups (SS-17 backup scheduler)
    restart: unless-stopped
volumes:
  forge-data:
```
SS-21/SS-22 reminders that the server crate must honor (not packaging, but listed so they're not forgotten at image time): TLS 1.3 with a pinned self-signed cert exchanged at pairing (no TOFU); structured logs that contain **no document content**; a local-only status page.

### 5.5 systemd unit (bare-metal self-host, no Docker)

For self-hosters who run the static binary directly. `/etc/systemd/system/forge-server.service` (planning sketch):

```ini
[Unit]
Description=Forge embedded sync server (SS-15..19)
After=network-online.target
Wants=network-online.target

[Service]
Type=simple
User=forge
Group=forge
# ExecStart=/usr/local/bin/forge serve --addr 0.0.0.0:4455 --data /var/lib/forge
ExecStart=/usr/local/bin/forge demo      # placeholder until `serve` exists
Restart=on-failure
# Hardening (defensible defaults for a sandboxing product):
NoNewPrivileges=true
ProtectSystem=strict
ProtectHome=true
ReadWritePaths=/var/lib/forge
PrivateTmp=true

[Install]
WantedBy=multi-user.target
```
```bash
sudo useradd --system --home /var/lib/forge forge && sudo mkdir -p /var/lib/forge && sudo chown forge /var/lib/forge
sudo systemctl daemon-reload && sudo systemctl enable --now forge-server
sudo systemctl status forge-server
```

### 5.6 Acceptance check — Docker/server packaging

```bash
docker build -t forge:dev -f forge/Dockerfile forge/   && docker run --rm forge:dev   # exit 0
docker build -t forge:scratch -f forge/Dockerfile.scratch forge/ && docker run --rm forge:scratch  # exit 0
# Image is non-root and minimal:
docker run --rm --entrypoint id forge:dev               # uid=10001(forge), not 0
docker image inspect forge:scratch -f '{{.Size}}'       # only a few MB
```

---

## Section 6 — How WSL users run it

Two distinct ways a WSL user touches Forge; both are first-class.

### 6.1 As a build/test/CLI box (v1, now)

Identical to native Linux — Sections 2–4 work unchanged. The only rules:
1. **Work inside the WSL ext4 filesystem** (`~/src/...`), never `/mnt/c/...` (Section 2.4).
2. To invoke the Linux binary *from* Windows tooling, WSL interop lets you call it via `wsl.exe`:
   ```powershell
   # from a Windows PowerShell prompt:
   wsl.exe -d Ubuntu -- ~/src/terrane/forge/target/release/forge demo
   ```
3. To run the binary *inside* WSL but reach a server from a Windows browser, see §6.2.

### 6.2 As a headless server host reachable from Windows (when `forge-server` lands)

WSL2 runs in a lightweight VM with its own virtual NIC. Two reach scenarios:

- **From the same Windows host (localhost):** WSL2 forwards `localhost`. A server bound to `0.0.0.0:4455` inside WSL is reachable at `http://localhost:4455` from Windows apps. No extra config on recent Windows builds (localhost forwarding is on by default).
- **From other devices on the LAN (the SS-16 mDNS/LAN use case):** the WSL2 VM IP is NAT'd, so other machines can't reach it directly. Either:
  - enable **mirrored networking** in `%UserProfile%\.wslconfig` (Windows 11 22H2+):
    ```ini
    [wsl2]
    networkingMode=mirrored
    ```
    then the server's LAN address is the Windows host's address — mDNS/Bonjour (SS-16) and pairing-by-LAN-address work, or
  - add a Windows `netsh portproxy` rule mapping a Windows port to the WSL VM IP (NAT mode), and open the Windows Firewall for it.

  > Honest caveat to surface in docs (matches SS-18 availability honesty): under default NAT networking the WSL VM IP changes across reboots, so LAN self-hosting from WSL is best done with `networkingMode=mirrored`. For "real" always-on self-hosting, a native Linux box or the Docker image is the better target; WSL is ideal for **dev and single-host** use.

- **Relay path (SS-16):** the outbound relay tunnel needs no inbound port and therefore **just works from WSL** with no networking config — the server dials out to the cloud relay. This is the recommended WSL remote-access path.

### 6.3 Docker under WSL

`docker build`/`docker run` from Sections 5.x work under WSL via Docker Desktop's WSL2 integration, or by running `dockerd` directly inside the distro. The acceptance checks in §5.6 are the validation.

### 6.4 Acceptance check — WSL

```bash
# Inside WSL:
uname -a                                   # shows ...microsoft-standard-WSL2...
~/src/terrane/forge/target/release/forge demo   # exit 0
# From Windows PowerShell:
wsl.exe -- ~/src/terrane/forge/target/release/forge demo   # exit 0
```

---

## PART B — The future GTK GUI

> ## ⚠️ POST-GA / NOT v1
>
> **Everything below is planning only.** Per **PS-13** ("No Linux GUI in v1 (D5); revisit GTK4/gtk-rs post-GA") and master-PRD §7 non-goals ("Linux desktop GUI (headless server only)"), the GTK shell is **not in v1 scope**, has **no committed milestone**, and is gated on the open question "Linux GUI demand check post-GA" (PS-13 / PRD 06 §10 #3). This section exists so the eventual GUI is an *implement-and-package* task, not a redesign. **Do not build this for v1.**

## Section 7 — Architecture: GTK4/libadwaita over the same core, no FFI

The Linux GUI would be **another thin shell** (PRD 06 §1): native chrome + the UI-tree renderer + platform services, over the **same** `forge-core`. Crucially, because the GTK shell is itself written in **Rust** (gtk-rs / `gtk4` + `libadwaita` crates), it consumes the core by **direct Rust function calls** — it links `forge-core` as a normal Cargo dependency. **No UniFFI, no C-ABI, no wasm-bindgen.** This is the same structural pattern as the WinUI plan's renderer, but where WinUI must cross a C#↔Rust UniFFI/C-ABI boundary (PS-14), the GTK shell has *no language boundary at all*.

```
┌──────────────────────────────────────────────────────────────────────┐
│  forge-gui  (Rust binary, crate `shell-gtk`, POST-GA)                  │
│  ┌───────────────┐   ┌──────────────────────────────────────────────┐ │
│  │ GTK4 +        │   │  GtkUiRenderer                                │ │
│  │ libadwaita    │◀──│  UI tree (forge_ui::Node) → GTK widgets       │ │
│  │ chrome        │   │  Patch (forge_ui::Patch) → in-place widget ops │ │
│  │ (window, tabs,│   └──────────────────────────────────────────────┘ │
│  │  headerbar)   │            ▲ subscribes to Stream<UI patches>        │
│  └───────────────┘            │ emits events (onTap/onChange) as Cmds   │
│            direct Rust calls — NO FFI │                                 │
│  ┌────────────────────────────────────▼─────────────────────────────┐ │
│  │ forge-core  (WorkspaceCore::handle, Command/Event/Stream, CR-A1)  │ │
│  │   pipeline · runtime(QuickJS) · storage(SQLite) · ui · policy ...  │ │
│  └───────────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────────┘
```

### 7.1 Crate placement & dependencies

A new (post-GA) member, e.g. `crates/shell-gtk` or a sibling `shells/gtk`. Sketch `Cargo.toml`:

```toml
[package]
name = "forge-gui"
edition = "2021"

[[bin]]
name = "forge-gui"
path = "src/main.rs"

[dependencies]
forge-core   = { path = "../core" }     # direct, no FFI
forge-domain = { path = "../domain" }   # CoreCommand / CoreEvent / Node-bearing payloads
forge-ui     = { path = "../ui" }       # Node, Patch, diff/apply — the renderer's input contract
# Pinned at GUI-implementation time; representative current majors:
gtk4        = { version = "0.9", package = "gtk4", features = ["v4_10"] }
libadwaita  = { version = "0.7", package = "libadwaita", features = ["v1_4"] }
```
System deps to build it (Debian/Ubuntu): `sudo apt-get install -y libgtk-4-dev libadwaita-1-dev` (Fedora: `sudo dnf install -y gtk4-devel libadwaita-devel`). These are **only** needed for the post-GA GUI build — the v1 headless artifacts (Parts A) require none of them.

### 7.2 The renderer: `forge_ui::Node` → GTK widgets

The GTK renderer implements the **same UI-tree renderer contract** as every other shell (UI-1): it subscribes to the core's `Stream` of UI patches and maps the declarative component tree (PRD 05 UI-2, the ~26-component catalog) to GTK widgets — never the other way around; the applet never touches GTK. The `forge-ui` crate already gives the renderer everything it needs at the type level: `Node`, `Patch`, `Path`, `diff`, `apply`, `Node::Unknown` (UI-6 fallback), `WIRE_VERSION`.

Mapping sketch for the M0a catalog subset present in `forge-ui` today (`Stack/Text/Button/TextField/List`), extended to the full UI-2 catalog at GUI time:

| `forge_ui::Node` | GTK4 / libadwaita widget |
|---|---|
| `Stack { direction: v/h }` | `gtk::Box` with `Orientation::Vertical` / `Horizontal` |
| `Text` | `gtk::Label` |
| `Button { label, onTap }` | `gtk::Button`; `connect_clicked` → emit the `ActionRef` as a core command |
| `TextField { value, onChange }` | `gtk::Entry`; `connect_changed` → emit `onChange` `ActionRef` with the new value |
| `List` (virtualized) | `gtk::ListView` + `gtk::SignalListItemFactory` (virtualization handle, UI-13) |
| `Grid` / `Card` / `Divider` / `Spacer` | `gtk::Grid` / `adw::Bin`+CSS / `gtk::Separator` / sized `gtk::Box` |
| `Tabs` / `Modal` / `Form` | `adw::ViewStack`/`gtk::Notebook` / `gtk::Window` modal / `gtk::Box` with validation styling |
| `Node::Unknown` (UI-6) | a labeled fallback box (`gtk::Frame` + `gtk::Label`) — render usably degraded, never crash |

The patch path: on each `Patch` the renderer walks the widget tree by `Path` (index path) and applies the minimal op (`update_text`, `update_prop`, `insert`, `remove`, `replace`), exactly mirroring `forge_ui::apply`. Events flow the other way as `CoreCommand`s through `WorkspaceCore::handle` (CR-A1) — the GTK shell mutates **nothing** directly (storage/CRDT/schema/policy/runtime), satisfying the no-business-logic rule.

### 7.3 Platform services the GTK shell provides (PS-3)

When implemented, the GTK shell supplies the same platform-service surface every shell must (PS-3), with these Linux mappings:
- **Secrets store:** the **Secret Service API** (libsecret / GNOME Keyring / KWallet) — this is the Linux entry named in PS-3. Wired behind the planned `secrets` crate abstraction.
- **File pickers returning handles** (never raw paths to applets, PS-3): `gtk::FileDialog`; the shell hands the core an opaque handle, not the path.
- **Notifications:** `gio::Notification` / the freedesktop notification spec.
- **Deep links** `forge://`: a freedesktop `.desktop` `MimeType=x-scheme-handler/forge` registration.
- **OS permission prompts → capabilities:** mapped through the same policy/grant UI the other shells use.

### 7.4 Embedded-server reuse

The GTK shell does **not** reimplement the server. It would embed the **same** `forge-server` crate the headless daemon uses (SS-15 "use this computer as a server" toggle, tray/status, port config), exactly as the macOS shell does (PS-7). The headless daemon (Part A) and the GUI are two front-ends over one server crate — no divergence.

---

## Section 8 — Renderer-conformance note (UI-14) — load-bearing

**Normative even though the GTK shell is post-GA.** A GTK renderer is not "done" until it passes the **same renderer-conformance kit as every other shell** (UI-14): the shared golden trees + scripted-interaction + screenshot tests. Behavioral divergence is **release-blocking — the same bar as engine conformance (CR-12)**. This is the mechanism that guarantees "one core, native everywhere" actually holds for Linux.

Concretely, the GTK renderer must pass the existing golden corpus already in the tree:

- **Golden trees:** `forge/crates/ui/tests/golden/` — the ~20-case corpus indexed by `forge/crates/ui/tests/golden/manifest.json`, driven by `forge/crates/ui/tests/golden.rs`. It exercises three case kinds:
  - `roundtrip_*` — a tree must serialize→deserialize→serialize identically (the renderer must build the right widget tree for each).
  - `diff_*` — `diff(old, new)` must produce **exactly** the listed minimal index-path patches (e.g. `diff_text_change` → one `update_text` at `[0]`; `diff_nested_button_action_change` → one `update_prop` at `[1,0]`); the renderer must apply those patches to its live widget tree and reach the new state.
  - `unknown_*` — `Node::Unknown` and unknown props must render as the labeled fallback and **never error** (UI-6), e.g. `unknown_future_widget_child.json`, `unknown_button_extra_prop.json`.
- The same corpus is explicitly called "the renderer-conformance seed (UI-14)" in the header comment of `forge/crates/ui/tests/golden.rs` — so the GTK renderer is conformance-tested against the **identical fixtures** that gate the Rust diff/patch core, renderer zero (UI-13), and the macOS/web/WinUI renderers.

**Conformance harness for the GTK renderer (post-GA test sketch):**
1. **Golden-tree render parity:** for each `roundtrip_*` fixture, render the `Node` to a GTK widget tree and assert the structural mapping (widget type per node, child order, key props) — a headless GTK render under `GDK_BACKEND` offscreen / a virtual display (`xvfb-run` / a headless Wayland compositor) keeps it CI-runnable with no real display.
2. **Scripted interaction (UI-12/UI-14):** replay the golden interaction sequences — simulate `onTap`/`onChange`, assert the resulting patch sequence and the resulting widget state (e.g. "simulate `onTap` → expect Modal in next patch", UI-12).
3. **Screenshot tests (UI-14):** capture the rendered surface and compare against shared baselines (same screenshot suite all renderers share).
4. **Fallback fuzz (UI §10 / UI-6):** unknown-component/prop fuzz → zero crashes, 100% fallback rendering.

CI command shape (post-GA):

```bash
# The pure diff/patch contract already gates today and must stay green:
cargo test -p forge-ui --test golden       # roundtrip + diff + unknown corpus
# Post-GA, additionally:
xvfb-run -a cargo test -p forge-gui --test renderer_conformance   # golden render + interaction + screenshots
```
If any case diverges, the GTK shell does not ship — same release-blocking bar as CR-12.

---

## Section 9 — Linux desktop distribution formats (post-GA)

When the GTK GUI ships, distribute it through the standard Linux channels. Recommended priority: **Flatpak first** (best sandbox + reach + handles the GTK4/libadwaita runtime), then **AppImage** (single portable file), then `.deb`/`.rpm` for distro-native installs.

### 9.1 Flatpak (recommended primary)

Bundles the GTK4/libadwaita runtime, sandboxes the app, distributes via Flathub. Manifest sketch `org.forge.Gui.yaml`:

```yaml
app-id: org.forge.Gui
runtime: org.gnome.Platform
runtime-version: '47'
sdk: org.gnome.Sdk
sdk-extensions:
  - org.freedesktop.Sdk.Extension.rust-stable
command: forge-gui
finish-args:
  - --share=network                       # sync to home server (SS-1)
  - --socket=wayland
  - --socket=fallback-x11
  - --device=dri
  - --talk-name=org.freedesktop.secrets    # Secret Service for the secrets store (PS-3)
modules:
  - name: forge-gui
    buildsystem: simple
    build-commands:
      - cargo --offline build --release -p forge-gui
      - install -Dm755 target/release/forge-gui /app/bin/forge-gui
    sources:
      - type: dir
        path: .
```
```bash
flatpak-builder --force-clean build-dir org.forge.Gui.yaml
flatpak-builder --run build-dir org.forge.Gui.yaml forge-gui
```
The `--talk-name=org.freedesktop.secrets` permission is what lets the sandboxed app reach the Secret Service (PS-3 secrets). `--share=network` is required for sync; the relay/LAN reach honesty (SS-16/SS-18) still applies.

### 9.2 AppImage (single portable file)

A self-contained `.AppImage` that runs on most distros without install. Build with `linuxdeploy` + the GTK plugin (it bundles the GTK4 libs and theme engine):

```bash
cargo build --release -p forge-gui
linuxdeploy --appdir AppDir \
  --executable target/release/forge-gui \
  --desktop-file packaging/forge-gui.desktop \
  --icon-file packaging/forge-gui.png \
  --plugin gtk          # bundles GTK4/libadwaita runtime + GdkPixbuf loaders
linuxdeploy --appdir AppDir --output appimage
./Forge_GUI-x86_64.AppImage      # runs without install
```

### 9.3 `.deb` / `.rpm` (distro-native)

For users/admins who want apt/dnf-managed installs. `cargo-deb` and `cargo-generate-rpm` produce packages directly from Cargo metadata:

```bash
cargo install cargo-deb cargo-generate-rpm
cargo deb -p forge-gui                       # -> target/debian/forge-gui_<ver>_amd64.deb
cargo build --release -p forge-gui && cargo generate-rpm -p crates/shell-gtk
#                                              -> target/generate-rpm/forge-gui-<ver>.x86_64.rpm
```
These declare runtime deps on the distro GTK4/libadwaita packages (`libgtk-4-1`, `libadwaita-1-0` / `gtk4`, `libadwaita`) rather than bundling them, so they're the smallest downloads but require the matching distro libraries. Ship a `.desktop` file + icon + the `forge://` `x-scheme-handler` MIME registration in the package data.

### 9.4 The headless server is unaffected

Note the distinction: the **headless server** (Part A) ships as the **binary + Docker image + systemd unit** (SS-19) — it does **not** use Flatpak/AppImage/.deb-GUI packaging, because it has no GUI and no GTK dependency. The formats in this section are **only** for the post-GA desktop GUI. The two distribution stories never mix: a self-hoster installs the server; a desktop user installs the GUI; both link the same `forge-core`.

### 9.5 Acceptance check (post-GA, GUI)

- `cargo test -p forge-ui --test golden` green (the conformance seed) — gates today, pre-GUI.
- `xvfb-run -a cargo test -p forge-gui --test renderer_conformance` green — golden trees + interaction + screenshots all pass (UI-14), **release-blocking**.
- One of {Flatpak, AppImage, .deb, .rpm} installs and launches on a clean Ubuntu + a clean Fedora VM; the demo workspace renders identically to the macOS/web shells (PRD 06 §9: "same workspace file opens on every shipped platform").
- Secret Service reachable from inside the Flatpak sandbox (secrets store works, PS-3).

---

## Section 10 — Summary & ordering

1. **Now (v1):** ship the headless artifacts — portable glibc CLI (§3), fully static musl CLI (§4), Docker image + systemd unit for the server when it lands (§5), all runnable on WSL (§6). These require only a C toolchain + rustup; no GUI libraries.
2. **Track the budget:** record the stripped binary size against the PRD 01 §8 "< 12 MB native" gate in CI.
3. **Post-GA, gated on demand (PS-13 open question):** add the `shell-gtk` Rust crate, implement the `forge_ui::Node`→GTK renderer with **no FFI** (direct Rust calls, §7), make it pass the **shared UI-14 golden/interaction/screenshot conformance kit** (§8, release-blocking, same bar as CR-12), then distribute via Flatpak/AppImage/.deb/.rpm (§9).

The throughline: **one core, every artifact a thin wrapper.** The CLI, the Dockerized server, and the eventual GTK GUI all call `forge-core`'s `Command/Event/Stream` contract and (for the GUI) render the same component tree against the same golden corpus — so "native everywhere" is enforced by conformance, not by hope.
