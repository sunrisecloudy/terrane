# Releasing Terrane for M-series Macs

Terrane's supported end-user distribution is a Developer ID-signed and
Apple-notarized DMG attached to a GitHub release. The published binary is
`arm64` only and supports macOS 13 or newer. Intel Macs are not supported by
this release track.

## Release guarantees

Every published DMG must pass all of these gates:

- built from a clean annotated `vX.Y.Z` tag;
- Rust formatting, workspace tests, and strict clippy;
- macOS Xcode tests on an Apple-silicon GitHub runner;
- exactly 42 Ed25519-signed native capability archives;
- Developer ID signatures on every capability worker, the app, and the DMG;
- hardened runtime on the app and workers;
- Apple notarization and stapled tickets on both the app and DMG;
- strict `codesign`, Gatekeeper `spctl`, architecture, version, icon, and
  package-content verification;
- SHA-256 checksums, a machine-readable release manifest, and GitHub build
  provenance attestation.

An unsigned preflight artifact created by CI is never an end-user release.

## Required GitHub configuration

Create a protected GitHub environment named `production-release`. Require a
reviewer before secrets are exposed, and limit deployment tags to `v*`. The
workflow independently refuses any tag whose commit is not contained in
`origin/main`.

Configure these repository or environment secrets:

| Secret | Contents |
| --- | --- |
| `MACOS_CERTIFICATE_P12_BASE64` | Base64 of a Developer ID Application certificate and private key exported as PKCS#12 |
| `MACOS_CERTIFICATE_PASSWORD` | Password used when exporting that PKCS#12 |
| `APPLE_NOTARY_KEY_P8_BASE64` | Base64 of an App Store Connect API private key |
| `APPLE_NOTARY_KEY_ID` | App Store Connect API key ID |
| `APPLE_NOTARY_ISSUER_ID` | App Store Connect issuer ID |
| `TERRANE_CAP_SIGNING_KEY_HEX` | 64 lowercase hex characters for the production Ed25519 capability-signing seed |

Do not reuse the CI-only capability key from `.github/workflows/ci.yml`.
Do not commit certificates, private keys, API keys, or signing seeds.

## Local production build

The Mac must contain the matching Developer ID Application identity and a
notarytool profile:

```sh
xcrun notarytool store-credentials terrane-notary

export MACOS_SIGNING_IDENTITY="Developer ID Application: Example (TEAMID)"
export TERRANE_CAP_SIGNING_KEY_HEX="<64-hex-production-seed>"
export APPLE_NOTARY_KEYCHAIN_PROFILE="terrane-notary"

scripts/build-macos-release.sh \
  --version 0.2.0 \
  --build-number 1 \
  --output artifacts/macos \
  --notarize

scripts/verify-macos-release.sh \
  artifacts/macos/Terrane-0.2.0-macos-arm64.dmg \
  0.2.0
```

The local build is for diagnosis and final rehearsal. GitHub Actions is the
authoritative release builder.

## Publishing

1. Confirm `main` is clean, pushed, and green in CI.
2. Update `CHANGELOG.md`, the matching `docs/releases/vX.Y.Z.md`, application
   version, and any user-facing compatibility notes.
3. Create and push an annotated tag:

   ```sh
   git tag -a v0.2.0 -m "Terrane v0.2.0"
   git push origin v0.2.0
   ```

4. Approve the protected `production-release` environment.
5. Wait for `Release macOS` to complete.
6. Download the GitHub asset on a separate M-series Mac, compare
   `SHA256SUMS`, drag Terrane to Applications, launch it, create an app, quit,
   relaunch, and confirm its data persists.

The workflow creates the GitHub release only after signing, notarization, and
final artifact verification succeed.

## Failure and rollback

- Never upload an unsigned replacement under an existing release.
- If signing or notarization fails, fix the source or credentials and create a
  new build from the same unmodified tag. Do not move a published tag.
- If a published artifact is unsafe, mark the GitHub release as a draft or
  remove its assets, publish an incident note, and release a higher patch
  version. Revoke the notarization ticket or Developer ID certificate through
  Apple when compromise is suspected.
- Retain the release manifest, checksum, workflow URL, tag commit, and Apple
  notarization submission IDs for the release record.
