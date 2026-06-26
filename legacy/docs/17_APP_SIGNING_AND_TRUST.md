# App Signing and Trust Model

## 1. Purpose

Generated webapps are untrusted source packages until the platform validates and installs them. Signing does not make arbitrary generated code safe; it proves that the exact package the user approved is the exact package the runtime is executing.

The required lifecycle is:

```text
source package
  -> validate
  -> policy audit
  -> permission approval
  -> canonicalize
  -> sign
  -> install immutable version (single DB transaction)
  -> run smoke/micro tests
  -> enable
```

## 2. Source package vs installed package

A **source package** is what AI generates:

```text
manifest.json
index.html
styles.css
app.js
smoke-tests.json   optional
migrations/        optional
```

An **installed package** is what the platform stores after validation:

```text
manifest.json
files...
signature.json          platform-generated
install-report.json     platform-generated
content-hashes.json     platform-generated
```

Bundled example apps in the repository are source packages. Release packaging or dev installation signs them before runtime execution.

AI must not generate `signature.json`, `install-report.json`, or `content-hashes.json` — the platform produces those after canonicalization.

## 3. Trust levels

| Trust level | Meaning | Allowed by default | Distribution |
|---|---|---|---|
| `bundled` | Shipped with the reviewed app binary/package | yes | App Store, store distribution, sideload |
| `user-generated` | Generated locally by the user's AI session | yes after approval | sideload / TestFlight / Developer-ID / Android sideload / desktop direct install |
| `developer` | Installed through control-plane / Codex dev mode | dev builds only | dev builds |
| `remote` | Downloaded from a remote source | disabled | none (reserved for future revisions) |
| `quarantined` | Installed but failed audit/test/runtime checks | no | none |

App Store distribution accepts only `bundled` packages (docs/00 D1).

## 4. Signature scope

The signature must cover:

- canonical manifest JSON;
- each package file path and its SHA-256 hash;
- migration files and their hashes;
- permission list;
- resource budget;
- network policy;
- required runtime version;
- data version;
- app id and app version.

Do not sign mutable runtime state such as storage data, logs, or install reports.

## 5. Signature format

Use `schemas/app-signature.schema.json`.

```json
{
  "appId": "notes-lite",
  "appVersion": "0.1.0",
  "dataVersion": 1,
  "runtimeVersion": "0.1.0",
  "trustLevel": "user-generated",
  "algorithm": "ed25519",
  "keyId": "platform-host:<bundle-id>:<machine-id-hash>",
  "manifestHash": "sha256:...",
  "contentHash": "sha256:...",
  "permissionsHash": "sha256:...",
  "policyHash": "sha256:...",
  "signedAt": "2026-05-28T00:00:00Z",
  "signedBy": "local-platform",
  "signature": "base64..."
}
```

### 5.1 Algorithm

- `algorithm = "ed25519"` is the only production algorithm. The signature is over the canonical byte stream defined in §5.2 and verifies against the platform host's Ed25519 public key.
- `algorithm = "none-dev"` is permitted only on the reference host or when the runtime is explicitly started with `--dev` and connected to the dev control plane. Production builds reject `none-dev` with `signature_untrusted`.

### 5.2 Signed payload (the bytes that go into Ed25519.sign)

```
"terrane/sig/v1\n"
appId "\n"
appVersion "\n"
dataVersion "\n"
runtimeVersion "\n"
trustLevel "\n"
keyId "\n"
manifestHash "\n"
contentHash "\n"
permissionsHash "\n"
policyHash "\n"
signedAt "\n"
```

The string is UTF-8 encoded, LF newlines only, no trailing newline after the last field. This payload is deterministic given the package and canonicalization rules in §6.

### 5.3 Hash inputs

| Hash | Computed over |
|---|---|
| `manifestHash` | canonical JSON bytes of `manifest.json` |
| `contentHash` | SHA-256 of the concatenation, in sorted-path order, of (path NUL SHA-256(file_bytes) "\n") for every file in the package |
| `permissionsHash` | canonical JSON bytes of `manifest.permissions` (sorted ascending) |
| `policyHash` | canonical JSON bytes of `{ "resourceBudget", "networkPolicy", "capabilities" }` from the manifest |

## 6. Canonicalization

Implement deterministic canonicalization before hashing:

1. Sort object keys in JSON ascending by Unicode code point.
2. Sort file records by logical package path ascending.
3. For each file record, hash the verbatim packaged file bytes to a
   `sha256:`-prefixed lowercase digest.
4. Build the `contentHash` input as `path`, a single NUL byte, the recomputed
   per-file digest, and LF for each sorted file record.
5. Hash exact bytes after this framing; the signing verifier does not rewrite
   line endings or strip BOMs.

Package validation remains responsible for path safety before install: paths
must be forward-slash relative package paths and must not contain traversal
segments or absolute roots. Those checks are separate from the signed
`contentHash` byte framing so independent signers can reproduce the same
preimage exactly.

Canonical JSON serialization uses:

- no insignificant whitespace,
- key order sorted ascending,
- numbers serialized as the shortest round-trip representation,
- no trailing newline.

## 7. Key management

### 7.1 Per-host platform keypair

Each native host generates a platform keypair on first launch:

- Algorithm: Ed25519.
- Stored in the platform secure store where available; otherwise in `PlatformDatabase` with `mode 0600` access:
  - iOS / macOS: Keychain (`kSecAttrAccessibleWhenUnlocked`).
  - Android: Android Keystore (`KeyProperties.PURPOSE_SIGN`).
  - Windows: DPAPI-encrypted in `%LOCALAPPDATA%\<product>\platform.key`.
  - Linux: libsecret if available; otherwise `$XDG_DATA_HOME/<product>/platform.key` with mode 0600.
  - Reference host: `~/.cache/terrane/platform.key` (test-only).
- The public key is exposed via the control plane for cross-host verification of exported bundles.

### 7.2 Key id

`keyId = "platform-host:<bundle-id>:<sha256(public-key)[0:16]>"`. The id is logged in `app_install_reports`.

### 7.3 Key rotation

- The platform keypair rotates if compromised, on explicit user action, or when the host detects a stored-key integrity error.
- After rotation, previously installed apps remain mounted; the runtime re-verifies on next mount and re-signs with the new key. This re-sign produces a new `app_installations` row of type `re-sign`.
- A second key may be retained for 90 days as a "trust grace" key so backup exports signed by the old key can still be imported.

### 7.4 Bundled-app signing

Bundled apps are signed during release packaging using a developer-controlled offline key. Their `trustLevel = "bundled"`. The bundled key id is also pinned in the host binary so the runtime can verify bundled apps even before the per-host platform keypair exists.

## 8. Runtime checks at mount

Before mounting an app, the runtime must verify, in order:

1. Signature exists for the installed package.
2. `algorithm` is allowed in the current build mode (`none-dev` only in dev).
3. `keyId` resolves to a known public key (platform host, bundled, or imported grace key).
4. Ed25519 signature verifies against the signed payload (§5.2).
5. `manifestHash` matches the stored canonical manifest.
6. `contentHash` matches a recomputed hash over `app_files`.
7. `permissionsHash` matches the stored permissions for the active install.
8. `policyHash` matches the stored budget/policy snapshot.
9. `runtimeVersion` is compatible per docs/04 §8.
10. Package is not `quarantined`.
11. User consent still covers declared permissions.

Any mismatch returns a structured error (`signature_invalid`, `signature_untrusted`, `manifest_tampered`, `content_tampered`, `permission_tampered`, `policy_tampered`, `runtime_version_incompatible`, `package_quarantined`, `consent_revoked`) and refuses mount.

## 9. Permission changes require reapproval

Any app update that changes permissions, network policy, resource budget, capabilities, or storage migration plan must produce an install report with `requiresUserApproval = true`. The runtime keeps the previous active version active until approval is granted.

## 10. Dev control-plane constraints

Codex control tools may install unsigned source packages into the reference host only if the install command explicitly sets:

```json
{ "devUnsigned": true, "target": "reference-host" }
```

All real native targets must run the normal validation/signing path, even in development. Dev native builds may use the local platform key but must not accept `algorithm = "none-dev"` unless the host is started with `--allow-unsigned-dev`, which itself is a compile-out flag in App Store builds.

## 11. Backup interaction

When a backup bundle (docs/29) is imported:

1. The bundle declares the platform key id and exports the public key.
2. The receiving host verifies app signatures using the bundle's public key.
3. Imported apps are re-signed with the receiving host's current key before activation.
4. The original bundle signature is retained in `app_install_reports` for audit.
