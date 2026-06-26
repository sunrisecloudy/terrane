//! The DL-24 exclusion guard: which `kv` namespaces are local-only / secret and
//! must NEVER be written to a bundle. The single chokepoint the guard test pins.

/// True iff a `kv` namespace holds **local-only / secret** data that must NEVER
/// be exported (DL-24): secrets, provider credentials, and device-local settings
/// / window state. The reserved `__forge/meta` namespace is explicitly portable
/// (applet manifests/programs + run counter), so it is excluded from this guard.
///
/// The match is by reserved namespace prefix so an applet cannot accidentally
/// (or maliciously) smuggle a secret out by choosing a clever key — the whole
/// namespace is dropped. Applet `ctx.storage` namespaces are `applet/<id>` and
/// are portable workspace data, so they are not matched here.
pub fn is_local_only_namespace(namespace: &str) -> bool {
    // Reserved secret / device-local buckets. Each entry is the bucket root; a
    // namespace is dropped when it IS the bucket exactly (a key stored directly
    // under the root, e.g. `secret`) OR is a child of it (`secret/<...>`). Matching
    // both forms closes the gap where an exact root namespace like `secret`,
    // `provider`, or `__device` (no trailing key) would otherwise slip past a
    // prefix-only check and be exported (review 061 P2).
    const LOCAL_ONLY_BUCKETS: &[&str] = &[
        "secret", // secret values
        "secrets",
        "provider", // provider credentials / tokens
        "credentials",
        "device", // device-local settings / window state
        "__device",
        "local", // local window state / transient UI
        "__local",
    ];
    LOCAL_ONLY_BUCKETS.iter().any(|bucket| {
        namespace == *bucket
            || namespace
                .strip_prefix(bucket)
                .is_some_and(|rest| rest.starts_with('/'))
    })
}
