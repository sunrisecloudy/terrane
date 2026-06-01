# zig-crdt

Standalone deterministic CRDT notebook profile for the Terrane.

The package intentionally owns only replayable CRDT logic. Hosts derive app, actor,
notebook, ACL, and session context before passing envelopes to the C ABI.

```sh
zig build test
```

Generated apps never link or import this package directly. They call the
platform-owned `notebook.*` bridge methods documented in
`docs/03_RUNTIME_API_SPEC.md`.
