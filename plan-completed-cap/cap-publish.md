# Capability: `publish` — signed export / install of app bundles

New crate `rust/crates/terrane-cap-publish/`, namespace `publish`, registered
in `default_registry`. Lets a home share an app beyond itself: `terrane app
export <id>` produces a signed archive; `terrane app install <archive>` on
another home verifies it and installs with recorded provenance. Builds on the
existing `app.import` flow (bundle files land as `kv.set` events under
`__terrane/app-bundle/`, see [cap-app-update.md](cap-app-update.md)) — this
plan extends that flow, it does not fork it.

## Locked decision

**TOFU per publisher key.** v1 trust is trust-on-first-use: the first install
from a publisher key records `publish.trusted { pubkey, label }`; later
installs signed by the same key proceed silently; an archive for an
already-installed app id signed by a *different* key hard-stops with a prompt
through the existing permission-elicitation flow (web shell hold, MCP
`elicitation/create`, CLI y/N). No registry, no CA, no revocation list — the
key is the publisher.

## Publisher identity and signing

The `replica` capability provides only a 53-bit Loro PeerID (`u64`) — a stable
id, **not** key material — and `terrane-cap-crypto` is a symmetric vault
(Argon2id + XChaCha20-Poly1305), so publishing mints its own keypair:

- **ed25519 via `ed25519-dalek`** (workspace dep), created lazily on first
  export. Private key lives in the host keychain / secret store defined by
  [cap-oauth-connections.md](cap-oauth-connections.md) — never in the event
  log, never in State.
- The public half is a shareable fact: recorded once as
  `publish.identity-created { pubkey, replica_peer }` (binding the key to this
  home's replica id for provenance display). Replay restores the pubkey; the
  private key is edge material like the crypto keyring.

## Archive format

`<id>-<version>.terrane` — a canonical tar: `manifest.json` (with `version`,
per cap-app-update.md), every bundle file (sorted paths, text-only, same
validation rules as `app.import`), plus `publish.json`:

```jsonc
{
  "formatVersion": 1,
  "app": "todo", "version": "1.2.0",
  "bundleHash": "<sha256 of sorted (path, sha256(content)) list>",
  "publisher": { "pubkey": "<base64 ed25519>", "replicaPeer": "0x…", "label": "veha" },
  "signature": "<ed25519 over bundleHash ∥ app ∥ version, base64>"
}
```

`bundleHash` is the same construction cap-app-update.md records in
`app.upgraded`, so export/install/upgrade all speak one hash.

## Command / event surface

| Surface | Name | Notes |
| --- | --- | --- |
| Host command | `terrane app export <id> [-o path]` | edge-only: read folded bundle kv, build archive, sign. Nothing to record — export changes no state |
| Command | `app.install` | args `archive_path` → `Effect::InstallSignedBundle`; edge verifies signature + bundleHash, then reuses the import path |
| Event (new) | `publish.installed` | `{ app, version, bundle_hash, publisher_pubkey, publisher_label }` — provenance, emitted in the **same batch** as the existing `app.added` + kv file events (extends `app.import`'s batch shape; `app.added` itself stays byte-compatible) |
| Event (new) | `publish.trusted` | `{ pubkey, label }` — first-use trust fact |
| Event (new) | `publish.identity-created` | `{ pubkey, replica_peer }` |

Fold: `PublishState { identity: Option<Pubkey>, trusted: BTreeMap<Pubkey,
Label>, provenance: BTreeMap<AppId, Provenance> }`. Reacts to `app.removed`
by dropping provenance (trust in the key survives — it is per publisher, not
per app). Installing over an existing id routes through `app.upgrade`
(cap-app-update.md) with the same signature checks.

## Replay story

Verification is an edge act at install time; the *outcome* — provenance and
trust — is events. Replay folds `publish.*` + `app.added` + kv events and
rebuilds the catalog with identical provenance without re-verifying any
signature or touching any archive. A replayed home trusts exactly what it was
told to trust, in order.

## Security — what a malicious bundle can and cannot do

**The permission system is the sandbox.** A bundle is data until granted;
installing one grants nothing:

- Backend JS runs in the QuickJS sandbox (`terrane-cap-js-runtime`): no
  filesystem, no ambient network, no `eval`/`Function`, 64 MiB memory cap,
  ~5 s wall budget. Its entire reach is `ctx.resource.*`, and every namespace
  in `manifest.resources` still goes through auth grants with the existing
  in-session prompts — a hostile todo app asking for `net` looks exactly as
  suspicious as it is.
- What it *can* do: waste its own budget, render a deceptive UI in its own
  iframe (shell chrome and other apps' frames are origin/nonce-isolated), and
  — **if the user grants `net`** — exfiltrate whatever data the user also
  granted it. The prompt copy must therefore name grants concretely; that is
  the real security boundary and this doc says so plainly.
- What it *cannot* do: touch another app's kv/blobs (app-scoped state), call
  ungranted capabilities, escalate via install (installing records facts, it
  executes nothing), or forge provenance (signature covers the exact bytes).
- Validation before any event is emitted: signature, bundleHash, id-safety,
  symlink/text rules, runtime whitelist — a tampered archive dies at the edge
  with a typed error and an untouched log.

## Limits

- Archive ≤ 16 MiB (text bundles; blob-carrying archives are v2 with
  [cap-blob.md](cap-blob.md) sidecar entries), ≤ 512 files.
- `formatVersion` unknown ⇒ typed error naming the newest supported version.
- One publisher identity per home in v1.

## Implementation plan

1. **Crate `terrane-cap-publish`:** events + fold + describe + `doc.rs`;
   `installed_event()` / `trusted_event()` / `identity_created_event()`
   constructors; pure archive-metadata validation helpers.
2. **Interface:** `Effect::InstallSignedBundle { source }`.
3. **Edge:** archive build/sign (`terrane-host/src/publish.rs`, `ed25519-dalek`
   + keychain store from cap-oauth-connections.md); `InstallSignedBundle` arm:
   verify, TOFU check (prompt via the existing elicitation path on key
   change), then delegate to the `import_app_bundle` internals and append the
   `publish.installed` batch.
4. **CLI:** `terrane app export` / `terrane app install`; MCP `app_install`
   for agent-driven installs (subject to the same prompts).
5. **Register** in `default_registry`; `APP_API.md` note (apps have no publish
   surface — deliberately).
6. **Tests:** engine tests `terrane-core/tests/cap/publish.rs` (fold, TOFU
   state, replay identity, app.removed); e2e `terrane-host/tests/cap/publish.rs`
   (export→install round-trip between two temp homes, tampered-byte rejection,
   key-change prompt path, provenance in state) — default-run, no network.

Gate after each step:
`cargo test --workspace --locked && cargo clippy --workspace --all-targets --locked -- -D warnings`.

## Non-goals (v1)

Auto-update checks and feeds (ties to [cap-app-update.md](cap-app-update.md)),
key rotation/revocation, multi-signer archives, encrypted archives, a central
registry, and publishing *data* (bundles only — data travels via sync).

## Decisions to confirm

- **Premium catalog as a publish target** — *recommendation:* defer; v1
  distribution is file / AirDrop / URL (`terrane app install <path|url>` with
  the URL fetched at the edge, then verified identically). The Premium
  web/mac catalog becomes a listing of signed archives in a later pass once
  the format is proven. *Alternative:* ship catalog upload now — rejected: it
  drags server auth and moderation into a format-definition milestone.
- **Key custody** — *recommendation:* host keychain via the
  cap-oauth-connections.md store. *Alternative:* a passphrase-sealed file
  under `$TERRANE_HOME` using `terrane-cap-crypto` primitives — acceptable
  fallback on platforms without a keychain; keep it as the portable path.
- **Trust scope** — *recommendation:* per publisher key (one approval covers
  all their apps). *Alternative:* per (key, app id) pair — stricter, noisier;
  revisit if TOFU proves too coarse.
