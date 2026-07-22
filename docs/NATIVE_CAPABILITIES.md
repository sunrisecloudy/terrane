# Native lazy capabilities

Terrane keeps `app`, `auth`, `kv`, `connection`, `blob`, `person`, `replica`,
and `telemetry` resident. The remaining 41 namespaces are signed native Rust
workers speaking the versioned, length-prefixed JSON protocol in
`terrane-cap-protocol`. No Rust ABI crosses the process boundary and existing
Rust capability implementations do not need a WASI rewrite.

## Package

Set a release Ed25519 signing seed and run:

```sh
TERRANE_CAP_SIGNING_KEY_HEX=<64-hex-characters> \
  scripts/package-native-capabilities.sh target/native-capabilities
```

The output contains one signed bundle directory per namespace and
`verifying-key.hex`. Release tooling copies that directory to
`TerraneHost.app/Contents/Resources/capabilities`. The host also accepts
`TERRANE_CAP_BUNDLE_DIR` for development and tests.

Never commit a production signing seed. The public verifying key is safe to
ship beside the signed manifests.

## Activation and storage

An empty home starts no workers. App invocation and web UI opening call
`prepare_app`, which reads the runtime and normalized capability requirements
persisted in `AppRecord`, expands transitive dependencies, and loads them
concurrently. Direct commands and queries load their namespace on demand.
Simultaneous loads coalesce on the namespace slot.

Each home stores:

- `capabilities.lock.json`: exact versions, platforms, architectures, bundle
  digests, and executable hashes.
- `capabilities/cache/<namespace>/<version>/<hash>/`: verified extracted files.
- `capabilities/state/<namespace>.json`: snapshot, cursor, and canonical hash
  written atomically before eviction.

At most eight idle workers remain warm for ten minutes. Background manifests
are kept alive until a trusted operator clears the hint or evicts all workers.

## Operations

```sh
terrane cap status
terrane cap prewarm time net
terrane cap evict time
terrane cap evict --all
terrane cap repair time
terrane cap verify
```

Repair revalidates the exact locked artifact and never changes its version.
`cap verify` replays every pinned worker, compares its cursor and canonical
state hash with the resident compatibility implementation, and then evicts it.

Set `TERRANE_CAP_STATIC_FALLBACK=1` for the one-release emergency fallback.
Set `TERRANE_CAP_REQUIRE_DYNAMIC=1` in packaged builds to make missing bundles
or keys a startup error instead of silently using that fallback.

Native workers are trusted first-party code. The process boundary contains
crashes and avoids Rust ABI coupling; it is not a sandbox for third-party
binaries. UI and main-thread effects remain parent-host connector operations.
