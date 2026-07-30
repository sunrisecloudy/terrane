# Releasing Terrane for M-series Macs

Terrane's supported end-user distribution is a Developer ID-signed and
Apple-notarized DMG attached to a GitHub release. The published binary is
`arm64` only and supports macOS 13 or newer. Intel Macs are not supported by
this release track.

## Release guarantees

Every published DMG must pass all of these gates:

- built from a clean annotated `vX.Y.Z` tag;
- Rust formatting, workspace tests, and strict clippy on the release Mac;
- macOS Xcode tests on the release Mac;
- exactly 42 Ed25519-signed native capability archives;
- Developer ID signatures on every capability worker, the app, and the DMG;
- hardened runtime on the app and workers;
- Apple notarization and stapled tickets on both the app and DMG;
- strict `codesign`, Gatekeeper `spctl`, architecture, version, icon, and
  package-content verification;
- SHA-256 checksums and a machine-readable release manifest containing the
  exact source commit.

An unsigned local build is never an end-user release.

## Unsigned previews

When Developer ID credentials are unavailable, Terrane may publish a
GitHub **pre-release** using `scripts/publish-macos-preview.sh`. Its tag must
use `vX.Y.Z-preview.N`, and its notes must state that it is unsigned,
unnotarized, blocked by Gatekeeper by default, and not production-grade.

The preview publisher runs the same local Rust and Xcode tests, package-content
checks, checksum validation, GitHub upload, and post-download verification.
It uses a documented non-secret capability-index key and does not attempt
Apple code signing or notarization. The DMG contains the signed capability
index but not the 42 worker archives. Those archives are uploaded beside the
DMG and verified before and after publication; the app fetches exact pinned
archives on demand. A 40 MiB DMG budget is enforced.

```sh
git tag -a v0.2.0-preview.2 -m "Terrane v0.2.0 Preview 2"
scripts/publish-macos-preview.sh --version 0.2.0 --preview 2
```

## Release Mac configuration

The release Mac must have Xcode, Rust, XcodeGen, GitHub CLI, and `jq`. Sign in
to GitHub CLI with permission to push tags and create releases:

```sh
gh auth login
gh auth status
```

Configure these local credentials:

| Credential | Contents |
| --- | --- |
| Keychain signing identity | Developer ID Application certificate and private key |
| `terrane-notary` keychain profile | App Store Connect API key stored by `notarytool` |
| `TERRANE_CAP_SIGNING_KEY_HEX` | 64 lowercase hex characters for the production Ed25519 capability-signing seed |

Do not commit certificates, private keys, API keys, or signing seeds.

## Publishing from this Mac

The Mac must contain the matching Developer ID Application identity and a
notarytool profile:

```sh
xcrun notarytool store-credentials terrane-notary

export MACOS_SIGNING_IDENTITY="Developer ID Application: Example (TEAMID)"
export TERRANE_CAP_SIGNING_KEY_HEX="<64-hex-production-seed>"
export APPLE_NOTARY_KEYCHAIN_PROFILE="terrane-notary"

scripts/publish-macos-release.sh \
  --version 0.2.0 \
  --build-number 1 \
  --output artifacts/macos
```

The publisher runs every source and native test locally, builds and notarizes
the arm64-only DMG, verifies it, pushes the annotated tag if needed, creates
the GitHub release, downloads the published assets, checks the checksum, and
verifies the downloaded DMG again.

## Release sequence

1. Confirm `main` is clean and pushed.
2. Update `CHANGELOG.md`, the matching `docs/releases/vX.Y.Z.md`, application
   version, and any user-facing compatibility notes.
3. Create an annotated tag locally:

   ```sh
   git tag -a v0.2.0 -m "Terrane v0.2.0"
   ```

4. Run `scripts/publish-macos-release.sh` as shown above. The script pushes
   the tag only after all local gates pass.
5. Download the public asset in a clean user account, compare `SHA256SUMS`,
   drag Terrane to Applications, launch it, create an app, quit, relaunch, and
   confirm its data persists.

## Failure and rollback

- Never upload an unsigned replacement under an existing release.
- If signing or notarization fails, fix the source or credentials and create a
  new build from the same unmodified tag. Do not move a published tag.
- If a published artifact is unsafe, mark the GitHub release as a draft or
  remove its assets, publish an incident note, and release a higher patch
  version. Revoke the notarization ticket or Developer ID certificate through
  Apple when compromise is suspected.
- Retain the release manifest, checksum, tag commit, and Apple notarization
  submission IDs for the release record.
