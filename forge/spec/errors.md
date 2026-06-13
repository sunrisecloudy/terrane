# Core Error Catalog

Source of record: forge/crates/domain/src/lib.rs CoreError and prd-merged/01 CR-A4. The .code() token currently equals the variant name.

| Variant | .code() token | Raised when | Example trigger | PRD |
|---|---|---|---|---|
| ValidationError | ValidationError | Malformed command payload, parse failure, invalid input shape | TypeScript parser cannot parse an applet module | CR-A4, CR-14 |
| PermissionDenied | PermissionDenied | Actor role fails RBAC for the command | Viewer attempts record.put | CR-A3, CR-A4 |
| CapabilityRequired | CapabilityRequired | Applet lacks a required host capability | Applet calls ctx.db.write without db.write grant | CR-A4, SC-8 |
| StorageError | StorageError | SQLite or substrate serialization failure | Duplicate immutable CRDT chunk with different bytes | CR-A4, DL-4 |
| SchemaCompatibilityError | SchemaCompatibilityError | Schema change is not additive or references unknown schema state | Widen FloatNum to IntNum | CR-A4, DL-8 |
| QueryError | QueryError | Query DSL compile/execute failure | Filter references an unsupported operator | CR-A4, DL-15 |
| RuntimeError | RuntimeError | QuickJS or applet execution failure not caused by a resource limit | Entrypoint throws an exception | CR-A4, CR-8 |
| ResourceLimitExceeded | ResourceLimitExceeded | Wall, memory, fuel, host-call, storage, log, output, or network budget exceeded | Run exceeds max_host_calls | CR-A4, CR-5 |
| SyncError | SyncError | Peer, CRDT, or replication protocol failure | Sync frontier cannot be decoded | CR-A4, SS-4 |
| ConflictRequiresUser | ConflictRequiresUser | A merge conflict cannot be resolved automatically | Two actors edit a schema constraint incompatibly | CR-A4, DL-11 |
| ProviderError | ProviderError | External model/provider failed behind an allowed abstraction | LLM provider returns a policy-scanned failure | CR-A4, LM-9 |
| PlatformUnavailable | PlatformUnavailable | Host namespace is not available on the current target | Schedule API on a shell that has no scheduler | CR-A4 |

## Notes

- FFI/shell boundaries should return CoreResponse.error instead of panicking.
- Tests may unwrap; production command/runtime/storage paths should map failures into one of these variants.
- If a future subsystem wants a new variant, add it in forge-domain first and update this table before wiring shell contracts.
