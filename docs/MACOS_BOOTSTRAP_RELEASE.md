# macOS bootstrap releases

Terrane's public macOS download is a small Apple-Silicon bootstrap application.
The bootstrap downloads, verifies, installs, and opens the full Terrane runtime
on first launch. The same versioned runtime store is the update foundation for
later releases.

This is a machine-built release path. It does not use GitHub Actions.

## Trust model

There are two distinct signatures:

1. macOS ad-hoc code signatures protect bundle integrity after packaging.
2. Terrane's Ed25519 release signature authenticates the downloadable manifest.

The Ed25519 public key is embedded in
`host/macos/BootstrapSources/BootstrapConfiguration.swift`. The private key must
never be committed. The local default private-key location used for release
work is `.terrane-release/update-signing-key.pem`, which is gitignored and must
be backed up securely outside the repository.

Without an Apple Developer account, the applications cannot be Developer-ID
signed or notarized. Users may need to use macOS **Open** on the first bootstrap
launch. Terrane's own signature verification does not replace Apple's
notarization trust.

The release manifest signs these exact values:

- manifest format
- runtime version
- `arm64` architecture
- immutable artifact URL
- artifact SHA-256
- artifact byte size
- expected root application bundle name

Production artifacts must use HTTPS. Plain HTTP is accepted only for an
explicit loopback test configuration.

## Runtime installation

The bootstrap stores runtime data under:

```text
~/Library/Application Support/Terrane/
├── downloads/<sha256>.zip
├── downloads/<sha256>.parts/part-*
├── versions/<version>/Terrane.app
└── runtime-state.json
```

When the release server advertises byte-range support and the exact expected
length, the bootstrap downloads with up to eight connections. Every response
must return the requested `Content-Range`; otherwise it safely falls back to one
stream. Partial ranges remain resumable across bounded automatic retries.

The native progress window displays downloaded and total bytes, current
five-second transfer speed, estimated time remaining, active connection count,
and download elapsed time. Verification and installation have separate elapsed
timers so a slow transfer is distinguishable from local processing.

If all connections stop making progress for 12 seconds, the bootstrap cancels
the stalled requests and resumes their partial ranges. It retries a maximum of
three times before showing the manual **Try Again** action.

Installation verifies the manifest signature, exact byte count, SHA-256, safe
archive layout, and the app's recursive code signature. Extraction happens in a
staging directory. Activation writes a versioned state file atomically.

The runtime receives a one-use health-marker path. The bootstrap switches to the
new version only after installation and waits for the runtime window to report
healthy startup. If startup fails and a previous version is available, it marks
the failed version, restores the previous version, and does not retry the same
bad release on every launch.

When the release service is unavailable, an already-installed healthy runtime
still opens.

## Build on the release Mac

Requirements:

- Apple Silicon Mac
- Xcode and XcodeGen
- Node.js
- OpenSSL for generating the separate Ed25519 key
- librsvg (`rsvg-convert`) when regenerating the canonical macOS icon
- the project Cargo/sccache setup

One-time signing-key creation:

```sh
mkdir -p .terrane-release
chmod 700 .terrane-release
openssl genpkey \
  -algorithm ED25519 \
  -out .terrane-release/update-signing-key.pem
chmod 600 .terrane-release/update-signing-key.pem
```

Back up that key securely. Confirm its raw public key matches the value embedded
in the bootstrap:

```sh
openssl pkey \
  -in .terrane-release/update-signing-key.pem \
  -pubout -outform DER \
  | tail -c 32 \
  | xxd -p -c 64
```

Build a release:

```sh
export TERRANE_UPDATE_SIGNING_KEY="$PWD/.terrane-release/update-signing-key.pem"
export TERRANE_UPDATE_BASE_URL="https://github.com/sunrisecloudy/terrane/releases/download/v0.2.0-preview.5"
scripts/build-macos-bootstrap-release.sh \
  0.2.0-preview.5 \
  artifacts/macos-bootstrap/0.2.0-preview.5
```

The build pins every Rust and C/C++ object to macOS 13, uses the shared Cargo
cache, builds arm64-only applications, verifies both application signatures and
Mach-O deployment targets, verifies both bundles contain the canonical
organic-T AppIcon through 1024px, creates the runtime ZIP and bootstrap DMG,
then checks all release hashes.

Upload these assets to the matching GitHub release:

- `Terrane-Bootstrap-arm64.dmg`
- `TerraneRuntime-arm64.zip`
- `terrane-bootstrap-manifest.json`
- `SHA256SUMS`

The bootstrap's stable manifest URL is:

```text
https://github.com/sunrisecloudy/terrane/releases/latest/download/terrane-bootstrap-manifest.json
```

Because the stable URL uses GitHub's `latest` redirect, publish the bootstrap
release with **Latest** enabled. Do not mark it as a GitHub prerelease: GitHub's
`latest` endpoint excludes prereleases. A preview label in the tag or title is
fine.

The manifest's runtime URL is immutable and includes the exact release tag.

Example publication command:

```sh
gh release create v0.2.0-preview.5 \
  artifacts/macos-bootstrap/0.2.0-preview.5/Terrane-Bootstrap-arm64.dmg \
  artifacts/macos-bootstrap/0.2.0-preview.5/TerraneRuntime-arm64.zip \
  artifacts/macos-bootstrap/0.2.0-preview.5/terrane-bootstrap-manifest.json \
  artifacts/macos-bootstrap/0.2.0-preview.5/SHA256SUMS \
  --target <release-commit-sha> \
  --title "Terrane 0.2.0 Preview 5 (Canonical Icon, Unsigned)" \
  --notes-file <release-notes.md> \
  --latest
```

## Full-loop local test

Create the runtime package with a loopback URL, then serve it slowly:

```sh
scripts/package-bootstrap-runtime.mjs \
  --app /path/to/Terrane.app \
  --output /tmp/terrane-bootstrap-fixture \
  --version 0.2.0-preview.5 \
  --base-url http://127.0.0.1:8765 \
  --signing-key .terrane-release/update-signing-key.pem

scripts/bootstrap-test-server.mjs \
  --root /tmp/terrane-bootstrap-fixture \
  --port 8765 \
  --bytes-per-second 131072 \
  --stall-after-bytes 262144 \
  --stall-seconds 15 \
  --stall-count 8
```

Launch the bootstrap with an isolated store:

```sh
open -n \
  --env TERRANE_BOOTSTRAP_MANIFEST_URL=http://127.0.0.1:8765/terrane-bootstrap-manifest.json \
  --env TERRANE_BOOTSTRAP_ALLOW_INSECURE_LOCALHOST=1 \
  --env TERRANE_BOOTSTRAP_HOME=/tmp/terrane-bootstrap-home \
  --env TERRANE_BOOTSTRAP_RUNTIME_HOME=/tmp/terrane-runtime-home \
  --env TERRANE_BOOTSTRAP_CONNECTIONS=8 \
  /path/to/bootstrap/Terrane.app
```

For a faster automated stall test, set
`TERRANE_BOOTSTRAP_STALL_TIMEOUT=2`. Production defaults to 12 seconds. The
connection count is bounded to 1–8 and automatic retries default to three.

Acceptance requires:

- visible determinate progress, exact bytes, current speed, ETA, connection
  count, and elapsed download time
- eight non-overlapping range requests with complete byte coverage
- automatic partial-range resume after a forced all-connection stall
- separate visible verification and installation timers
- signed-manifest rejection after any field is changed
- hash or size mismatch rejection before extraction
- successful verified extraction and code-signature verification
- runtime health acknowledgement
- fallback to the installed runtime when the manifest service is offline
- rollback when a newly installed runtime does not acknowledge health
- second launch does not download the same runtime again

## Publishing checklist

- Working tree and intended commit are recorded.
- Bootstrap and runtime are arm64-only.
- Bootstrap, DMG app, Dock entry, and runtime expose the same canonical AppIcon.
- Both report `minos 13.0`.
- No linker warnings report objects built for a newer macOS version.
- Bootstrap unit tests pass.
- The throttled eight-connection first-run loop is visually checked.
- Forced stall detection and automatic partial-range resume are exercised.
- Failure, retry, successful activation, and second-launch paths are exercised.
- DMG and runtime sizes are recorded in the release notes.
- `shasum -a 256 -c SHA256SUMS` passes.
- GitHub assets exactly match the signed manifest.
