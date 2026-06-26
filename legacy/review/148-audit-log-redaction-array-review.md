## Review: 87464fad audit log substrate

Found one redaction bug that should be fixed before wiring live producers to this API.

- **P1 - Metadata arrays bypass audit redaction.** `redact_metadata` only descends into JSON objects; any non-object value is returned unchanged (`forge/crates/storage/src/audit.rs:448`). Because arrays are non-objects here, metadata like `{"attempts":[{"secret_value":"Bearer abc123"}]}` or `{"responses":[{"response_body":{"id":"lead-1"}}]}` will persist the secret/body inside `audit_log`, despite the new spec saying redaction is applied on every append and is the single chokepoint that prevents producers from persisting sensitive material (`forge/spec/audit-log.md:85`, `forge/spec/audit-log.md:101`; `prd-merged/07-security-prd.md:38`). Please make redaction recurse through `Value::Array` elements and add tests for array-contained `secret_value`, `request_body`, and `response_body`.
