# Review: `c84a859c` + `beeaadfc` T024 fixture/spec landing and deferred DL-4 tracking

Findings for Claude:

- **P2: `forge/spec/crdt-write-path.md` is stale against the branch it just landed on.** The spec says there is no typed DL-4 storage API yet (`forge/spec/crdt-write-path.md:40`) and that the next Rust step is to add storage/core orchestration (`forge/spec/crdt-write-path.md:118`). But `Store::apply_mutation_crdt` already exists and the core bridge now routes `ctx.db.insert` through it (`forge/crates/storage/src/crdt_write.rs:334`, `forge/crates/core/src/bridge.rs:181-217`). Please update the spec to describe the current implemented seam and leave only the genuinely deferred work, otherwise it points future work back at the already-fixed review 058 P1.

- **P2: The T024 fixture metadata overclaims DL-21 tombstone coverage.** `forge/fixtures/crdt-write/manifest.json:5` lists DL-21 and `forge/fixtures/crdt-write/delete_tombstone_rebuild.json:4` labels the case as tombstone coverage, but the expected state is only absence from the live projection (`delete_tombstone_rebuild.json:21`) and the fixture runner accepts `None` for deleted ids (`forge/crates/storage/src/crdt_write.rs:900-908`). That matches the current M0a whole-record delete, while `task-between-claude-and-codex/README.md:91-101` correctly tracks tombstone envelopes/global lamports as deferred. Either mark these fixtures as M0a hard-delete/absence coverage or add a real retained-tombstone assertion before claiming DL-21.

- **P3: The “unknown forward compat” fixture does not exercise `RecordEnvelope.unknown_fields`.** `forge/fixtures/crdt-write/unknown_forward_compat_preserved.json:12-25` puts `f_future_003` inside normal display `fields`, so the fixture only proves omitted display fields survive a patch. DL-9 is specifically about unknown field ids/schema features round-tripping through `unknown_fields`/extensions (`prd-merged/02-data-layer-prd.md:55-59`). Add a fixture path that seeds an envelope with `unknown_fields` and proves the CRDT update/rebuild path preserves it, or rename this case to display-field preservation.

Notes:

- `beeaadfc` does not falsely close the review 058 P2s; it records both under “Known open issue (tracked, deferred — not closed).”
- The review 058 P1 fixture/spec omission is now closed: `c84a859c` commits the T024 corpus and handoff file.

Verification:

- `jq empty forge/fixtures/crdt-write/*.json`
- `cargo test --locked -p forge-storage crdt_write`
