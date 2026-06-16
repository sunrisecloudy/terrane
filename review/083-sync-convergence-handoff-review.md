# Review 083 - commits c8a04843, 176fc67e, 96dafb7f, d5ff0fc7

## Finding

- [P1] Bind the signed package identity to the installed applet id. Commit
  `176fc67e` correctly compares signed capabilities, network grants, and
  resource limits, but `verify_install_signature` still receives only
  `(cmd, manifest, sources)` and `bind_signature_to_manifest` compares only the
  policy surface. The signed fixture's manifest says `appId: "app.notes"`, while
  the positive test installs and records it as `Signed` under `"app_signed"`
  (`forge/crates/core/tests/spine.rs:2124`). That means a valid signature for one
  app identity can be attached to a different local applet id, leaving package
  provenance and replacement/upgrade identity unbound even though
  `appId` is part of the signed preimage. Pass the requested `applet_id` into the
  signature bind and reject when `package.manifest.appId` differs from it, or
  introduce an explicit separate local alias/package-id model and test that trust
  reporting and upgrades use the signed identity.

- [P1] Bind the full enforced limit surface, not only two budget fields. The
  binder checks signed `resourceBudget.wall_ms` and `memory_bytes`, but the
  runtime enforces `fuel`, `max_host_calls`, `storage_bytes`, and `log_bytes`
  from the stored top-level manifest too (`forge/crates/domain/src/manifest.rs:189`,
  `forge/crates/runtime/src/runner.rs:50`,
  `forge/crates/runtime/src/host.rs:196`,
  `forge/crates/runtime/src/host.rs:520`). A signed install can therefore widen
  host-call/storage/log/fuel budgets while still reporting `Signed`. Either add
  those enforced fields to the signed package policy hash and compare them, or
  reject signed installs whose top-level values differ from the runtime defaults
  when the signed manifest omits them.

- [P1] Preserve network policy constraints in the signature bind. `signed_net_set`
  reduces each signed allow rule to `(method, url)`, but `NetRule`/`NetPolicy`
  also enforce `max_response_bytes`, `max_body_bytes`, `timeout_ms`, request and
  response content types, and `allow_secret_headers`
  (`forge/crates/domain/src/manifest.rs:147`,
  `forge/crates/policy/src/net.rs:364`,
  `forge/crates/policy/src/net.rs:392`). A top-level manifest with the same
  method/url but looser caps or newly allowed secret headers passes the bind
  today. Compare the whole normalized net rule, not just routing fields.

- [P2] Bind entrypoint before treating multi-file packages as signed. PRD 08 says
  the package manifest includes `manifest{entrypoint|ui entry, capabilities}`,
  but the binder explicitly cannot derive/compare entrypoint
  (`forge/crates/core/src/workspace.rs:2060`), and the install path chooses the
  runnable source from the top-level manifest (`forge/crates/core/src/workspace.rs:466`).
  For a signed multi-file package, a caller can pick a different signed file as
  the runnable entrypoint. Add an entrypoint field to the signed fixture/package
  manifest and compare it, or reject signed multi-file installs until this is
  representable.

Key code references:

- `forge/crates/core/src/workspace.rs:1923` - `verify_install_signature` does not
  take the requested applet id.
- `forge/crates/core/src/workspace.rs:2076` - `bind_signature_to_manifest`
  compares policy/limits but not `package.manifest.appId`, all limits, or full
  net constraints.
- `forge/crates/signing/src/preimage.rs:184` - `appId` is part of the signed
  package preimage.

## T026 Handoff

The T026 request in `c8a04843` is aligned with `prd-merged/03-sync-server-prd.md`
SS-1/SS-2 and keeps SS-7 deferred to the next validation layer. I handled it in
`96dafb7f` and added a small local follow-up to make seeded fixtures unambiguous:

- `forge/spec/sync-protocol.md`
- `forge/fixtures/sync/manifest.json`
- 10 sync convergence vectors under `forge/fixtures/sync/`
- `"seed_mode": "shared_history"` on seeded fixtures so the future runner clones
  one baseline CRDT history instead of replaying seed ops independently per peer.

Implementation cautions already captured in the spec/result:

- Current local `chunk-NNNN` ids are not safe as exchanged peer frontiers because
  disconnected peers can collide on `(doc_id, chunk_id)`. Use peer-scoped or
  content-addressed exchanged chunk ids before persisting remote chunks.
- SS-7 authorization metadata must travel with opaque Loro chunks so remote ops
  can be rejected before `import_updates`.

`d5ff0fc7` adds `Store::list_doc_ids()` plus a focused storage test for distinct,
sorted doc ids. I did not find a blocking issue in that commit.
