# Phase E — Crypto seam (step E13)

**Theme:** unify the Ed25519 token + signature format. Intentionally **last**: it is
security-critical but **low raw-line** (~550 LOC) and **orthogonal** to the bigger wins, so it does
not block them. Key custody stays per-platform.

---

## E13 — Consolidate token/signing into `forge-signing`

**Moves (format + algorithm):**
- One canonical control **token** format + generation — macOS `DevControlPlane.swift:2832-2860`
  (`generateToken`), Linux `dev_control_plane.c:209-230`.
- One canonical **signature** payload (`terrane/sig/v1` header, canonical field ordering, Ed25519,
  base64) + verification — macOS `DevControlPlane.swift:3528-3578` (`signPayload`,
  `signaturePayload`) and `:5644-5690` (verify/hash), Windows `DevControlPlane.cpp:7046-7068`.

Define both in `forge-signing` and expose generate/verify through the JSON seam (or a small signing
command group). Reuse existing `forge-signing` primitives.

**Stays per-platform — key custody.** Behind a thin `KeyStore` interface the crypto seam calls:
- Keychain (macOS/iOS), Windows CNG, libsecret (Linux), Android Keystore.

The shell provides "load/store this key"; the core does "construct payload, sign, verify, mint
token." Custody (where the private key physically lives) never leaves the platform.

**Migration order:** macOS first, then fan out. Tokens/signatures minted before and after **must be
byte-for-byte interoperable** — verify with a vector set captured from the current implementation.

**Validation:** `cargo test -p forge-signing` (sign/verify + token vectors, must match existing
tokens byte-for-byte); security review; per-shell test that tokens minted/verified pre/post are
interoperable.

**Risk:** high (security). **App-visible:** no (the control token is debug-surface auth; package
signatures are app-visible via `signature.verify`, already in the core). **Effort:** L.

---

## Open decision

Confirm scope in [09-decisions-and-open-questions.md](09-decisions-and-open-questions.md): is the
crypto unification **in scope for this pass**, or deferred? Because it is low-line and orthogonal, it
can be dropped from the program without affecting A–D. Default recommendation: **do it, last**, since
"one authoritative signing format" is a genuine security win and a divergent signature format across
platforms is a latent interop bug.

---

## Phase E exit criteria

- One canonical token + `terrane/sig/v1` signature format in `forge-signing`, used by all shells.
- Key custody remains per-platform behind a `KeyStore` interface — no private keys cross the seam.
- Pre/post tokens and signatures interoperate byte-for-byte; security review passed.

---

## Program complete

After E13, the native shells are thin: each is OS glue (WebView, listeners, SQLite transport, key
custody, OS install/uninstall) plus data-file loading plus delegation to the core. Adding a sixth
platform is implementing the glue and loading the data — not re-deriving ~18K lines of logic. See
[10-validation-and-sequencing.md](10-validation-and-sequencing.md) for the end-to-end validation
story.
