# Forge on Linux / WSL — 00 · Overview & Linux/WSL Role

> **Scope of this document:** what the Linux plan covers, why it is deliberately
> smaller than the Windows plan, and exactly what "Linux/WSL" means as a Forge
> target in v1. This is the map; the numbered documents that follow are the
> turn-by-turn directions (toolchain setup, building the workspace, running the
> CLI harness, and — when it lands — running the embedded sync server as a
> daemon).
>
> **Audience & assumptions:** you are sitting at a WSL2 Ubuntu shell or a native
> Linux box, you have a terminal, and you want to build, test, and run the Forge
> Rust core headlessly. Everything here is concrete: exact packages, exact
> commands, exact acceptance checks. No hand-waving.

---

## 1. The one-sentence role

On Linux, **Forge v1 is headless**: it is the frictionless build/test target for
the `forge` Rust workspace, the host for the `forge` CLI harness (the usable
"app" today), and — once `forge-server` lands — the host for the **embedded sync
server running as a daemon**. There is **no Linux desktop GUI in v1** (PS-13,
Master PRD §7 non-goals, Decision D5). A GTK4/libadwaita GUI is explicitly a
post-GA revisit.

This is why the Linux plan is small: Windows has to ship a full WinUI 3 shell
(PS-14), an installer, a renderer, and platform services. Linux ships a binary
and a systemd unit. The hard part on Linux is **nothing new** — it is the same
Rust core that already builds and runs on this machine today.

---

## 2. Why Linux is the *primary* build + test + run target

The other three points in the rollout (macOS, Web, Windows) each need a shell
toolchain — Xcode/Swift, wasm-bindgen + a browser harness, .NET + WinUI. Linux
needs **only the Rust toolchain plus a C toolchain and a few `-dev` headers**.
There is no shell to build, no GUI SDK, no codesigning, no notarization. The
workspace is pure Rust with vendored/bundled native dependencies:

| Crate | Native dependency | How it builds on Linux |
|---|---|---|
| `forge-storage` | SQLite | `rusqlite` with `features = ["bundled"]` — SQLite is **compiled from vendored C source**, no system `libsqlite3` needed. Requires a C compiler (`cc`/`clang`) only. |
| `forge-runtime` | QuickJS | `rquickjs = "0.12"` ships the QuickJS C source and builds it via `cc`. Gated `#[cfg(not(target_arch = "wasm32"))]`, so it builds natively on Linux and is skipped for the WASM target. |
| `forge-pipeline` | SWC (pure Rust) | `swc_core = "68"` with no `tty-emitter` feature — pure Rust, no system deps, also `wasm32-unknown-unknown`-clean. |
| `forge-crdt` | Loro (pure Rust) | `loro` crate — no system deps. |
| everything else | — | pure Rust (`serde`, `thiserror`, `sha2`). |

The practical consequence: a fresh Ubuntu container goes from zero to
`forge demo` green with **one `apt-get` line and one `rustup` install** (covered
in `02-PREREQUISITES.md`). That makes Linux/WSL the canonical CI builder and the
fastest local inner-loop target for core development — which is exactly the role
the Master PRD assigns it ("Linux headless build … same crate as desktop server
mode", §7).

### 2.1 WSL2 is a first-class target, not an afterthought

A developer on Windows runs the **Linux** Forge path inside **WSL2** (Ubuntu
22.04+). Everything in this plan is written to work identically on:

- **WSL2 / Ubuntu 22.04+** (the reference WSL environment), and
- **native Linux** (Debian 12+, Fedora 39+, Arch).

WSL-specific notes (filesystem performance, `systemd` enablement, port
forwarding to the Windows host, mDNS caveats) are called out inline in the
relevant documents and collected in `07-WSL-NOTES.md`. The rule of thumb: build
inside the WSL filesystem (`~/…`, i.e. `ext4`), **not** under `/mnt/c`, or you
pay a 5–20× I/O penalty on `cargo` builds.

---

## 3. The three Linux deliverables (and their state)

### Deliverable A — Build & test the Rust workspace (works today)

`git clone` → `cargo build --workspace` → `cargo test --workspace`. This is the
M0a spine: the crates `domain, storage, crdt, schema, policy, runtime, pipeline,
ui, core, cli, testkit` compile and their tests pass on Linux today. CI runs the
same commands. Covered by `03-BUILD-WORKSPACE.md`.

### Deliverable B — The `forge` CLI harness (works today)

The usable Linux "app" right now is the `forge` binary (crate `forge-cli`,
PS-5). It speaks the real Command/Event/Stream contract (CR-A1..A5) the way
every shell does. Its one real subcommand today is:

```bash
forge demo
```

which drives the **entire executable spine end to end, fully offline**:

```
TS source ─SWC transpile─▶ JS ─QuickJS realm (zero ambient capability)─▶
  capability-checked ctx ─▶ SQLite write ─▶ UI component-tree patch ─▶
  deterministic RunRecord ─▶ replay (asserted byte-identical)
```

It exits non-zero if the run fails or replay diverges, so it doubles as the M0a
acceptance gate. Covered by `04-RUN-CLI-HARNESS.md`. As the command catalog
(`forge/spec/commands.md`, CR-A2) grows, this same binary grows real
subcommands (`workspace.create`, `applet.install`, `runtime.run/replay`, …) — it
is the SDK CLI's ancestor.

### Deliverable C — The embedded sync server daemon (planned / M2)

This is the v1 Linux *deliverable* in the product sense: the **same `forge-server`
crate** that the desktop app embeds, run on Linux as a **headless daemon** —
Sync & Server "Deployment B" (SS-15..19). It provides self-hosted sync over
WebSocket/TLS, LAN discovery (mDNS) + outbound relay tunnel, backup/export
scheduling, an optional local LLM gateway, a local marketplace mirror, and a
type-check service for paired thin clients.

**This crate does not exist in `forge/crates/` yet.** Today's workspace stops at
the M0a spine (the `server/` and `sync/` crates from PRD 01 §2 are planned, not
present — compare `forge/Cargo.toml`'s member list). So most of Deliverable C is
**planned/M2 work**, documented here as a target with concrete packaging
(systemd unit, Docker image, config schema, firewall/port guidance) so that when
the crate lands the Linux daemon is a drop-in. Covered by
`05-EMBEDDED-SERVER-DAEMON.md` and `06-PACKAGING.md`.

---

## 4. Works today vs. planned

| Capability | State on Linux | Where it lives | Doc |
|---|---|---|---|
| Build the whole Rust workspace | ✅ Works today | `forge/crates/*` | 03 |
| `cargo test --workspace` green | ✅ Works today | `forge/crates/*` | 03 |
| `wasm32-unknown-unknown` build check (pipeline/ui/domain) | ✅ Works today | `forge-pipeline`, `forge-ui`, `forge-domain` | 03 |
| `forge demo` end-to-end spine + replay | ✅ Works today | `forge-cli` | 04 |
| QuickJS sandbox (native, zero ambient capability) | ✅ Works today | `forge-runtime` (`rquickjs`) | 04 |
| SWC transpile + static policy scan, offline | ✅ Works today | `forge-pipeline` | 04 |
| SQLite-backed records/oplog/runs, WAL | ✅ Works today | `forge-storage` (`rusqlite` bundled) | 03/04 |
| Loro CRDT docs + merge/replay | ✅ Works today | `forge-crdt` | 03 |
| Full command catalog (`workspace.*`, `applet.*`, `runtime.replay`, …) | 🟡 Partial — facade + demo path only | `forge-core`, `spec/commands.md` | 04 |
| Offline `tsgo`/TS7 type-check sidecar (CR-15) | 🟡 Planned — SWC strip works today | (sidecar TBD) | 04 (note) |
| Embedded sync server crate (`forge-server`) | ⛔ Planned / M2 | not yet in workspace | 05 |
| Run server as a Linux daemon (systemd) | ⛔ Planned / M2 | depends on `forge-server` | 05 |
| Single-binary + Docker self-host packaging | ⛔ Planned / M2 (SS-19) | depends on `forge-server` | 06 |
| mDNS/LAN discovery + relay tunnel | ⛔ Planned / M2 (SS-16) | depends on `forge-server` | 05/07 |
| Local LLM gateway / marketplace mirror / type-check service | ⛔ Planned / post-M2 (SS-17) | depends on `forge-server` | 05 (note) |
| **Linux desktop GUI (GTK4/libadwaita)** | ❌ **Non-goal in v1** (revisit post-GA) | — | §6 |

Legend: ✅ runs on this machine today · 🟡 partial / under construction ·
⛔ planned, not yet in the codebase · ❌ deliberate non-goal.

---

## 5. Target matrix

### 5.1 Operating systems

| Tier | Target | Notes |
|---|---|---|
| **Reference (WSL)** | WSL2 + Ubuntu 22.04 LTS or 24.04 LTS | First-class build/run target. `systemd` available on WSL ≥ 0.67.6; enable in `/etc/wsl.conf`. |
| **Reference (native)** | Ubuntu 22.04 / 24.04, Debian 12 (bookworm) | `apt` package set in `02-PREREQUISITES.md`. |
| **Supported** | Fedora 39+ | `dnf` package set provided alongside `apt`. |
| **Supported** | Arch / EndeavourOS (rolling) | `pacman` package set provided. |
| **CI** | `ubuntu-latest` GitHub-hosted runner (22.04/24.04) | Same `apt` line; this is the canonical CI builder. |

### 5.2 CPU architectures

- **x86_64** — primary (WSL2, CI, most desktops/servers).
- **aarch64** — supported (Apple-Silicon-hosted WSL is not a thing, but ARM
  Linux servers and ARM CI runners are; the Rust core is arch-agnostic and the
  bundled C deps cross-compile cleanly with the right `cc`).

### 5.3 Rust toolchain

Pinned by `forge/rust-toolchain.toml` to **`stable`**, with the documented
constraint that `libsqlite3-sys 0.38+` needs `cfg_select!` (stable ≥ 1.93). This
machine is on `1.96.0`; **use Rust ≥ 1.93** on Linux. Install via `rustup`
(distro `rustc` packages are often too old). Add the WASM target for the
size/clean-build checks:

```bash
rustup target add wasm32-unknown-unknown
```

Details and the exact `rustup` bootstrap are in `02-PREREQUISITES.md`.

---

## 6. Deliberate non-goal: no Linux desktop GUI in v1

To be unambiguous, because it shapes the whole plan:

- **There is no GTK4, libadwaita, Tauri, or any windowed Forge app for Linux in
  v1.** (PS-13; Master PRD §7 "Non-goals (v1): Linux desktop GUI (headless
  server only)"; Decision D5.)
- The Linux "UI" is the **terminal**: the `forge` CLI harness and (later) the
  daemon's **local-only status page / admin UI** served over HTTP by
  `forge-server` (SS-15, SS-22) — a web page, not a native window.
- A native Linux GUI (GTK4/gtk-rs, evaluated against the renderer conformance
  kit like every other shell) is a **post-GA** item, gated on the open question
  "Linux GUI demand check post-GA" (PS §10.3). It is out of scope for every
  document in this plan.

If you came here looking for a desktop app, the answer for v1 is: run the web
client (M3, PWA) in a browser, point it at a self-hosted `forge-server` daemon
running on this Linux box. That combination *is* the Linux desktop experience in
v1.

---

## 7. Document map for the Linux plan

| Doc | Title | What it gives you |
|---|---|---|
| `00-OVERVIEW.md` | Overview & Linux/WSL role | **(this file)** scope, role, works-today-vs-planned, targets, non-goals |
| `02-PREREQUISITES.md` | Toolchain & system packages | exact `apt`/`dnf`/`pacman` sets, `rustup` bootstrap, WSL2 + systemd enablement |
| `03-BUILD-WORKSPACE.md` | Build & test the workspace | `cargo build/test --workspace`, the `wasm32` check, CI recipe |
| `04-RUN-CLI-HARNESS.md` | Run the CLI harness | `forge demo`, the spine walkthrough, acceptance proof, growing the command catalog |
| `05-EMBEDDED-SERVER-DAEMON.md` | Embedded server daemon (planned) | `forge-server` as a systemd daemon, config, ports, mDNS, relay |
| `06-PACKAGING.md` | Packaging (planned) | single binary, Docker image, config schema, backup/restore (SS-19) |
| `07-WSL-NOTES.md` | WSL specifics | filesystem perf, port forwarding to Windows host, mDNS/LAN caveats |

> Numbering note: documents are numbered with gaps so platform-shared sections
> can be slotted in without renumbering. Start at `02-PREREQUISITES.md`.

---

## 8. Acceptance checks for "Linux v1 baseline is green"

These are the concrete, runnable gates that prove the **works-today** Linux path
is healthy on a fresh box. (Server-daemon acceptance lives in `05`, and only
applies once `forge-server` exists.) Run from the workspace root
`forge/`:

```bash
# 1. The workspace builds clean (native target).
cargo build --workspace                      # exit 0

# 2. All crate tests pass headlessly (no display, no network).
cargo test  --workspace                      # exit 0, all green

# 3. The web-bound crates are wasm32-unknown-unknown-clean (CR-15 §8).
cargo build -p forge-pipeline -p forge-ui -p forge-domain \
  --target wasm32-unknown-unknown            # exit 0

# 4. The executable spine runs end to end and replay is byte-identical.
cargo run -p forge-cli --bin forge -- demo   # exit 0; prints the demo report
#   ↳ exits NON-ZERO if the run fails or deterministic replay diverges (CR-12).
```

**Pass criteria:** all four commands exit `0`, with no network access at any
point (the entire spine is offline by construction — SWC + QuickJS + bundled
SQLite + in-process replay). On WSL2 these must pass with **no X server / no
display** set; nothing here touches a GUI. This is the M0 "green on
macOS/**Linux**/WASM CI targets" exit condition (PS §9, CR-12, Master PRD M0a),
verified on the Linux leg.

---

*End of 00 · Overview & Linux/WSL role. Next: `02-PREREQUISITES.md`.*
