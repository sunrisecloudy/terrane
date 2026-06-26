# Forge on Linux / WSL — Plan Index

> **Executive summary (read this first).** On Linux, **Forge v1 is headless**:
> it is the frictionless target to **build + test** the `forge` Rust workspace,
> the host for the **`forge` CLI harness** (the usable "app" today — `forge demo`
> drives `TS → SWC → QuickJS → ctx → SQLite → UI tree → deterministic replay`
> fully offline), and — once the `forge-server` crate lands (M2) — the host for
> the **embedded sync server running as a systemd daemon / Docker container**.
> **There is no Linux desktop GUI in v1** (PS-13, Master PRD §7 non-goals,
> Decision D5); a GTK4/libadwaita GUI is an explicitly separate **post-GA** track.
> **WSL2 is a first-class build/run target**, not an afterthought.

---

## Table of contents

| Doc | Title | What it gives you |
|---|---|---|
| [`00-OVERVIEW.md`](./00-OVERVIEW.md) | Overview & Linux/WSL role | Scope, the one-sentence role, why Linux is the primary build/test target, works-today-vs-planned matrix, target OS/arch matrix, the deliberate no-GUI non-goal. |
| [`01-BUILD-AND-RUN-WSL.md`](./01-BUILD-AND-RUN-WSL.md) | Building & running the workspace | Exact `apt`/`dnf`/`pacman` packages, `rustup` bootstrap, WSL2 hygiene, `cargo build/test/run --workspace`, the `wasm32` lane, a one-shot setup script, troubleshooting. **Start here to get green.** |
| [`02-HEADLESS-SERVER-DAEMON.md`](./02-HEADLESS-SERVER-DAEMON.md) | The embedded server daemon (`forge-server`, mostly planned) | Where `sync`/`server` crates plug in, the WebSocket/TLS sync protocol (SS-1/2), RBAC-before-apply, device-token pairing, mDNS + relay, backups, the systemd unit + Docker image, firewall notes. |
| [`03-PACKAGING-AND-FUTURE-GUI.md`](./03-PACKAGING-AND-FUTURE-GUI.md) | Packaging + the future GTK GUI | **Part A (v1):** portable glibc + static musl binaries, Docker images, how WSL users run it. **Part B (post-GA):** the GTK4/libadwaita GUI sketch (no FFI), renderer conformance, Flatpak/AppImage/`.deb`/`.rpm`. |
| [`04-MILESTONES.md`](./04-MILESTONES.md) | Linux milestones & acceptance gates | The ordered **L0…L4** milestones — each with deliverables, a single green exit-gate command, and a PRD mapping — plus the post-GA GTK track and Linux/WSL-specific risks. |

> Numbering note: the section files are numbered with gaps so platform-shared
> sections can be slotted in later without renumbering.

---

## WSL prerequisites checklist

Tick these before running anything (full detail in `01-BUILD-AND-RUN-WSL.md` §2–§4):

- [ ] **WSL2 + Ubuntu 22.04/24.04 installed.** PowerShell (Admin): `wsl --install -d Ubuntu-24.04 && wsl --update`. Confirm `wsl -l -v` shows **VERSION 2**.
- [ ] **Repo lives in the Linux filesystem (`~/src/...`), NOT `/mnt/c/...`.** Verify with `df -T ~` → `ext4`, not `9p`/`drvfs`. (Biggest WSL perf/correctness fix — 5–20× slower and file-lock races on the Windows drive.)
- [ ] **WSL given enough RAM/CPU.** `C:\Users\<you>\.wslconfig` → `[wsl2]` `memory=8GB`, `processors=4`, `swap=4GB`; then `wsl --shutdown` and reopen.
- [ ] **Base image updated.** `sudo apt-get update && sudo apt-get -y upgrade`.
- [ ] **System build deps installed** (Debian/Ubuntu/WSL): `sudo apt-get install -y build-essential pkg-config curl ca-certificates git libssl-dev`. (Bundled SQLite needs a C compiler; **no `libsqlite3-dev`** required. `libssl-dev` is pre-staged for the future `forge-server` TLS.)
- [ ] **C toolchain resolves:** `cc --version && make --version && pkg-config --version`.
- [ ] **Rust via `rustup` (not the distro `rustc`):** `curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable --profile default && . "$HOME/.cargo/env"`.
- [ ] **Rust ≥ 1.93** (`libsqlite3-sys 0.38+` needs `cfg_select!`; verified on 1.96.0): `rustup update stable && rustc --version`.
- [ ] **WASM target added:** `rustup target add wasm32-unknown-unknown` (confirm via `rustup target list --installed`).
- [ ] **(For the M2 daemon, later):** `systemd=true` in `/etc/wsl.conf` + `wsl --shutdown`; prefer the **relay tunnel** or `networkingMode=mirrored` for LAN reach (WSL2 is NAT'd).

---

## Start here → L0 (the exact first commands)

L0 is "the machine can build Forge": the workspace builds + tests and `forge demo`
replays byte-identically. Full walkthrough in `01-BUILD-AND-RUN-WSL.md`; milestone
definition and PRD mapping in `04-MILESTONES.md` (§ L0).

```bash
# 0. One-time host setup (Debian/Ubuntu/WSL): system deps + rustup + wasm target.
sudo apt-get update
sudo apt-get install -y build-essential pkg-config curl ca-certificates git libssl-dev
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
  --default-toolchain stable --profile default
. "$HOME/.cargo/env"
rustup update stable                       # ensure >= 1.93 (built on 1.96.0)
rustup target add wasm32-unknown-unknown

# 1. Clone into the Linux filesystem (NOT /mnt/c) and enter the workspace root.
mkdir -p ~/src && cd ~/src
git clone <YOUR_TERRANE_REMOTE_URL> terrane
cd terrane/forge

# 2. The L0 exit gate — all three must exit 0, and the demo must replay.
cargo build --workspace                                   # exit 0
cargo test  --workspace                                   # 0 failed (~370+ passed)
cargo run -p forge-cli --bin forge -- demo \
  | grep -q 'REPLAY IDENTICAL: true' && echo "L0 GREEN"
```

When `L0 GREEN` prints, the headless v1 path is operational on this box. Next:
extend the CLI into the on-disk dev loop (**L1**), then the sync seam (**L2**),
the embedded daemon (**L3**), and self-host packaging + Linux CI (**L4**) — all
in `04-MILESTONES.md`.
