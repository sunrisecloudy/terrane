//! forge-secrets: the `ctx.secrets` store + the net-header secret injector.
//!
//! Normative spec: prd-merged/07 **SC-13** / **SC-10** (secrets handling),
//! prd-merged/01 **CR-3** (`net` namespace secret refs). The contract:
//!
//!   - an applet references a secret **by name only** — it never sees the value;
//!   - the host resolves that name to a real value and injects it into the
//!     **outgoing** net-request header for an allowlisted destination *only*;
//!   - the resolved value is **never** readable as a string by applet JS, **never**
//!     serialized into the [`RunRecord`](forge_domain) trace or any LLM context,
//!     **never** logged, **never** synced.
//!
//! ## Trace-safety (SC-13)
//!
//! The recorded `net.fetch` args carry the **`secret_ref`** (the name), never the
//! resolved value: the runtime records the request *with the secret_ref header*,
//! and the host resolves + injects the real value **inside** the `host_call`
//! closure (so the live HTTP bridge gets the real header but the recording keeps
//! the `secret_ref`). On replay the recorded response is served and no secret is
//! needed at all.
//!
//! ## What lives here vs. host-side
//!
//! This crate is **wasm-clean**: it is the [`SecretStore`] trait, a redacting
//! [`SecretValue`] wrapper, an [`InMemorySecretStore`] dev/test backend, and the
//! pure [`resolve_secret_headers`] injector. The *real* OS-keychain backend
//! (Keychain / DPAPI / libsecret) is shell-side and out of scope for M0a — it is
//! just another [`SecretStore`] impl wired in by the host.
//!
//! ## Defence in depth with the net policy
//!
//! The egress [`NetPolicy`](forge_policy::NetPolicy) already gates a
//! `secret_ref` header at policy time: a `secret_ref` is permitted only on a
//! header name listed in the matched rule's `allow_secret_headers`. This injector
//! **re-checks the same gate** before it resolves a value, so the two agree and
//! the value is materialized only for an allowlisted header — even if a caller
//! reached the injector without the policy having run.

#![forbid(unsafe_code)]

use forge_domain::{CoreError, Result};
use forge_policy::HeaderValue;
use std::collections::BTreeMap;
use std::sync::RwLock;

/// A resolved secret value. The wrapper exists so the value is **never** printed:
/// its [`Debug`] and [`Display`] redact to `<secret:REDACTED>`, so a secret can
/// never leak into a log line, a panic message, a trace, or an error string by
/// accident. The plaintext is reachable **only** through [`expose`](Self::expose),
/// which the net-header injector calls at the last moment before handing the
/// header to the live HTTP client — and nowhere else (SC-13).
#[derive(Clone, PartialEq, Eq)]
pub struct SecretValue(String);

impl SecretValue {
    /// Wrap a plaintext secret. The plaintext is held privately and is only
    /// reachable via [`expose`](Self::expose).
    pub fn new(value: impl Into<String>) -> Self {
        SecretValue(value.into())
    }

    /// The plaintext value. The **only** way to read a secret as a string —
    /// reserved for the host-side injector that writes the real header into the
    /// outgoing request. NEVER call this on a path that reaches applet JS, the
    /// RunRecord trace, an LLM context, a log, or sync (SC-13).
    pub fn expose(&self) -> &str {
        &self.0
    }
}

/// Redacts: a secret value never renders its plaintext, even in a `{:?}`.
impl std::fmt::Debug for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SecretValue(<secret:REDACTED>)")
    }
}

/// Redacts: a secret value never renders its plaintext, even in a `{}`.
impl std::fmt::Display for SecretValue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("<secret:REDACTED>")
    }
}

/// The secret store an applet's `ctx.secrets` (by-name) refs resolve against, and
/// that the net-header injector reads from. The concrete OS-keychain backend is
/// **shell-side**; this trait is the wasm-clean seam, and [`InMemorySecretStore`]
/// is the dev/test backend.
///
/// `get` is the only method the injector needs. `store`/`delete` exist for a
/// shell to provision/rotate secrets out of band; they default to a
/// `PlatformUnavailable` error so a read-only backend (the common case) need not
/// implement them.
pub trait SecretStore {
    /// Resolve a secret by name. `Ok(None)` means "no such secret" (the injector
    /// turns that into a `PermissionDenied` for the referenced name, fail-closed);
    /// `Err` is a backend failure (e.g. a locked keychain).
    fn get(&self, name: &str) -> Result<Option<SecretValue>>;

    /// Provision/overwrite a secret (shell-side, out of band). Default: the
    /// backend is read-only and reports `PlatformUnavailable`.
    fn store(&self, name: &str, _value: SecretValue) -> Result<()> {
        Err(CoreError::PlatformUnavailable(format!(
            "secret store is read-only; cannot store secret {name:?}"
        )))
    }

    /// Remove a secret (shell-side, out of band). Default: read-only backend.
    fn delete(&self, name: &str) -> Result<()> {
        Err(CoreError::PlatformUnavailable(format!(
            "secret store is read-only; cannot delete secret {name:?}"
        )))
    }
}

/// An in-memory `name -> value` [`SecretStore`] for dev/tests. The real OS
/// keychain backend is shell-side and out of M0a scope; this backend keeps the
/// crate self-contained and lets tests assert the injector end-to-end without any
/// OS dependency. It is fully writable (`store`/`delete` mutate the map).
#[derive(Default)]
pub struct InMemorySecretStore {
    secrets: RwLock<BTreeMap<String, SecretValue>>,
}

impl InMemorySecretStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Build a store pre-seeded from `(name, value)` pairs.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let map = pairs
            .into_iter()
            .map(|(k, v)| (k.into(), SecretValue::new(v)))
            .collect();
        InMemorySecretStore { secrets: RwLock::new(map) }
    }

    /// Seed/overwrite a secret (test/dev convenience; mirrors `store`).
    pub fn insert(&self, name: impl Into<String>, value: impl Into<String>) {
        self.secrets
            .write()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name.into(), SecretValue::new(value));
    }

    /// Seed/overwrite a secret and return `self` (builder-style test/dev
    /// convenience used by runtime/core fixtures).
    pub fn with_secret(self, name: impl Into<String>, value: impl Into<String>) -> Self {
        self.insert(name, value);
        self
    }
}

impl Clone for InMemorySecretStore {
    fn clone(&self) -> Self {
        let secrets = self
            .secrets
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        InMemorySecretStore {
            secrets: RwLock::new(secrets),
        }
    }
}

impl SecretStore for InMemorySecretStore {
    fn get(&self, name: &str) -> Result<Option<SecretValue>> {
        let secrets = self.secrets.read().map_err(|_| {
            CoreError::RuntimeError("secret store lock poisoned while reading".to_string())
        })?;
        Ok(secrets.get(name).cloned())
    }

    fn store(&self, name: &str, value: SecretValue) -> Result<()> {
        let mut secrets = self.secrets.write().map_err(|_| {
            CoreError::RuntimeError("secret store lock poisoned while writing".to_string())
        })?;
        secrets.insert(name.to_string(), value);
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<()> {
        let mut secrets = self.secrets.write().map_err(|_| {
            CoreError::RuntimeError("secret store lock poisoned while deleting".to_string())
        })?;
        secrets.remove(name);
        Ok(())
    }
}

/// Redacted Debug: names are useful for tests/diagnostics, values are never
/// printed (SC-13).
impl std::fmt::Debug for InMemorySecretStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let secrets = self
            .secrets
            .read()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        f.debug_struct("InMemorySecretStore")
            .field("names", &secrets.keys().collect::<Vec<_>>())
            .field("values", &"<redacted>")
            .finish()
    }
}

/// Resolve any `secret_ref` request headers to their concrete plaintext values,
/// injecting each **only** into a header whose name is in `allow_secret_headers`
/// for the destination's matched net rule (SC-13).
///
/// Input `headers` mirror [`forge_policy::NetRequest::headers`]: each value is
/// either a [`HeaderValue::Literal`] (passed through unchanged) or a
/// [`HeaderValue::Secret`] carrying a `secret_ref` name. The output is the
/// concrete header map ready to hand to the live HTTP client — every secret_ref
/// replaced by its resolved value.
///
/// Fail-closed semantics (must agree with the [`NetPolicy`](forge_policy::NetPolicy)
/// `allow_secret_headers` gate and the `fixtures/secrets/*` conformance cases):
///   - a `secret_ref` header whose name is **not** in `allow_secret_headers` ⇒
///     [`CoreError::PermissionDenied`] (a *policy* denial: the secret may not go
///     to this destination/header — `secret_header_not_allowlisted_denied`);
///   - a `secret_ref` naming a secret the store does **not** have ⇒
///     [`CoreError::RuntimeError`] (a *resolution* failure, distinct from a policy
///     denial: the grant is valid but the host cannot resolve the named secret —
///     `unknown_secret_name_error`). The header is never sent empty/placeholder;
///   - a store backend failure ⇒ propagated as the store's [`CoreError`].
///
/// In every error case the message carries only the secret **name**, never the
/// value, so a secret can never leak through an error string.
///
/// The header-name allowlist comparison is ASCII-case-insensitive, matching the
/// policy engine. The resolved [`String`] values are **never** printed and live
/// only in the returned map, which the caller hands straight to the live HTTP
/// bridge **inside** the record/replay closure — so the recording keeps the
/// `secret_ref`, never the value.
pub fn resolve_secret_headers(
    headers: &BTreeMap<String, HeaderValue>,
    allow_secret_headers: &[String],
    store: &dyn SecretStore,
) -> Result<BTreeMap<String, String>> {
    let mut resolved = BTreeMap::new();
    for (name, value) in headers {
        match value {
            HeaderValue::Literal(v) => {
                resolved.insert(name.clone(), v.clone());
            }
            HeaderValue::Secret { secret_ref } => {
                // Gate: the destination's matched rule must list this header name
                // in allow_secret_headers (defence-in-depth with NetPolicy).
                let allowed = allow_secret_headers
                    .iter()
                    .any(|h| h.eq_ignore_ascii_case(name));
                if !allowed {
                    return Err(CoreError::PermissionDenied(format!(
                        "secret injection denied: header {name:?} is not in the destination's allow_secret_headers (secret_ref {secret_ref:?})"
                    )));
                }
                // Resolve the name → value. An unknown secret is fail-closed as a
                // RuntimeError (a resolution failure — the grant is valid but the
                // host cannot resolve the named secret), distinct from the
                // PermissionDenied policy gate above. Never a silently-empty
                // header; the message carries only the NAME, never the value.
                let secret = store.get(secret_ref)?.ok_or_else(|| {
                    CoreError::RuntimeError(format!(
                        "secret injection failed: no secret named {secret_ref:?} is provisioned for header {name:?}"
                    ))
                })?;
                resolved.insert(name.clone(), secret.expose().to_string());
            }
        }
    }
    Ok(resolved)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allow(names: &[&str]) -> Vec<String> {
        names.iter().map(|s| s.to_string()).collect()
    }

    // --- SecretValue REDACTS (no value ever reaches Debug/Display/logs) ------

    #[test]
    fn secret_value_debug_redacts() {
        let s = SecretValue::new("hunter2-super-secret");
        let dbg = format!("{s:?}");
        assert!(!dbg.contains("hunter2"), "Debug leaked the secret: {dbg}");
        assert_eq!(dbg, "SecretValue(<secret:REDACTED>)");
    }

    #[test]
    fn secret_value_display_redacts() {
        let s = SecretValue::new("hunter2-super-secret");
        let shown = format!("{s}");
        assert!(!shown.contains("hunter2"), "Display leaked the secret: {shown}");
        assert_eq!(shown, "<secret:REDACTED>");
    }

    #[test]
    fn secret_value_expose_is_the_only_reader() {
        let s = SecretValue::new("plaintext");
        assert_eq!(s.expose(), "plaintext");
    }

    // --- SecretStore get: known / unknown / write-back -----------------------

    #[test]
    fn in_memory_store_get_known_and_unknown() {
        let store = InMemorySecretStore::from_pairs([("secret_weather", "KEY-123")]);
        let got = store.get("secret_weather").unwrap().unwrap();
        assert_eq!(got.expose(), "KEY-123");
        assert!(store.get("missing").unwrap().is_none(), "unknown secret is None, not an error");
    }

    #[test]
    fn in_memory_store_store_and_delete_roundtrip() {
        let store = InMemorySecretStore::new();
        store.store("k", SecretValue::new("v")).unwrap();
        assert_eq!(store.get("k").unwrap().unwrap().expose(), "v");
        store.delete("k").unwrap();
        assert!(store.get("k").unwrap().is_none());
    }

    #[test]
    fn read_only_default_backend_refuses_writes() {
        // A backend that only implements `get` inherits the fail-closed default
        // store/delete (PlatformUnavailable), so a read-only keychain is safe.
        struct ReadOnly;
        impl SecretStore for ReadOnly {
            fn get(&self, _name: &str) -> Result<Option<SecretValue>> {
                Ok(None)
            }
        }
        let ro = ReadOnly;
        assert_eq!(
            ro.store("k", SecretValue::new("v")).unwrap_err().code(),
            "PlatformUnavailable"
        );
        assert_eq!(ro.delete("k").unwrap_err().code(), "PlatformUnavailable");
    }

    // --- resolver: inject allowlisted, error on unknown / non-allowlisted ----

    #[test]
    fn resolver_injects_an_allowlisted_secret() {
        let store = InMemorySecretStore::from_pairs([("secret_weather", "Bearer XYZ")]);
        let mut headers = BTreeMap::new();
        headers.insert(
            "Authorization".to_string(),
            HeaderValue::Secret { secret_ref: "secret_weather".to_string() },
        );
        headers.insert(
            "Accept".to_string(),
            HeaderValue::Literal("application/json".to_string()),
        );
        let resolved =
            resolve_secret_headers(&headers, &allow(&["Authorization"]), &store).unwrap();
        // The allowlisted secret header now carries the resolved value; the
        // literal header passes through unchanged.
        assert_eq!(resolved.get("Authorization").unwrap(), "Bearer XYZ");
        assert_eq!(resolved.get("Accept").unwrap(), "application/json");
    }

    #[test]
    fn resolver_header_allowlist_is_case_insensitive() {
        // The rule lists "Authorization"; the request header is "authorization".
        let store = InMemorySecretStore::from_pairs([("s", "v")]);
        let mut headers = BTreeMap::new();
        headers.insert(
            "authorization".to_string(),
            HeaderValue::Secret { secret_ref: "s".to_string() },
        );
        let resolved =
            resolve_secret_headers(&headers, &allow(&["Authorization"]), &store).unwrap();
        assert_eq!(resolved.get("authorization").unwrap(), "v");
    }

    #[test]
    fn resolver_errors_on_unknown_secret() {
        // An allowlisted header naming a missing secret is a RESOLUTION failure
        // (RuntimeError), distinct from the PermissionDenied policy gate — matches
        // the `unknown_secret_name_error` fixture's expected_error_code.
        let store = InMemorySecretStore::new(); // empty
        let mut headers = BTreeMap::new();
        headers.insert(
            "Authorization".to_string(),
            HeaderValue::Secret { secret_ref: "nope".to_string() },
        );
        let err =
            resolve_secret_headers(&headers, &allow(&["Authorization"]), &store).unwrap_err();
        assert_eq!(err.code(), "RuntimeError");
        // The error names the secret (so the host can diagnose) but never a value.
        assert!(err.to_string().contains("nope"), "{err}");
    }

    #[test]
    fn resolver_errors_on_non_allowlisted_header() {
        let store = InMemorySecretStore::from_pairs([("s", "leak-me")]);
        let mut headers = BTreeMap::new();
        // secret_ref on a header NOT in allow_secret_headers ⇒ PermissionDenied,
        // and crucially the value is NEVER read (the store value never leaks).
        headers.insert(
            "X-Sneaky".to_string(),
            HeaderValue::Secret { secret_ref: "s".to_string() },
        );
        let err =
            resolve_secret_headers(&headers, &allow(&["Authorization"]), &store).unwrap_err();
        assert_eq!(err.code(), "PermissionDenied");
        assert!(!err.to_string().contains("leak-me"), "value leaked into error: {err}");
    }

    #[test]
    fn resolver_passes_through_when_no_secret_refs() {
        // Pure-literal headers need no store interaction and round-trip unchanged.
        let store = InMemorySecretStore::new();
        let mut headers = BTreeMap::new();
        headers.insert(
            "Accept".to_string(),
            HeaderValue::Literal("text/plain".to_string()),
        );
        let resolved = resolve_secret_headers(&headers, &[], &store).unwrap();
        assert_eq!(resolved.get("Accept").unwrap(), "text/plain");
    }

    #[test]
    fn resolved_value_never_appears_in_any_debug_render() {
        // The resolved string lives only in the returned concrete map; the
        // SecretValue it came from still redacts everywhere it is printed.
        let store = InMemorySecretStore::from_pairs([("s", "TOPSECRET")]);
        let dbg = format!("{:?}", store.get("s").unwrap().unwrap());
        assert!(!dbg.contains("TOPSECRET"), "store value rendered in Debug: {dbg}");
    }
}
