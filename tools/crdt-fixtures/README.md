# CRDT Fixture Generator

Generates Loro-backed notebook CRDT conformance fixtures for the Forge sync and CRDT contracts.

Run from the repo root:

```sh
cargo run --manifest-path tools/crdt-fixtures/Cargo.toml
```

Check the checked-in fixtures without rewriting them:

```sh
cargo run --manifest-path tools/crdt-fixtures/Cargo.toml -- --check
```

The generator uses the pinned checkout at `external-lib/loro` as the CRDT oracle and writes JSON fixtures to `tests/fixtures/crdt`.
