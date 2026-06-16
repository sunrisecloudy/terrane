# Review 091: signed net-cap closure + sync-RBAC decision

Reviewed commits:

- `48b0a43b forge-core: validate signed capabilities.net[] fields, fail closed (review 089 #1)`
- `0cbc8ec2 forge-core: SS-7 remote-op authorization decision + 10-vector harness`

## Findings

- **P2 - Treat `schema_write` claims as self-escalation before op-specific checks.** `authorize_remote_op` checks incoming role/db grants against the trusted membership, but explicitly skips a claim of `schema_write: true` when `trusted.schema_write` is false (`forge/crates/core/src/sync_rbac.rs:242`). That contradicts `forge/spec/sync-rbac.md:28`, which says incoming claims may narrow but must not widen trusted grants, and the decision order in `forge/spec/sync-rbac.md:50`. A trusted editor with `db.write=["tasks"]` can send a record insert claim containing `schema_write: true` and still be allowed because the schema claim is ignored once the op is not a schema change. Please reject `claim.schema_write && !trusted.schema_write` before role/grant checks, and add a vector where an otherwise-authorized record write is denied for that widened schema-maintenance claim.

- **P2 - Validate required envelope metadata before allowing wildcard grants.** The new `RemoteOpEnvelope` carries `collection`, `record_id`, `schema_id`, and `schema_version`, but `authorize_remote_op` never validates the required fields called out by `forge/spec/sync-rbac.md:52`. Record writes use `env.collection.as_deref().unwrap_or("")`, so an owner/editor with wildcard `db.write=["*"]` can allow a record insert with no collection/record id; schema changes similarly allow with `schema_id=None` when role + `schema_write` pass (`forge/crates/core/src/sync_rbac.rs:268`, `forge/crates/core/src/sync_rbac.rs:284`). That would leave the later apply path/audit without a concrete resource identity and misses SS-7's resource/schema-compatibility gate from `prd-merged/03-sync-server-prd.md:21`. Please fail closed on missing or inconsistent metadata before grant checks, including doc-id/resource-type consistency and schema version presence/compatibility, and add negative vectors for malformed record and schema envelopes.

## Notes

- The signed `capabilities.net[]` commit closes review 089's unknown-field gap with a focused fixture and no new issue found.
- No new handoff files appeared beyond the already-known T001-T028 set; `T023-ctx-db-query.md` remains an older `status: requested` handoff.
