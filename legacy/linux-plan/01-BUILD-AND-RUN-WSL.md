# Building & running the workspace on Linux/WSL

> **Scope.** This is the concrete, copy-pasteable build guide for the `forge`
> Rust workspace on Linux and on Windows via WSL2 (Ubuntu). It targets the
> **headless v1 role** of Linux (PRD 06 §5, **PS-13**; DECISIONS D5): build and
> test the Rust workspace, run the `forge` CLI harness, and — once the
> `forge-server` crate lands — run the embedded sync server as a daemon
> (PRD 03 **SS-15..19**). **No GUI in v1.** GTK4/`gtk-rs` + libadwaita is
> explicitly post-GA and is out of scope for this document.
>
> Everything here is verified against the working tree at
> `/Users/vehasuwat/Project/terrane/forge`: toolchain `stable` (built on
> `rustc 1.96.0`), `rusqlite 0.40.1` (bundled SQLite via `libsqlite3-sys 0.38.1`),
> `rquickjs 0.12.0`, `loro 1.13.1`, `swc_core 68.0.6`.

---

## 1. What you are building (and why the deps matter)

The workspace (`forge/Cargo.toml`, `resolver = "2"`) is eleven crates:

```
crates/
  domain/     pure types + validation        — wasm32-clean (no I/O)
  storage/    rusqlite (bundled SQLite)       — NATIVE ONLY (compiles C)
  crdt/       loro wrappers                   — pure Rust, wasm-capable
  schema/     dynamic relational schema       — wasm32-clean (no std::time/fs)
  policy/     RBAC + capability engine        — wasm32-clean
  runtime/    JsEngine trait + rquickjs       — JsEngine trait is wasm-clean;
                                                QuickJsEngine is NATIVE ONLY
  pipeline/   SWC transpile + static scan     — wasm32-clean (default-features off)
  ui/         component-tree diff/patch       — wasm32-clean
  core/       command/event/stream facade     — native (pulls storage+runtime)
  cli/        the `forge` M0a harness binary  — native
  testkit/    deterministic fixtures/harness  — native
```

Two crates link **C** and therefore need a C toolchain at build time:

- **`forge-storage`** → `rusqlite = { version = "0.40.1", features = ["bundled"] }`.
  The `bundled` feature compiles the **amalgamated SQLite C source** that ships
  inside `libsqlite3-sys`. No system `libsqlite3` is used or needed — but a C
  compiler (`cc`) **is**.
- **`forge-runtime`** → `rquickjs = "0.12"`, gated to native targets
  (`[target.'cfg(not(target_arch = "wasm32"))'.dependencies]`). `rquickjs`
  bundles and compiles the **QuickJS C engine**.

Everything else is pure Rust:

- **`loro 1.13.1`** (CRDT) — pure Rust, no C, builds for native **and** wasm.
- **`swc_core 68`** (`forge-pipeline`) — pure Rust, `default-features = false`
  with only the `ecma_*` features enabled (no `tty-emitter`), so it stays
  wasm-clean.
- **`sha2 0.10`** (`forge-domain`) — pure Rust.

**Net:** the only *system* packages the build needs beyond `rustup` are a C
compiler + `make` + `pkg-config` (`build-essential` covers all three on Debian/
Ubuntu). `bundled` SQLite means **no `libsqlite3-dev` is required**. TLS/OpenSSL
(`libssl-dev`) is **not** needed today — no crate in the current tree links
OpenSSL — but you will want it pre-installed for when `forge-server` lands
(its WebSocket/TLS 1.3 transport, SS-1/SS-21), so the one-shot script installs
it proactively.

---

## 2. WSL2 setup (Ubuntu) — skip if you are on bare Linux

> If you are on a native Linux box, jump to **§3**. These steps are for Windows
> users running WSL2. WSL is a **first-class** build/run target for `forge`.

### 2.1 Install WSL2 + Ubuntu (PowerShell, as Administrator)

```powershell
wsl --install -d Ubuntu-24.04
wsl --set-default-version 2
wsl --update
```

Reboot if prompted. Launch **Ubuntu** from the Start menu and create your UNIX
user. Confirm you are on WSL **2** (not 1):

```powershell
wsl -l -v          # VERSION column must read 2 for Ubuntu
```

### 2.2 Critical WSL hygiene — work inside the Linux filesystem

**Do not** clone or build under `/mnt/c/...` (the Windows drive). Cross-OS
filesystem I/O over the 9P bridge is slow and causes `cargo` file-lock races and
permission churn. Clone into your Linux home (`~`), e.g. `~/src/terrane`.

Add a `.wslconfig` in your **Windows** home (`C:\Users\<you>\.wslconfig`) to give
the build room — `swc` + bundled SQLite + QuickJS compile a fair amount of C/Rust:

```ini
[wsl2]
memory=8GB
processors=4
swap=4GB
```

Then `wsl --shutdown` (PowerShell) and reopen Ubuntu to apply.

### 2.3 Update the base image

```bash
sudo apt-get update && sudo apt-get -y upgrade
```

---

## 3. System build dependencies (exact commands)

### 3.1 Debian / Ubuntu (apt) — the path WSL Ubuntu uses

```bash
sudo apt-get update
sudo apt-get install -y \
  build-essential \
  pkg-config \
  curl \
  ca-certificates \
  git \
  libssl-dev      # not needed today; pre-staged for forge-server (SS-1/SS-21)
```

What each gives you:

| Package | Provides | Needed by |
|---|---|---|
| `build-essential` | `gcc`, `g++`, `make`, libc headers | bundled SQLite + QuickJS C compile |
| `pkg-config` | `pkg-config` binary | `cc`/`-sys` crate probing |
| `curl`, `ca-certificates` | TLS fetch | `rustup` installer + crates.io |
| `git` | clone | getting the repo |
| `libssl-dev` | OpenSSL headers | **future** `forge-server` TLS |

> `clang` works too: if you prefer it over `gcc`, `sudo apt-get install -y clang`
> and `export CC=clang`. The bundled SQLite and QuickJS sources build under both.

### 3.2 Fedora / RHEL / openSUSE (dnf) — bare-Linux alternative

```bash
# Fedora / RHEL / CentOS Stream
sudo dnf install -y \
  @development-tools \
  gcc gcc-c++ make \
  pkgconf-pkg-config \
  curl ca-certificates git \
  openssl-devel
```

```bash
# openSUSE (zypper)
sudo zypper install -y -t pattern devel_C_C++
sudo zypper install -y gcc gcc-c++ make pkg-config curl git libopenssl-devel
```

### 3.3 Sanity-check the C toolchain

```bash
cc --version          # gcc (or clang) must resolve
make --version
pkg-config --version
```

If `cc` is missing here, the Rust build will fail in `libsqlite3-sys` /
`rquickjs` with a `linker `cc` not found` or `failed to run custom build
command` error — see **§9 Troubleshooting**.

---

## 4. Rust toolchain (rustup)

`forge/rust-toolchain.toml` pins:

```toml
[toolchain]
channel = "stable"
# libsqlite3-sys 0.38+ requires cfg_select! (stable >= 1.93); built on 1.96.0
```

So the workspace tracks **stable**, with a hard floor of **Rust 1.93** (the
`cfg_select!` requirement from `libsqlite3-sys 0.38`). It is verified building on
**1.96.0**. Install rustup and let the `rust-toolchain.toml` auto-select the
channel on first `cargo` invocation inside the repo.

### 4.1 Install rustup

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y \
  --default-toolchain stable --profile default
source "$HOME/.cargo/env"
```

`--profile default` includes `rustc`, `cargo`, `rustfmt`, **and `clippy`** (you
need clippy for the lint gate in §6.5).

### 4.2 Make sure you are ≥ the 1.93 floor

```bash
rustup update stable
rustc --version    # expect 1.96.0 or newer; MUST be >= 1.93.0
```

> If a corporate image pins you below 1.93, `libsqlite3-sys 0.38.1` will fail to
> compile (`cfg_select!` is unstable/absent). Fix by `rustup update stable`.

### 4.3 Add the wasm32 target (for the wasm lane, §7)

```bash
rustup target add wasm32-unknown-unknown
rustup target list --installed   # confirm wasm32-unknown-unknown is listed
```

### 4.4 Component check

```bash
rustup component add clippy rustfmt   # idempotent; needed by §6.5 / formatting
```

---

## 5. Clone the repository

> Work inside the **Linux** filesystem (see §2.2). Replace the URL with your
> actual remote.

```bash
mkdir -p ~/src && cd ~/src
git clone <YOUR_TERRANE_REMOTE_URL> terrane
cd terrane/forge
```

On first `cargo` command in `forge/`, rustup reads `rust-toolchain.toml` and
selects/downloads the `stable` channel automatically. Confirm you are in the
right place:

```bash
test -f Cargo.toml && grep -q 'resolver = "2"' Cargo.toml && echo "in forge/ ✓"
```

---

## 6. Build, test, run, lint (the native lane)

All commands run from `forge/`.

### 6.1 Build the whole workspace

```bash
cargo build --workspace
```

First build compiles bundled SQLite + QuickJS C and the full SWC tree; expect a
few minutes cold, seconds warm. **Acceptance:** exits `0`, no errors.

### 6.2 Run the full test suite

```bash
cargo test --workspace
```

**Acceptance:** all tests green. The suite is large (**~370+ tests** across unit,
integration, golden-tree, corpus/hostile-applet, and data-driven fixture tests).
Expect output ending with a series of:

```
test result: ok. <N> passed; 0 failed; ...
```

across every crate. **Zero failures** is the gate. If you want the headline
number:

```bash
cargo test --workspace 2>&1 | grep -E '^test result:' \
  | awk '{p+=$4} END {print p" tests passed"}'
```

### 6.3 Run the CLI harness demo (the M0a spine proof)

This is the central acceptance test of the whole platform (PRD 01 **CR-12**;
PRD 06 **PS-5**): `TS → SWC → QuickJS → capability ctx → SQLite write → UI tree
patch → deterministic replay`, fully offline.

```bash
cargo run -p forge-cli -- demo
```

**Acceptance:** the process prints the emitted UI tree, the stored `notes`
records, the run id/result/fingerprint, and — the load-bearing line — exits `0`
with:

```
REPLAY IDENTICAL: true
```

The binary (`forge`) exits **non-zero** if the run fails or replay diverges, so
this is CI-gateable directly. Quick assertion:

```bash
cargo run -q -p forge-cli -- demo | grep -q 'REPLAY IDENTICAL: true' \
  && echo "SPINE GREEN ✓"
```

> `forge help` lists the subcommands; `demo` is the one real subcommand at M0a.

### 6.4 Build a release binary (for daemon/packaging later)

```bash
cargo build --release -p forge-cli
./target/release/forge demo      # same spine, optimized build
```

The release binary is what the headless daemon and self-host packaging
(SS-19, covered in a later plan doc) will wrap.

### 6.5 Lint gate — clippy, warnings as errors

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

**Acceptance:** exits `0`. `-D warnings` promotes every clippy/rustc warning to
an error, matching CI. (Optional, also run by CI:)

```bash
cargo fmt --all --check    # formatting gate; exits 0 if formatted
```

---

## 7. The wasm32 lane (pure-logic crates)

Linux/WSL is where the wasm build is exercised in CI (PS-13 headless path; the
web shell PS-10 consumes these crates compiled to wasm). The **pure-logic**
crates are wasm-clean **today** and must stay that way — building them for
`wasm32-unknown-unknown` is a standing gate.

### 7.1 The command

```bash
cargo build \
  -p forge-domain \
  -p forge-schema \
  -p forge-policy \
  -p forge-pipeline \
  -p forge-ui \
  --target wasm32-unknown-unknown
```

**Acceptance:** exits `0`. These five crates compile to wasm with **no C, no
`std::time`/`std::fs`, no native deps**. (`forge-crdt`/`loro` is also pure Rust
and wasm-capable, but is validated separately as part of the storage-on-wasm
work; the five above are the guaranteed-clean set.)

### 7.2 Why `storage` and `runtime` are NOT in the wasm lane

They are **native-only by construction**, and the workspace is structured so this
is explicit rather than accidental:

- **`forge-storage`** links **bundled SQLite (C)** via `libsqlite3-sys`. The C
  amalgamation does not target `wasm32-unknown-unknown`. The browser path uses a
  **different backend** — SQLite-WASM on OPFS (PRD 06 **PS-10/PS-11**) — not this
  crate's C build.
- **`forge-runtime`** gates `rquickjs` behind
  `[target.'cfg(not(target_arch = "wasm32"))'.dependencies]` because `rquickjs`
  ships native QuickJS **C** and does not build for wasm. The `JsEngine` *trait*
  and all pure record/replay logic stay wasm-clean; the concrete
  `QuickJsEngine` is `#[cfg(not(target_arch = "wasm32"))]`. The browser path uses
  **QuickJS-WASM** in a worker (PS-10), a future backend behind the same trait.

**Consequence (a later task, not this doc):** the *full* spine on wasm needs a
**`sqlite-wasm` storage backend** and a **`quickjs-wasm` engine backend** slotted
behind the existing `Store` boundary and `JsEngine` trait. The architecture
already reserves the seams (the trait is target-independent; storage is behind a
`Store` facade), so this is additive, not a refactor. Until then, the wasm lane
proves the pure-logic crates compile — it does not run the end-to-end spine on
wasm.

### 7.3 Optional: confirm the native-only crates *fail fast* on wasm

This is documentation, not a gate — it shows the intended boundary:

```bash
# Expected to FAIL: storage links C SQLite, has no wasm backend yet.
cargo build -p forge-storage --target wasm32-unknown-unknown ; echo "exit=$?"
```

You will see a `libsqlite3-sys` build-script error; that is the intended,
honest signal that the wasm storage backend is unimplemented — **not** a
regression.

---

## 8. One-shot setup script

Save as `~/setup-forge-linux.sh`, then `bash ~/setup-forge-linux.sh`. Idempotent;
safe to re-run. It installs system deps + rustup + the wasm target, then verifies
the spine.

```bash
#!/usr/bin/env bash
# Terrane `forge` — Linux/WSL one-shot setup + spine verification.
# Headless v1 path (PRD 06 PS-13). Re-runnable.
set -euo pipefail

REPO_URL="${1:-}"                 # optional: pass your git remote as $1
WORKDIR="${HOME}/src/terrane"

say() { printf '\n\033[1;32m==> %s\033[0m\n' "$*"; }

# --- 1. System packages (Debian/Ubuntu; dnf branch for Fedora/RHEL) ---------
if command -v apt-get >/dev/null 2>&1; then
  say "Installing system build deps (apt)"
  sudo apt-get update
  sudo apt-get install -y build-essential pkg-config curl ca-certificates git libssl-dev
elif command -v dnf >/dev/null 2>&1; then
  say "Installing system build deps (dnf)"
  sudo dnf install -y @development-tools gcc gcc-c++ make pkgconf-pkg-config \
    curl ca-certificates git openssl-devel
else
  echo "Unsupported package manager. Install: gcc/g++/make, pkg-config, curl, git, openssl headers." >&2
  exit 1
fi

# --- 2. Verify C toolchain --------------------------------------------------
say "Verifying C toolchain"
cc --version >/dev/null && make --version >/dev/null && pkg-config --version >/dev/null
echo "cc / make / pkg-config OK"

# --- 3. rustup + stable toolchain (>= 1.93 floor) ---------------------------
if ! command -v rustup >/dev/null 2>&1; then
  say "Installing rustup (stable, default profile incl. clippy)"
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
    | sh -s -- -y --default-toolchain stable --profile default
fi
# shellcheck disable=SC1091
source "${HOME}/.cargo/env"
rustup update stable
rustup component add clippy rustfmt
rustup target add wasm32-unknown-unknown
say "Rust toolchain"; rustc --version; cargo --version

# --- 4. Get the repo (skip if already present or no URL given) --------------
if [ -d "${WORKDIR}/forge" ]; then
  say "Using existing checkout at ${WORKDIR}/forge"
elif [ -n "${REPO_URL}" ]; then
  say "Cloning ${REPO_URL}"
  mkdir -p "$(dirname "${WORKDIR}")"
  git clone "${REPO_URL}" "${WORKDIR}"
else
  echo "No checkout at ${WORKDIR}/forge and no REPO_URL passed. Re-run: bash $0 <git-url>" >&2
  exit 1
fi
cd "${WORKDIR}/forge"

# --- 5. Build / test / spine / lint / wasm lane (the acceptance gates) ------
say "cargo build --workspace";           cargo build --workspace
say "cargo test --workspace";            cargo test --workspace
say "forge demo (spine proof)";          cargo run -q -p forge-cli -- demo | tee /tmp/forge-demo.txt
grep -q 'REPLAY IDENTICAL: true' /tmp/forge-demo.txt && echo "SPINE GREEN ✓"
say "cargo clippy -D warnings";          cargo clippy --workspace --all-targets -- -D warnings
say "wasm32 pure-logic lane";            cargo build \
  -p forge-domain -p forge-schema -p forge-policy -p forge-pipeline -p forge-ui \
  --target wasm32-unknown-unknown

say "ALL GREEN — forge is built, tested, and the M0a spine replays identically."
```

**Acceptance for the whole script:** it ends with the `ALL GREEN` line and exits
`0`. Any failed gate aborts under `set -e`.

---

## 9. Troubleshooting (common WSL/Linux build errors)

| Symptom | Cause | Fix |
|---|---|---|
| `error: linker `cc` not found` or `failed to run custom build command for libsqlite3-sys` / `rquickjs-sys` | No C compiler — `build-essential` not installed | `sudo apt-get install -y build-essential pkg-config` (§3.1). Re-verify with `cc --version`. |
| `cfg_select!` / "unstable feature" error inside `libsqlite3-sys 0.38` | Rust older than the **1.93** floor | `rustup update stable && rustc --version` — must be ≥ 1.93 (built on 1.96.0). |
| `error: failed to acquire packages... blocking waiting for file lock on package cache` (hangs) | Two `cargo`/`rust-analyzer`/IDE processes contending for the lock — common in WSL when an editor indexes while you build | Run one cargo at a time. Kill stragglers: `pkill -f rust-analyzer; pkill -f cargo`. Then retry. Avoid building under `/mnt/c` (slow locks). |
| Extremely slow builds, random `Permission denied`, or files "changing" between runs | Repo lives on the Windows drive `/mnt/c/...` over the 9P bridge | **Move the checkout into the Linux home** (`~/src/...`). This is the single biggest WSL fix (§2.2). |
| `error: /bin/sh^M: bad interpreter` or `\r`-related script failures | Windows CRLF line endings (file edited on the Windows side / git `autocrlf=true`) | `git config --global core.autocrlf input`; normalize a stray script with `sed -i 's/\r$//' script.sh`; re-clone if the whole tree is CRLF. |
| `wasm32-unknown-unknown` target "not installed" | Target not added | `rustup target add wasm32-unknown-unknown` (§4.3). |
| `forge-storage` / `forge-runtime` fail to build for `--target wasm32-unknown-unknown` | **Expected** — these are native-only (C SQLite / C QuickJS) | Not a bug. Only build the five pure-logic crates for wasm (§7.1). |
| `cannot find -lssl` / `openssl-sys` error (only once `forge-server` is added) | OpenSSL dev headers missing | `sudo apt-get install -y libssl-dev pkg-config` (already in §3.1 / the one-shot script). |
| Out-of-memory / killed compiler in WSL during the SWC or C build | Default WSL memory cap too low | Raise `[wsl2] memory`/`swap` in `C:\Users\<you>\.wslconfig`, then `wsl --shutdown` (§2.2). |
| `clippy: command not found` | rustup installed with `minimal` profile | `rustup component add clippy` (§4.4). |

---

## 10. Acceptance checklist (what "green on Linux/WSL" means)

Run from `forge/`. All must pass:

- [ ] **§3.3** `cc --version && make --version && pkg-config --version` → all resolve.
- [ ] **§4.2** `rustc --version` → ≥ 1.93.0 (verified on 1.96.0).
- [ ] **§4.3** `rustup target list --installed` includes `wasm32-unknown-unknown`.
- [ ] **§6.1** `cargo build --workspace` → exit 0.
- [ ] **§6.2** `cargo test --workspace` → **0 failed** (~370+ passed).
- [ ] **§6.3** `cargo run -p forge-cli -- demo` → prints `REPLAY IDENTICAL: true`, exit 0.
- [ ] **§6.5** `cargo clippy --workspace --all-targets -- -D warnings` → exit 0.
- [ ] **§7.1** wasm build of `domain/schema/policy/pipeline/ui` → exit 0.

When all eight pass, the headless v1 path is fully operational on this Linux/WSL
machine. The next plan documents cover running the `forge` CLI as a long-lived
harness and (when `forge-server` lands, SS-15..19) running the embedded sync
server as a systemd daemon and self-host single binary + Docker image.
