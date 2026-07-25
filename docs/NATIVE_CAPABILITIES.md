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

The output contains 42 independently linked, signed `<namespace>-<version>.tcap`
archives, a signed `index.json`, and `verifying-key.hex`. Each archive contains
only that namespace's native worker executable and manifest. Release tooling
copies the package directory to
`TerraneHost.app/Contents/Resources/capabilities`. The host also accepts
`TERRANE_CAP_BUNDLE_DIR` for development and tests.

Set `TERRANE_CAP_INDEX_BASE_URL` while packaging to sign an exact-artifact
repair origin into the index. Repair first validates the packaged `.tcap`, then
downloads that same archive and accepts it only when its SHA-256 matches the
home's exact lock. It never resolves or upgrades to a newer version.

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
- `capabilities/state/<namespace>.json`: the replay-minimal worker event slice,
  cursor, payload hash, and canonical owned/subscribed-event projection hash,
  written atomically before eviction. This reconstructs both private state and
  the fundamental shadow slices even for legacy capabilities that did not have
  typed snapshot hooks. Activation restores it and folds only the log tail;
  invalid and legacy snapshots fall back to full replay.
- `capabilities/downloads/`: exact-hash repair artifacts fetched from the
  signed index only when the packaged artifact is missing or corrupt.

At most eight idle workers remain warm for ten minutes. Persisted automation,
job, scheduler, stream, and webhook state reactivates the corresponding workers
without an app window. Empty homes still start with zero dynamic workers.

On the first open of an existing home, Terrane keeps the event log unchanged,
replays every pinned dynamic worker, compares its canonical projection with the
resident one-release compatibility implementation, writes durable worker
snapshots, and only then atomically writes `capabilities/migration-v1.json`.
An interrupted or failed migration has no marker and is retried on the next
open. Unless dynamic mode is required, failure detaches the manager and leaves
the fundamental control plane running on the emergency static fallback.

## Operations

```sh
terrane cap status
terrane cap prewarm time net
terrane cap evict time
terrane cap evict --all
terrane cap repair time
terrane cap verify
```

Repair revalidates the exact locked artifact and never changes its version;
offline failure disables only that capability. Worker responses have bounded
deadlines, and a crashed or timed-out worker is restored and retried once.
`cap verify` replays every pinned worker, compares its cursor and canonical
owned/subscribed-event projection hash with the resident compatibility
implementation, and then evicts it.

Set `TERRANE_CAP_STATIC_FALLBACK=1` for the one-release emergency fallback.
Set `TERRANE_CAP_REQUIRE_DYNAMIC=1` in packaged builds to make missing bundles
or keys a startup error instead of silently using that fallback.

Native workers are trusted first-party code. The process boundary contains
crashes and avoids Rust ABI coupling; it is not a sandbox for third-party
binaries. Worker-owned headless effects run in the signed process (currently
wall-clock observation and webhook token rotation). UI, main-thread, secret
storage, media, model, and platform effects use the versioned parent-host
connector; their returned events still pass signed-manifest validation before
commit. Live, non-recorded observations such as `sysinfo` also cross an
explicit live-sample connector and remain outside the event log.
