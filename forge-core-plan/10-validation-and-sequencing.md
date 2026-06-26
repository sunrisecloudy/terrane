# 10 — Validation & Sequencing

## Sequencing at a glance

```
 Phase A  DATA + SCHEMA            low/med risk, ~no app-visible change   ← start here
   A1 forge/data/ + loader ─┬─ A2 non-replay enums/config
                            ├─ A3 replay enums (forge-domain)
                            ├─ A4 control-tools catalog + envelope
                            └─ A5 shared SQLite schema (+delete fallback)
 Phase B  DEVCONTROLPLANE   biggest line win, DEBUG-only, low blast radius
   A3,A4,A5 ─▶ B6 forge-controlcore (macOS, golden vectors) ─▶ B7 fan out 4 shells
 Phase C  SECURITY/POLICY CORE   HIGH risk, app-visible, replay-sensitive
   A* ─▶ C8 network/private-IP          C9 manifest ─▶ C10 bridge envelope/perm/budget
   A5+C10 ─▶ C11 recording (replay gap)
 Phase D  APP-LIFECYCLE AUTHORITY  (needs A3+A5+C11)
   D12 package.* / quota.auto_quarantine — core owns webapp registry (macOS + reference-host first)
 Phase E  CRYPTO   security-critical, low-line, orthogonal — LAST
   E13 token/signature in forge-signing
```

**Dependency rules:** A1 precedes all A. **Q8** (package vs applet namespace) precedes A3 enum naming
and D12 command design. **Q9** (dual-DB strategy) precedes A5 migration scope and D12 write path.
**Q10** (reference-host) means B6/D12 fan-out pairs **macOS + reference-host** before other shells.
A3+A4+A5 precede B6. B6 precedes B7. C9 precedes C10. A5+C10 precede C11. A3+A5+C11 precede D12.
E13 depends only on A1.

**Why this order:** front-load raw-line wins and low risk; defer security/authority behind their
prerequisites. Phase A changes almost nothing app-visible. Phase B is the biggest deletion but
debug-only (safest large change). Phase C is where app-visible security logic consolidates — gated by
conformance vectors + security review + contract re-export. Phase D is the most behaviorally
significant (authority move) — last but one. Phase E is orthogonal and low-line — last.

## Validation gates by phase

| Phase | Rust gate | Shell gate | Contract |
|---|---|---|---|
| A | `cargo test -p forge-domain` / `-p forge-storage` + clippy | macOS `swift build && swift test`; per-shell loaded-data == old literal | export + verify when an app-visible data file changes |
| B | `cargo test -p forge-controlcore` (golden vectors) | macOS integration test identical pre/post; cross-platform parity test | none (debug-only) |
| C | `cargo test -p forge-policy/-core/-runtime` (conformance vectors) + clippy | macOS host parity (accept/reject crafted inputs); replay-identical gate | export + verify + **security review**; refresh Premium pin on accepted change |
| D | `cargo test -p forge-core` (lifecycle + auto-quarantine vectors) + `cargo run -p forge-cli -- demo` | per-shell rollback/quarantine outcome parity; SC-12 audit assertion; replay-identical | export + verify; Premium pin per Q4 |
| E | `cargo test -p forge-signing` (byte-for-byte token/sig vectors) | per-shell pre/post interop test; **security review** | none (control token internal; package sig already in contract) |

**Workspace-wide gate before any release-ish checkpoint:**
```sh
cd forge
cargo test --workspace --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo run -p forge-cli -- demo
```
**Public contract:**
```sh
node --no-warnings tools/export-public-contract.mjs --out artifacts/public-contract.json
node --no-warnings tools/verify-public-contract.mjs --contract artifacts/public-contract.json --root .
```
After an **accepted** app-visible contract change, refresh the Premium pin in `../terrane-premium`
intentionally and run its contract verification.

## Build-environment constraints (important for executing)

Verified in this repo's dev environment:

- ✅ **Rust workspace** builds + tests here (`cargo build --workspace` green; ~35s cold).
- ✅ **macOS Swift package** builds + tests here (`swift build` green ~9s; Swift 6.3.2). The macOS
  shell is fully validatable locally, including `TerraneHostMacTests` (2988 lines).
- ❌ **iOS** — SwiftPM defaults to the macOS target; UIKit deps won't build without an iOS SDK target.
- ❌ **Windows (C++/MSVC)**, **Linux (meson/GTK)**, **Android (Gradle)** — not buildable on macOS.

**Implication:** drive every change **macOS-first** and validate locally. The four non-macOS shells
are validated by (a) the **shared Rust tests + golden/conformance vectors** they call into, (b)
careful code review of the mechanical delegation change, and (c) **their own CI**. Each fan-out step
is one shell per commit so CI isolates regressions. Never mark a fan-out step "done" on review alone
if its CI is red.

## Commit & branch hygiene (from project memory)

- Branch off `main`; **stage your own files explicitly; never `git add -A`.** Never commit
  `forge/target/` (gitignored).
- Frequent, granular, **green** commits — one logical move per commit (a data file + its consumer; a
  new command + its handler + vectors; one shell's fan-out). macOS-first then fan out.
- Preserve unrelated dirty/untracked work.

## Risk register (top items)

| Risk | Mitigation |
|---|---|
| Replay divergence when moving recording/lifecycle into core (C11, D12) | replay-identical gate; capture crash-recovery inputs; conformance vectors before migration |
| Security regression in the one network/bridge gate (C8, C10) | conformance vector matrix + security review of the single implementation before fan-out |
| Hidden per-platform behavior differences surface as "drift" when unified (B, C) | golden/parity vectors captured from current output; treat any diff as a decision, not a silent change |
| Public-contract / Premium pin churn (C, D) | only app-visible commands enter the contract (Q4); refresh pin intentionally after acceptance |
| Non-macOS shells can't be built locally | macOS-first; shared vectors; one-shell-per-commit; rely on each shell's CI |
| reference-host drifts from native shells during B/D | treat as sixth consumer; macOS + reference-host paired per phase; same golden vectors |
| `package.*` vs `applet.*` namespace collision (D12) | Q8 default: separate namespaces; `PackageVersionStatus` not `AppletStatus` |
| Dual-DB write path ambiguity (A5, D12) | Q9 default: dual files, unified authority; single-file merge post-D |
| `wasm32`-cleanliness broken by HTML parsing in controlcore (B6) | gate native-only parsing behind `cfg(not(wasm32))`; keep pure matchers wasm-clean |

## Definition of done (program)

- Native shells contain only OS glue + data loading + delegation to the core.
- ~18K lines of duplicated logic removed; one authoritative implementation of each concern.
- One SQLite schema; one network/bridge/manifest/recording/lifecycle/crypto implementation.
- Control surface uniform across platforms (capability matrix explicit in data).
- All gates green; public contract current; Premium pin refreshed where accepted.
- A new platform = implement glue + load data. No logic re-derivation.
