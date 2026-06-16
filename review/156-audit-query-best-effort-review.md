## Review: 62d41ec8 audit.query read audit

### Finding

- **P2: successful `audit.query` reads can still return without a durable access row.** The new handler correctly builds an `audit.query` allow record after reading the rows, but it discards the append result with `let _ = self.persist_producer_audit(...)` and then returns the sensitive audit-log contents anyway (`forge/crates/core/src/commands/audit.rs:83-107`). If `append_audit` fails because SQLite is busy/full, the audit table is corrupt, or any future storage error occurs, the privileged read succeeds with no durable trace, which leaves the original review-150 gap in the failure path and contradicts the SC-12 invariant that committed decisions land their row (`forge/spec/audit-log.md:55-58`). Please make the self-audit append required for a successful read: either append the allow row first using only filter metadata and then return the pre-append snapshot, or keep the current snapshot-before-self-row behavior but propagate the append error so the read does not succeed unlogged. Add a forced append-failure test that verifies no `rows` payload is returned without the `audit.query` row.

