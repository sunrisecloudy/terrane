# Signing Fixtures

These fixtures use a TEST-ONLY Ed25519 keypair in test-keypair.json. Never use that keypair for any package outside tests.

Canonical bytes are the docs/17 payload:

terrane/sig/v1\n
appId\n
appVersion\n
dataVersion\n
runtimeVersion\n
trustLevel\n
keyId\n
manifestHash\n
contentHash\n
permissionsHash\n
policyHash\n
signedAt

There is no trailing newline after signedAt. Hashes are lowercase sha256 hex with the sha256: prefix. manifestHash and permissionsHash use stable key-sorted JSON. contentHash is sha256 over sorted path entries: path, NUL, sha256(content), newline. policyHash signs the object {resourceBudget, networkPolicy, capabilities} using stable key-sorted JSON.

The vectors separate crypto failures from package-hash failures and marketplace policy failures so the Rust verifier can report the right layer.
