//! The SC-15 / MP-4 package-signing trust pipeline for an `applet.install`.
//!
//! Extracted verbatim from `workspace.rs` (a pure move, /simplify #6) so the
//! facade reads as orchestration while the signed-install verify/bind logic lives
//! in one focused module. NOTHING about behavior changes — the same functions in
//! the same order, only relocated.
//!
//! [`verify_install_signature`] is the single entry point
//! [`cmd_applet_install`](super::WorkspaceCore) calls. Its FAIL-CLOSED ORDERED
//! pipeline is load-bearing and preserved exactly:
//!
//!   verify (crypto + integrity + optional publisher policy)
//!     -> [`bind_signature_to_sources`]  (the signature blesses EXACTLY the
//!        installed code — review 080 #1)
//!     -> [`bind_signature_to_manifest`] (the runtime enforces EXACTLY the signed
//!        capability boundary + limits — review 082/083)
//!     -> [`reject_unknown_signed_policy_fields`] (refuse any signed policy field
//!        this core cannot enforce — review 086 #1)
//!
//! all BEFORE any install state is written, with no conditional short-circuit
//! added or removed. A failure at any layer is a typed `ValidationError` that
//! rejects the install so nothing is stored.

use forge_domain::{AppletId, CoreCommand, CoreError, Manifest, Result};
use forge_signing::{verify_package, Package, PublisherTrust, TrustOutcome};

use super::InstallTrust;

/// Verify the optional package signature carried on an `applet.install`
/// (SC-15 / MP-4), returning the [`InstallTrust`] to record.
///
/// The optional `signature` payload field is the prd-merged/08 MP-4 signed
/// package — the exact T012 fixture shape:
///
/// ```json
/// "signature": {
///   "package": { "manifest": {…}, "files": [{path, content, sha256}], "hashes": {…} },
///   "signature": "ed25519:…",
///   "public_key": "ed25519:…" | "<PEM SubjectPublicKeyInfo>",
///   "publisher_trust": { "publisher": "...", "status": "unknown" | …, "valid_until": "…" }
/// }
/// ```
///
/// When the field is ABSENT the install is [`InstallTrust::Unsigned`] (the M0a
/// default — signing is not yet mandatory). When PRESENT the package is verified
/// with [`forge_signing::verify_package`] over the canonical `terrane/sig/v1`
/// preimage:
///
///   - any failure — crypto (bad/garbage/wrong-key signature), `package_hash`
///     (a file/manifest/permissions/policy region tampered after signing), or
///     `policy` (publisher not trusted / expired) — is surfaced as
///     `ValidationError("package signature invalid: <layer>: <reason>")`, so the
///     caller REJECTS the install;
///   - the verified package is then BOUND to `sources` via
///     [`bind_signature_to_sources`] so the signature only blesses the code
///     actually being installed (review 080 #1);
///   - the signed package's MANIFEST is BOUND to the top-level `manifest` that is
///     stored and enforced via [`bind_signature_to_manifest`] (review 082 #1), so
///     a valid signature over code cannot be installed under a BROADER runtime
///     policy (extra capabilities / different app id, entrypoint, or limits) than
///     the publisher signed — the runtime enforces exactly the signed boundary;
///   - on success the verified publisher / key id (+ whether the policy layer was
///     enforced) is returned as [`InstallTrust::Signed`].
///
/// `publisher_trust` is optional: present → the marketplace-policy layer is
/// enforced (the publisher must be trusted and unexpired); absent → crypto +
/// integrity only, the M0a "verify when present, surface the result" default.
pub(super) fn verify_install_signature(
    cmd: &CoreCommand,
    applet_id: &AppletId,
    manifest: &Manifest,
    sources: &serde_json::Map<String, serde_json::Value>,
) -> Result<InstallTrust> {
    let signature = match cmd.payload.get("signature") {
        None | Some(serde_json::Value::Null) => return Ok(InstallTrust::Unsigned),
        Some(sig) => sig,
    };

    // The signed package (MP-4 `files`/`manifest`/`hashes`).
    let package: Package = signed_field(signature, "package")?;
    let signature_str = signed_str(signature, "signature")?;
    let public_key = signed_str(signature, "public_key")?;

    // Optional marketplace-policy input (the publisher trust set). Present →
    // enforce the policy layer; absent → crypto + integrity only.
    let publisher_trust: Option<PublisherTrust> = match signature.get("publisher_trust") {
        None | Some(serde_json::Value::Null) => None,
        Some(v) => Some(serde_json::from_value(v.clone()).map_err(|e| {
            CoreError::ValidationError(format!(
                "applet.install `signature.publisher_trust` is malformed: {e}"
            ))
        })?),
    };
    let publisher_trust_enforced = publisher_trust.is_some();

    // Verify over the canonical preimage. A CRYPTO/integrity/policy failure
    // rejects the install; the typed reason names the failing layer.
    match verify_package(
        &package,
        &signature_str,
        &public_key,
        publisher_trust.as_ref(),
    ) {
        TrustOutcome::Trusted => {
            // BIND the verified package to the install payload (review 080 #1):
            // a valid signature only blesses the EXACT code it signed. The signed
            // package's files must be identical (path + content) to the `sources`
            // that will actually be compiled and stored — otherwise a caller could
            // attach any valid signed package to arbitrary top-level code and still
            // be reported as `Signed`.
            bind_signature_to_sources(&package, sources)?;

            // BIND the signed package's manifest/policy to the top-level
            // `manifest` that is stored and enforced (review 082 #1 / 083): the
            // runtime must enforce EXACTLY the capability boundary + resource
            // limits the publisher signed, not a broader one. A signed install
            // whose top-level manifest grants more — a different app id, a wider
            // resource limit, a looser net rule, or a different entrypoint — than
            // the signed package manifest is rejected. The requested `applet_id`
            // is bound to the signed `appId` so a valid signature for one app
            // identity cannot bless a different local applet id (review 083 #1).
            bind_signature_to_manifest(&package, applet_id, manifest, sources)?;

            // Record the verified publisher identity for later trust reporting.
            let publisher = manifest_string(&package.manifest, "publisher");
            let key_id = manifest_string(&package.manifest, "keyId");
            Ok(InstallTrust::Signed {
                publisher,
                key_id,
                publisher_trust_enforced,
            })
        }
        TrustOutcome::Rejected(err) => Err(CoreError::ValidationError(format!(
            "package signature invalid: {}: {}",
            err.layer.as_str(),
            err.reason
        ))),
    }
}

/// Confirm a verified signed `package` actually describes the code being
/// installed (review 080 #1). Without this, a valid signature over package A
/// could be attached to an install of arbitrary code B and still report
/// `Signed` — the signature would bless an app that is not the one installed.
///
/// The bind is exact: the signed package's files and the install `sources` must
/// be the SAME set of `path -> content` entries. The signature already attests
/// the files' integrity (forge-signing verified each `contentHash`/per-file
/// digest), so matching the install sources to those files transitively binds
/// the signature to exactly what is compiled and stored. A mismatch — an extra,
/// missing, or differing file — is a `package_hash`-class rejection (the package
/// does not match the payload), surfaced like any other signature failure so the
/// install is rejected and nothing is stored.
fn bind_signature_to_sources(
    package: &Package,
    sources: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let reject = |reason: String| {
        Err(CoreError::ValidationError(format!(
            "package signature invalid: package_hash: {reason}"
        )))
    };

    // Same number of files (so the signed set has no extra files and the install
    // has no unsigned ones).
    if package.files.len() != sources.len() {
        return reject(format!(
            "signed package declares {} file(s) but the install carries {} source(s)",
            package.files.len(),
            sources.len()
        ));
    }

    // Every signed file must be present in the install with identical content,
    // and every install source must therefore be covered (the equal-length check
    // above turns "every signed file matches a source" into a bijection).
    for file in &package.files {
        match sources.get(&file.path).and_then(|v| v.as_str()) {
            Some(content) if content == file.content => {}
            Some(_) => {
                return reject(format!(
                    "install source {:?} content does not match the signed package",
                    file.path
                ));
            }
            None => {
                return reject(format!(
                    "signed file {:?} is not among the install sources",
                    file.path
                ));
            }
        }
    }
    Ok(())
}

/// Confirm the top-level install `manifest` enforces EXACTLY the capability
/// boundary + resource limits the publisher signed (review 082 #1).
///
/// [`bind_signature_to_sources`] binds the signed *code* to the install sources,
/// but `cmd_applet_install` stores and enforces a SEPARATE top-level
/// [`Manifest`] (its `capabilities`/`limits` are what the runtime's policy engine
/// checks every `ctx.*` call against). Without this bind, a valid signature over
/// code could be installed as `Signed` under a BROADER policy than the publisher
/// signed — e.g. `storage app/*` + `db tasks` where the publisher only signed
/// `storage notes/*` + `db notes` — so the runtime would enforce a capability
/// boundary the publisher never blessed.
///
/// This crate cannot *derive* a forge-domain [`Manifest`] from the signed package
/// manifest: the signed shape (prd-merged/08 MP-4 — `appId`, `permissions[]`,
/// `capabilities.{storage,db}.{read,write}`, `capabilities.ui`, `networkPolicy`,
/// `resourceBudget`) carries no `min_api`, so a clean conversion is impossible.
/// Instead we take option (b) — **reject on mismatch**: the policy-bearing fields
/// the publisher signed must match the install manifest EXACTLY. A mismatch is
/// surfaced like any other signature failure (a `ValidationError`), so the
/// install is rejected and nothing is stored.
///
/// The compared dimensions are exactly the runtime-enforced policy surface (review
/// 083 widened this from the prior partial surface):
///   - `appId` vs the requested `applet_id` — a valid signature for one app
///     identity must not bless a DIFFERENT local applet id (review 083 #1);
///   - `capabilities.storage.read` / `.write` (as a set);
///   - `capabilities.db.read` / `.write` (as a set);
///   - `capabilities.ui` (bool — signed `true`/absent is permissive in M0a);
///   - the WHOLE normalized net rule (method, url, `max_response_bytes`,
///     `max_body_bytes`, `timeout_ms`, request/response content types,
///     `allow_secret_headers`) — a signed install must not loosen a cap or add a
///     secret header (review 083 #3);
///   - EVERY enforced resource limit — `wall_ms`, `memory_bytes`, `fuel`,
///     `max_host_calls`, `storage_bytes`, `log_bytes`. A limit the signed
///     `resourceBudget` declares must equal the install's; a limit the signed
///     budget OMITS must equal the runtime default, so a signed install cannot
///     widen an unstated budget (review 083 #2);
///   - the MP-8 `compatibility` floor (`required_features` + `min_app_version`) —
///     the install's must EQUAL the signed package's (review 170). The install
///     manifest's compatibility is negotiated against the trusted client feature
///     registry BEFORE the install is accepted, but the signature covers only THIS
///     signed manifest; binding them equal means the negotiation that ran on the
///     install floor also vouches for the SIGNED floor, so a signed package
///     declaring a future `required_features` cannot be installed under a STRIPPED
///     top-level manifest that bypasses the gate;
///   - the runnable `entrypoint`. For a single-file signed package the entrypoint
///     must be that one file; a signed MULTI-FILE package is rejected because the
///     signed manifest does not (yet) carry an entrypoint to pin which file runs
///     (review 083 #4).
fn bind_signature_to_manifest(
    package: &Package,
    applet_id: &AppletId,
    install: &Manifest,
    sources: &serde_json::Map<String, serde_json::Value>,
) -> Result<()> {
    let reject = |reason: String| -> Result<()> {
        Err(CoreError::ValidationError(format!(
            "install manifest does not match the signed package manifest: {reason}"
        )))
    };

    let signed = &package.manifest;
    let signed_caps = signed.get("capabilities");

    // --- app identity: the signed appId must equal the installed applet id ----
    // (review 083 #1). The signed preimage binds `appId`, so a valid signature
    // for one app identity cannot be attached to a different local applet id and
    // still report Signed — provenance and upgrade identity stay bound.
    let signed_app_id = signed.get("appId").and_then(serde_json::Value::as_str);
    match signed_app_id {
        Some(id) if id == applet_id.as_str() => {}
        Some(id) => {
            return reject(format!(
                "appId {id:?} differs from the installed applet id {:?}",
                applet_id.as_str()
            ));
        }
        None => {
            return reject("signed package manifest is missing `appId`".into());
        }
    }

    // --- fail closed on UNKNOWN signed policy fields (review 086 #1) -----------
    //     The signed manifest is hashed and signed WHOLE, but the bind below
    //     narrows it through today-only shapes: capability sub-objects, each
    //     `networkPolicy.allow[]` rule, and `resourceBudget` are interpreted
    //     key-by-key, and the runtime `NetRule` tolerates unknown fields for
    //     forward-compat (see `forge_domain::NetRule`). That tolerance is fine
    //     for the runtime, but on the SIGNED-INSTALL path it is a hole: a signed
    //     package could carry a FUTURE, tighter constraint this core does not
    //     understand (a new net field, a new `resourceBudget` limit such as
    //     `network_bytes`/`output_bytes`, a new capability namespace) and we
    //     would silently install it as Signed WITHOUT enforcing that constraint.
    //     prd-merged/08 §08:24 is fail-closed: clients REFUSE packages that
    //     declare features they do not support. So here — scoped to the signed
    //     bind, NOT the global runtime tolerance — reject the install whenever a
    //     signed policy sub-object contains a key this core cannot enforce.
    reject_unknown_signed_policy_fields(signed, signed_caps)?;

    // --- storage scopes (read/write), compared as order-independent sets ------
    for action in ["read", "write"] {
        let signed_scope = signed_string_set(signed_caps, "storage", action);
        let install_scope: std::collections::BTreeSet<&str> = match action {
            "read" => install.capabilities.storage.read.iter(),
            _ => install.capabilities.storage.write.iter(),
        }
        .map(String::as_str)
        .collect();
        if signed_scope != install_scope {
            return reject(format!(
                "storage.{action} grant {:?} differs from the signed {:?}",
                sorted_vec(&install_scope),
                sorted_vec(&signed_scope)
            ));
        }
    }

    // --- db scopes (read/write), compared as order-independent sets -----------
    for action in ["read", "write"] {
        let signed_scope = signed_string_set(signed_caps, "db", action);
        let install_scope: std::collections::BTreeSet<&str> = match action {
            "read" => install.capabilities.db.read.iter(),
            _ => install.capabilities.db.write.iter(),
        }
        .map(String::as_str)
        .collect();
        if signed_scope != install_scope {
            return reject(format!(
                "db.{action} grant {:?} differs from the signed {:?}",
                sorted_vec(&install_scope),
                sorted_vec(&signed_scope)
            ));
        }
    }

    // --- ui: a signed `ui: false` must not be installed as `ui: true` ---------
    // (absent signed `ui` is treated as granted, matching the M0a manifest
    // default where an absent `capabilities.ui` grants UI).
    let signed_ui = signed_caps
        .and_then(|c| c.get("ui"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(true);
    if signed_ui != install.capabilities.ui {
        return reject(format!(
            "ui grant {} differs from the signed {signed_ui}",
            install.capabilities.ui
        ));
    }

    // --- network egress: the WHOLE normalized net rule set must match (review
    //     083 #3). The signed shape is `networkPolicy.allow[]`; the install shape
    //     is `capabilities.net[]`. Both are normalized to the SAME [`NetRule`]
    //     type and compared as order-independent sets, so a signed install that
    //     keeps the same (method, url) but loosens a cap (`max_response_bytes`,
    //     `max_body_bytes`, `timeout_ms`), changes a content-type constraint, or
    //     ADDS an `allow_secret_headers` entry no longer passes — every SC-5
    //     constraint the runtime enforces is bound, not just routing.
    let signed_net = signed_net_rules(signed)?;
    let install_net: std::collections::BTreeSet<NormalizedNetRule> = install
        .capabilities
        .net
        .rules()
        .iter()
        .map(NormalizedNetRule::from_rule)
        .collect();
    if signed_net != install_net {
        return reject(format!(
            "network egress grant {:?} differs from the signed {:?}",
            sorted_net_rules(&install_net),
            sorted_net_rules(&signed_net)
        ));
    }

    // --- filesystem grants: the WHOLE normalized files rule set must match
    //     (review 109 #1). Mirrors the net bind exactly: the signed shape is
    //     `capabilities.files.{read,write}[]`; the install shape is the same. Both
    //     sides normalize every `FileRule` (handle, path_glob, max_bytes,
    //     content_types) into [`NormalizedFileRule`] and compare order-independently
    //     per action, so a signed install that ADDS a grant, WIDENS a glob, raises
    //     `max_bytes`, or extends `content_types` no longer matches — every CR-3
    //     confinement the runtime enforces (`forge_runtime::host`) is bound, not
    //     just the handle. The runtime tolerates unknown `FileRule` fields for
    //     forward-compat, but `reject_unknown_signed_policy_fields` already rejects
    //     any unknown signed files key, so the signed/install normalization is total.
    for action in ["read", "write"] {
        let signed_files = signed_file_rules(signed_caps, action)?;
        let install_rules = match action {
            "read" => &install.capabilities.files.read,
            _ => &install.capabilities.files.write,
        };
        let install_files: std::collections::BTreeSet<NormalizedFileRule> = install_rules
            .iter()
            .map(NormalizedFileRule::from_rule)
            .collect();
        if signed_files != install_files {
            return reject(format!(
                "files.{action} grant {:?} differs from the signed {:?}",
                sorted_file_rules(&install_files),
                sorted_file_rules(&signed_files)
            ));
        }
    }

    // --- resource limits: EVERY enforced limit must match (review 083 #2). The
    //     runtime enforces wall_ms, memory_bytes, fuel, max_host_calls,
    //     storage_bytes, and log_bytes from the stored top-level manifest, so a
    //     signed install must not widen ANY of them. A limit the signed
    //     `resourceBudget` declares must equal the install's value; a limit the
    //     signed budget OMITS is bound to the runtime DEFAULT, so a signed install
    //     cannot silently widen an unstated budget. The runtime-enforced default
    //     is the single source of truth ([`forge_domain::Limits::default`]).
    let budget = signed.get("resourceBudget");
    let defaults = forge_domain::Limits::default();
    let limit_checks: [(&str, u64, u64); 6] = [
        ("wall_ms", install.limits.wall_ms, defaults.wall_ms),
        ("memory_bytes", install.limits.memory_bytes, defaults.memory_bytes),
        ("fuel", install.limits.fuel, defaults.fuel),
        ("max_host_calls", install.limits.max_host_calls, defaults.max_host_calls),
        ("storage_bytes", install.limits.storage_bytes, defaults.storage_bytes),
        ("log_bytes", install.limits.log_bytes, defaults.log_bytes),
    ];
    for (name, install_value, default_value) in limit_checks {
        // The signed expectation: the value the signed budget declared, or the
        // runtime default when the signed budget omits this limit.
        let signed_value = budget
            .and_then(|b| b.get(name))
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(default_value);
        if install_value != signed_value {
            return reject(format!(
                "limits.{name} {install_value} differs from the signed {signed_value}"
            ));
        }
    }

    // --- compatibility / required_features: the signed MP-8 floor must be bound
    //     to the install (review 170). `cmd_applet_install` negotiates the install
    //     `manifest.compatibility.required_features` (+ `min_app_version`) against
    //     the TRUSTED client feature registry BEFORE accepting the install, but the
    //     signature does NOT cover that top-level manifest — only this signed
    //     `package.manifest` does. Without binding it, a signed package whose
    //     `compatibility.required_features` names a FUTURE feature this client does
    //     not support could still install: the caller sends a top-level manifest with
    //     EMPTY/weaker `compatibility`, the negotiation runs against the STRIPPED
    //     compatibility and passes, and the signature stays valid over the signed
    //     package (whose compatibility it never re-checked) — bypassing MP-8. Bind it
    //     fail-closed like every other policy-bearing field: the install's
    //     `compatibility` must EQUAL the signed package's. Because the negotiation
    //     already ran on the install `compatibility` and it must now equal the signed
    //     one, the MP-8 gate effectively ran against the SIGNED (non-strippable)
    //     compatibility — a signed install that strips or weakens the declared floor
    //     is rejected, and a signed FUTURE feature only installs when the client
    //     genuinely supports it.
    let signed_compat = signed_compatibility(signed)?;
    if signed_compat != install.compatibility {
        return reject(format!(
            "compatibility {:?} differs from the signed {:?}",
            describe_compatibility(&install.compatibility),
            describe_compatibility(&signed_compat)
        ));
    }

    // --- entrypoint: the runnable entrypoint must be bound to the signed package
    //     (review 083 #4). The signed manifest does not (yet) carry an entrypoint
    //     field, so a signed MULTI-FILE package cannot pin which file runs — the
    //     install path would otherwise let a caller choose any signed file as the
    //     entrypoint. A single-file signed package is unambiguous: the one signed
    //     file IS the entrypoint, so the install's `entrypoint` must equal that
    //     file's path. Reject multi-file signed installs until the signed manifest
    //     can represent the entrypoint.
    if package.files.len() > 1 {
        return reject(format!(
            "signed multi-file packages are not installable yet: the signed manifest \
             carries no entrypoint to bind which of the {} files runs",
            package.files.len()
        ));
    }
    // bind_signature_to_sources (run before this) already proved `sources` equals
    // the signed files, so for a single-file package the lone source key is the
    // signed entrypoint path. Bind the install's chosen entrypoint to it.
    if let Some(signed_entry) = sources.keys().next() {
        if install.entrypoint != *signed_entry {
            return reject(format!(
                "entrypoint {:?} differs from the signed file {signed_entry:?}",
                install.entrypoint
            ));
        }
    }

    Ok(())
}

/// The set of strings under `manifest.capabilities.<ns>.<action>` (e.g.
/// `capabilities.storage.read`), or the empty set when the namespace/action is
/// absent. Used to compare a signed package's capability scopes against the
/// install manifest's order-independently.
fn signed_string_set<'a>(
    capabilities: Option<&'a serde_json::Value>,
    namespace: &str,
    action: &str,
) -> std::collections::BTreeSet<&'a str> {
    capabilities
        .and_then(|c| c.get(namespace))
        .and_then(|ns| ns.get(action))
        .and_then(serde_json::Value::as_array)
        .map(|arr| arr.iter().filter_map(serde_json::Value::as_str).collect())
        .unwrap_or_default()
}

/// Fail closed when a SIGNED package's policy carries a key this core cannot
/// enforce (review 086 #1).
///
/// The signed manifest is hashed/signed whole, but `bind_signature_to_manifest`
/// narrows it through today-only shapes — so an unknown key in a policy
/// sub-object would otherwise be dropped on the floor and the package would
/// install as Signed without that (possibly tighter) constraint being enforced.
/// This rejects, scoped to the signed-install bind path only, leaving the
/// runtime [`NetRule`](forge_domain::NetRule) forward-compat tolerance intact
/// for unsigned/already-installed manifests.
///
/// The known-key sets are the exact shapes the rest of this bind interprets:
///   - `capabilities`: `storage`, `db`, `ui`, `net`, `files` (and `storage`/`db`
///     each carry only `read`/`write`; `files` carries `read`/`write` arrays of
///     [`FileRule`](forge_domain::FileRule)-shaped entries);
///   - each `networkPolicy.allow[]` rule: the [`NetRule`](forge_domain::NetRule)
///     fields the policy engine enforces;
///   - `resourceBudget`: the six enforced limit keys.
///
/// A non-object where an object is expected, or any extra key, is a typed
/// rejection (never a panic).
fn reject_unknown_signed_policy_fields(
    signed: &serde_json::Value,
    signed_caps: Option<&serde_json::Value>,
) -> Result<()> {
    // The SC-5 constraints a `NetRule` carries — kept in lockstep with
    // `forge_domain::NetRule` so a NEW signed net field forces an update here
    // (and thus a deliberate enforcement decision) rather than silently passing.
    const NET_RULE_KEYS: &[&str] = &[
        "method",
        "url",
        "max_response_bytes",
        "max_body_bytes",
        "timeout_ms",
        "request_content_types",
        "response_content_types",
        "allow_secret_headers",
    ];
    // The CR-3 constraints a `FileRule` carries — kept in lockstep with
    // `forge_domain::FileRule` so a NEW signed files field forces an update here
    // (and thus a deliberate enforcement decision) rather than silently passing.
    const FILE_RULE_KEYS: &[&str] = &["handle", "path_glob", "max_bytes", "content_types"];
    // The resource limits this core actually enforces (mirrors the six-limit
    // bind below and `forge_domain::Limits`).
    const BUDGET_KEYS: &[&str] = &[
        "wall_ms",
        "fuel",
        "memory_bytes",
        "max_host_calls",
        "storage_bytes",
        "log_bytes",
    ];

    let unknown = |where_: &str, key: &str| -> Result<()> {
        Err(CoreError::ValidationError(format!(
            "install manifest does not match the signed package manifest: the signed \
             package declares an unsupported {where_} field {key:?} this core cannot \
             enforce; refusing to install it as Signed (review 086 #1)"
        )))
    };
    // Reject when `value` (when present) is an object carrying a key outside
    // `known`; a present-but-non-object policy field is also a rejection because
    // the bind cannot interpret it.
    let check_object = |where_: &str,
                        value: Option<&serde_json::Value>,
                        known: &[&str]|
     -> Result<()> {
        let value = match value {
            Some(v) => v,
            None => return Ok(()),
        };
        let obj = value.as_object().ok_or_else(|| {
            CoreError::ValidationError(format!(
                "install manifest does not match the signed package manifest: the signed \
                 package's {where_} is not an object (review 086 #1)"
            ))
        })?;
        for key in obj.keys() {
            if !known.contains(&key.as_str()) {
                return unknown(where_, key);
            }
        }
        Ok(())
    };

    // capabilities.* — only the namespaces this core maps are allowed.
    check_object(
        "capabilities",
        signed_caps,
        &["storage", "db", "ui", "net", "files"],
    )?;
    if let Some(caps) = signed_caps {
        for ns in ["storage", "db"] {
            check_object(
                &format!("capabilities.{ns}"),
                caps.get(ns),
                &["read", "write"],
            )?;
        }
        // capabilities.net[] is policy-bearing and covered by the signed policy
        // hash, so each entry must pass the SAME known-key check as
        // networkPolicy.allow[]. Otherwise a future/tighter net constraint hidden
        // under capabilities.net[] would install as Signed but go unenforced
        // (review 089 #1).
        if let Some(net) = caps.get("net").and_then(serde_json::Value::as_array) {
            for rule in net {
                check_object("capabilities.net[]", Some(rule), NET_RULE_KEYS)?;
            }
        }
        // capabilities.files.{read,write}[] is policy-bearing and covered by the
        // signed policy hash, so — exactly like capabilities.net[] — each entry
        // must carry only known `FileRule` fields. Otherwise a future/tighter
        // files constraint (a new per-action cap) hidden under the signed grant
        // would install as Signed but go unenforced (review 109 #1). The
        // `capabilities.files` object itself may only carry read/write.
        check_object("capabilities.files", caps.get("files"), &["read", "write"])?;
        if let Some(files) = caps.get("files") {
            for action in ["read", "write"] {
                if let Some(rules) = files.get(action).and_then(serde_json::Value::as_array) {
                    for rule in rules {
                        check_object(
                            &format!("capabilities.files.{action}[]"),
                            Some(rule),
                            FILE_RULE_KEYS,
                        )?;
                    }
                }
            }
        }
    }

    // networkPolicy — only the allowlist shape this core enforces is allowed.
    check_object("networkPolicy", signed.get("networkPolicy"), &["allow"])?;
    if let Some(network_policy) = signed.get("networkPolicy") {
        if let Some(allow_value) = network_policy.get("allow") {
            let allow = allow_value.as_array().ok_or_else(|| {
                CoreError::ValidationError(
                    "install manifest does not match the signed package manifest: the signed \
                     package's networkPolicy.allow is not an array (review 086 #1)"
                        .into(),
                )
            })?;
            for rule in allow {
                check_object("networkPolicy.allow[]", Some(rule), NET_RULE_KEYS)?;
            }
        }
    }

    // resourceBudget — only the six enforced limits may appear.
    check_object("resourceBudget", signed.get("resourceBudget"), BUDGET_KEYS)?;

    Ok(())
}

/// The full, normalized form of one network egress rule — every SC-5 constraint
/// the runtime enforces (review 083 #3), not just routing. Both a signed
/// `networkPolicy.allow[]` entry and an install `capabilities.net[]`
/// [`NetRule`](forge_domain::NetRule) normalize to this so they compare
/// order-independently as set elements: method is upper-cased (the policy engine
/// matches case-insensitively), and the content-type / secret-header lists are
/// sorted so declaration order does not matter. A difference in ANY field — a
/// looser cap, an added/changed content type, or a newly allowed secret header —
/// makes two rules unequal, so the bind rejects it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NormalizedNetRule {
    method: String,
    url: String,
    max_response_bytes: Option<u64>,
    max_body_bytes: Option<u64>,
    timeout_ms: Option<u64>,
    request_content_types: Vec<String>,
    response_content_types: Vec<String>,
    allow_secret_headers: Vec<String>,
}

impl NormalizedNetRule {
    /// Normalize an install manifest [`NetRule`](forge_domain::NetRule).
    fn from_rule(rule: &forge_domain::NetRule) -> Self {
        let sorted = |v: &[String]| -> Vec<String> {
            let mut out: Vec<String> = v.to_vec();
            out.sort();
            out
        };
        NormalizedNetRule {
            method: rule.method.to_ascii_uppercase(),
            url: rule.url.clone(),
            max_response_bytes: rule.max_response_bytes,
            max_body_bytes: rule.max_body_bytes,
            timeout_ms: rule.timeout_ms,
            request_content_types: sorted(&rule.request_content_types),
            response_content_types: sorted(&rule.response_content_types),
            allow_secret_headers: sorted(&rule.allow_secret_headers),
        }
    }
}

/// The signed network egress allowlist (`networkPolicy.allow[]`) as a set of fully
/// normalized [`NormalizedNetRule`]s. Each signed entry is deserialized through
/// the SAME [`NetRule`](forge_domain::NetRule) type the install manifest uses, so
/// the signed and install sides normalize identically and a missing/extra
/// constraint is caught. A malformed allow entry is a typed rejection, never a
/// panic.
fn signed_net_rules(
    signed: &serde_json::Value,
) -> Result<std::collections::BTreeSet<NormalizedNetRule>> {
    let allow = match signed
        .get("networkPolicy")
        .and_then(|n| n.get("allow"))
        .and_then(serde_json::Value::as_array)
    {
        Some(a) => a,
        None => return Ok(std::collections::BTreeSet::new()),
    };
    let mut out = std::collections::BTreeSet::new();
    for entry in allow {
        let rule: forge_domain::NetRule = serde_json::from_value(entry.clone()).map_err(|e| {
            CoreError::ValidationError(format!(
                "signed package manifest networkPolicy.allow entry is malformed: {e}"
            ))
        })?;
        out.insert(NormalizedNetRule::from_rule(&rule));
    }
    Ok(out)
}

/// The full, normalized form of one filesystem grant — every CR-3 constraint the
/// runtime enforces (review 109 #1), not just the handle. Both a signed
/// `capabilities.files.{read,write}[]` entry and an install
/// [`FileRule`](forge_domain::FileRule) normalize to this so they compare
/// order-independently as set elements: the `content_types` list is sorted so
/// declaration order does not matter. A difference in ANY field — a wider glob, a
/// bigger `max_bytes`, or an added content type — makes two rules unequal, so the
/// bind rejects it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct NormalizedFileRule {
    handle: String,
    path_glob: String,
    max_bytes: Option<u64>,
    content_types: Vec<String>,
}

impl NormalizedFileRule {
    /// Normalize an install manifest [`FileRule`](forge_domain::FileRule).
    fn from_rule(rule: &forge_domain::FileRule) -> Self {
        let mut content_types = rule.content_types.clone();
        content_types.sort();
        NormalizedFileRule {
            handle: rule.handle.clone(),
            path_glob: rule.path_glob.clone(),
            max_bytes: rule.max_bytes,
            content_types,
        }
    }
}

/// The signed filesystem grant for `action` (`read`/`write`) as a set of fully
/// normalized [`NormalizedFileRule`]s. Each signed entry is deserialized through
/// the SAME [`FileRule`](forge_domain::FileRule) type the install manifest uses,
/// so the signed and install sides normalize identically and a missing/extra
/// constraint is caught. A malformed entry is a typed rejection, never a panic.
fn signed_file_rules(
    signed_caps: Option<&serde_json::Value>,
    action: &str,
) -> Result<std::collections::BTreeSet<NormalizedFileRule>> {
    let rules = match signed_caps
        .and_then(|c| c.get("files"))
        .and_then(|f| f.get(action))
        .and_then(serde_json::Value::as_array)
    {
        Some(r) => r,
        None => return Ok(std::collections::BTreeSet::new()),
    };
    let mut out = std::collections::BTreeSet::new();
    for entry in rules {
        let rule: forge_domain::FileRule = serde_json::from_value(entry.clone()).map_err(|e| {
            CoreError::ValidationError(format!(
                "signed package manifest capabilities.files.{action} entry is malformed: {e}"
            ))
        })?;
        out.insert(NormalizedFileRule::from_rule(&rule));
    }
    Ok(out)
}

/// The signed package's MP-8 compatibility floor
/// (`manifest.compatibility.{min_app_version, required_features}`) as a
/// forge-domain [`Compatibility`](forge_domain::Compatibility), so the signed and
/// install sides compare through the SAME type the negotiation gate uses (review
/// 170). An ABSENT signed `compatibility` is the default (no floor) — matching a
/// manifest that declares none — so a signed package with no compatibility binds
/// to an install with no compatibility. A malformed signed `compatibility` is a
/// typed rejection, never a panic.
fn signed_compatibility(signed: &serde_json::Value) -> Result<forge_domain::Compatibility> {
    match signed.get("compatibility") {
        None | Some(serde_json::Value::Null) => Ok(forge_domain::Compatibility::default()),
        Some(v) => serde_json::from_value(v.clone()).map_err(|e| {
            CoreError::ValidationError(format!(
                "signed package manifest `compatibility` is malformed: {e}"
            ))
        }),
    }
}

/// A stable, readable description of a [`Compatibility`](forge_domain::Compatibility)
/// for the bind rejection message: the `min_app_version` floor (if any) plus each
/// required feature as `id>=min`, so a stripped/weakened install floor names what
/// differs from the signed one.
fn describe_compatibility(compat: &forge_domain::Compatibility) -> String {
    let mut parts = Vec::new();
    if let Some(min_app) = &compat.min_app_version {
        parts.push(format!("min_app_version={min_app}"));
    }
    for req in &compat.required_features {
        parts.push(format!("{}>={}", req.feature_id, req.min_version));
    }
    if parts.is_empty() {
        "<none>".to_string()
    } else {
        parts.join(", ")
    }
}

/// A sorted, readable `Vec` view of a normalized files-rule set, for a stable
/// rejection message that surfaces the full rule (glob + cap + content types).
fn sorted_file_rules(set: &std::collections::BTreeSet<NormalizedFileRule>) -> Vec<String> {
    set.iter()
        .map(|r| {
            format!(
                "{} {} max_bytes<={:?} content_types={:?}",
                r.handle, r.path_glob, r.max_bytes, r.content_types,
            )
        })
        .collect()
}

/// A sorted `Vec` view of a `&str` set, for a stable, readable rejection message.
fn sorted_vec(set: &std::collections::BTreeSet<&str>) -> Vec<String> {
    set.iter().map(|s| s.to_string()).collect()
}

/// A sorted, readable `Vec` view of a normalized net-rule set, for a stable
/// rejection message that surfaces the full rule (caps + secret headers), not
/// just routing.
fn sorted_net_rules(set: &std::collections::BTreeSet<NormalizedNetRule>) -> Vec<String> {
    set.iter()
        .map(|r| {
            format!(
                "{} {} resp<={:?} body<={:?} timeout<={:?} req_ct={:?} resp_ct={:?} secret_hdrs={:?}",
                r.method,
                r.url,
                r.max_response_bytes,
                r.max_body_bytes,
                r.timeout_ms,
                r.request_content_types,
                r.response_content_types,
                r.allow_secret_headers,
            )
        })
        .collect()
}

/// Read an optional `manifest.<key>` string out of a signed package's manifest
/// (a [`serde_json::Value`]), for recording the verified publisher / key id. A
/// missing/non-string field yields `None` rather than erroring — by the time
/// this runs the package has already verified, so this is provenance reporting,
/// not validation.
fn manifest_string(manifest: &serde_json::Value, key: &str) -> Option<String> {
    manifest
        .get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Deserialize a required sub-field of the install `signature` object into `T`,
/// surfacing a `ValidationError` (never a panic) on a missing/malformed field.
fn signed_field<T: serde::de::DeserializeOwned>(
    signature: &serde_json::Value,
    field: &str,
) -> Result<T> {
    let value = signature.get(field).ok_or_else(|| {
        CoreError::ValidationError(format!(
            "applet.install `signature` requires a `{field}` field"
        ))
    })?;
    serde_json::from_value(value.clone()).map_err(|e| {
        CoreError::ValidationError(format!("applet.install `signature.{field}` is malformed: {e}"))
    })
}

/// Read a required string sub-field of the install `signature` object.
fn signed_str(signature: &serde_json::Value, field: &str) -> Result<String> {
    signature
        .get(field)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            CoreError::ValidationError(format!(
                "applet.install `signature.{field}` must be a string"
            ))
        })
}
