//! `applet.install` — compile + (optionally) verify-signature + store an applet
//! (CR-A2, CR-13/CR-14, SC-15). Moved verbatim from `workspace.rs` (/simplify
//! #11a): the [`InstalledApplet`] / [`InstallTrust`] record types plus the
//! applet-store KV helpers live next to the install handler that produces them.

use forge_domain::{CoreError, Manifest, Result};

use super::super::persistence::META_NS;
use super::super::signing::verify_install_signature;
use super::super::WorkspaceCore;
use super::{require_applet_id, take_field};

/// The compiled, installed form of an applet: its manifest plus the transpiled
/// JS the runtime executes and the canonical `code_hash` the pipeline produced.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(in crate::workspace) struct InstalledApplet {
    pub(in crate::workspace) manifest: Manifest,
    /// Transpiled ES-module JavaScript (the runtime's `Program.source`).
    pub(in crate::workspace) js_code: String,
    /// `forge_domain::code_hash(js_code)` — the provenance + replay key.
    pub(in crate::workspace) code_hash: String,
    /// Monotone install version WITHIN the current install generation (CR-7).
    /// A fresh install starts at `version = 1`; an upgrade bumps it. Uninstall +
    /// reinstall starts a new generation back at `version = 1` (see
    /// [`install_generation`](InstalledApplet::install_generation)).
    pub(in crate::workspace) version: u32,
    /// The install generation (CR-7 / `forge/spec/applet-lifecycle.md` Identity):
    /// `1` for the first install of an `applet_id`, incremented each time the
    /// applet is uninstalled and later installed fresh under the same id. The
    /// version counter resets to `1` at the start of each generation, so version
    /// identity is `(applet_id, install_generation, version, code_hash)`. Older
    /// records (installed before lifecycle wiring) deserialize to `1` via the
    /// serde default, so the field is backward-compatible with the meta store.
    #[serde(default = "default_install_generation")]
    pub(in crate::workspace) install_generation: u32,
    /// The signing/trust result recorded at install time (SC-15 / MP-4). An
    /// install that carried a verified Ed25519 package records the verified
    /// publisher + key id here so a later command can report the package's trust;
    /// an install with no signature records [`InstallTrust::Unsigned`]. Older
    /// records (installed before signing) deserialize to `Unsigned` via the serde
    /// default, so the field is backward-compatible with the existing meta store.
    #[serde(default)]
    pub(in crate::workspace) trust: InstallTrust,
}

/// The signing/trust provenance recorded for an installed applet (SC-15 / MP-4).
///
/// M0a is *signing-ready, not mandatory*: an install MAY carry an Ed25519-signed
/// package, in which case the platform VERIFIES it before trusting/installing and
/// records the [`Signed`](InstallTrust::Signed) result; an install with no
/// signature proceeds [`Unsigned`](InstallTrust::Unsigned). A failed verification
/// never lands here — the install is rejected before any record is written.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub(in crate::workspace) enum InstallTrust {
    /// No signature accompanied the install (the M0a default; allowed because
    /// signing is not yet mandatory). The install response surfaces `unsigned`.
    #[default]
    Unsigned,
    /// The install carried an Ed25519-signed package whose signature verified
    /// over the canonical `terrane/sig/v1` preimage and whose live files/manifest
    /// still match the signed hashes. Records the verified publisher identity so a
    /// later command can report the package's trust.
    Signed {
        /// The verified publisher id (`manifest.publisher` in the signed package),
        /// when the package declared one.
        publisher: Option<String>,
        /// The signing key id (`manifest.keyId`) the package was signed under.
        key_id: Option<String>,
        /// Whether the marketplace-policy trust layer (publisher trust set) was
        /// also enforced for this install (`true`) or skipped — the M0a default of
        /// crypto + integrity only (`false`).
        publisher_trust_enforced: bool,
    },
}

impl InstallTrust {
    /// A compact JSON view of the trust result for the install response + meta.
    /// `Unsigned` surfaces `{ "status": "unsigned" }`; `Signed` surfaces the
    /// verified publisher / key id so a shell can report the package's trust
    /// without re-reading the stored applet.
    pub(in crate::workspace) fn to_json(&self) -> serde_json::Value {
        match self {
            InstallTrust::Unsigned => serde_json::json!({ "status": "unsigned" }),
            InstallTrust::Signed {
                publisher,
                key_id,
                publisher_trust_enforced,
            } => serde_json::json!({
                "status": "signed",
                "publisher": publisher,
                "key_id": key_id,
                "publisher_trust_enforced": publisher_trust_enforced,
            }),
        }
    }
}

/// The backward-compat default install generation for a record persisted before
/// lifecycle wiring (`forge/spec/applet-lifecycle.md`): the first generation is
/// `1`.
fn default_install_generation() -> u32 {
    1
}

/// KV key for an applet's installed record within [`META_NS`].
pub(in crate::workspace) fn applet_key(applet_id: &str) -> String {
    format!("applet/{applet_id}")
}

/// Serialize an installed applet to the canonical JSON bytes persisted under
/// [`applet_key`]. Shared by [`store_applet`](WorkspaceCore::store_applet) and
/// its tx-scoped form so both write byte-identical records.
fn serialize_applet(installed: &InstalledApplet) -> Result<Vec<u8>> {
    serde_json::to_vec(installed)
        .map_err(|e| CoreError::StorageError(format!("applet serialize failed: {e}")))
}

/// KV key (within [`META_NS`]) for an applet's **highest install generation ever
/// assigned**. Unlike the active applet record (which is removed on uninstall),
/// this counter SURVIVES uninstall so a later fresh install starts a NEW
/// generation (`forge/spec/applet-lifecycle.md`: "Reinstalling after uninstall
/// creates a new `install_generation`"). The full key is
/// `applet_generation/<applet_id>`.
pub(in crate::workspace) fn applet_generation_key(applet_id: &str) -> String {
    format!("applet_generation/{applet_id}")
}

impl WorkspaceCore {
    /// `applet.install` — compile each source (static policy scan + SWC
    /// transpile; reject forbidden constructs), validate the manifest, and store
    /// the manifest + transpiled program (CR-A2, CR-13/CR-14, SC-15).
    ///
    /// Payload: `{ applet_id, manifest, sources: { "<path>": "<ts>" }, signature? }`.
    /// The manifest's `entrypoint` selects which source is the runnable program.
    ///
    /// SC-15 / MP-4 — package signing/trust (M0a: *signing-ready, not mandatory*):
    /// the install MAY carry an optional Ed25519-signed package under a
    /// `signature` field (the prd-merged/08 MP-4 package shape
    /// `{ package: { manifest, files, hashes }, signature, public_key,
    /// publisher_trust? }`, identical to the T012 fixtures). When present the
    /// platform VERIFIES it via [`forge_signing::verify_package`] BEFORE trusting
    /// or installing the applet:
    ///
    ///   - a CRYPTO / integrity / policy failure REJECTS the install with
    ///     `ValidationError("package signature invalid: ...")` — nothing is stored;
    ///   - the verified package is BOUND to the install payload (review 080 #1):
    ///     its files must be the same `path -> content` set as `sources`, so a
    ///     valid signature can only bless the exact code being compiled/stored;
    ///   - on success the verified publisher / key id + trust layer is recorded in
    ///     the install metadata ([`InstallTrust::Signed`]) so a later command can
    ///     report the package's trust.
    ///
    /// When NO `signature` is present the install proceeds [`InstallTrust::Unsigned`]
    /// (the M0a default) — the existing demo path is untouched and the response
    /// simply reports `unsigned`. The signature check runs BEFORE compilation so a
    /// tampered/untrusted package never reaches the transpiler or the store.
    pub(in crate::workspace) fn cmd_applet_install(
        &mut self,
        cmd: &forge_domain::CoreCommand,
    ) -> Result<serde_json::Value> {
        let applet_id = require_applet_id(cmd)?;
        let manifest: Manifest = take_field(cmd, "manifest")?;
        manifest.validate()?;

        let sources = cmd
            .payload
            .get("sources")
            .and_then(|v| v.as_object())
            .ok_or_else(|| {
                CoreError::ValidationError("applet.install requires a `sources` object".into())
            })?;
        if sources.is_empty() {
            return Err(CoreError::ValidationError(
                "applet.install `sources` must not be empty".into(),
            ));
        }

        // MP-8 capability negotiation (`forge/spec/required-features.md`,
        // prd-merged/08): BEFORE accepting an install, refuse it when the package's
        // `compatibility.required_features` (or `min_app_version`) names anything the
        // TRUSTED client feature registry does not support at the required min
        // version. The registry is read from trusted workspace state — never the
        // request payload (review 048/050) — so a package cannot widen what the
        // client claims to support. A refusal is a typed `ValidationError`
        // ENUMERATING every unsupported feature (id + required min + client-has), and
        // nothing is stored. An empty `required_features` proceeds. This composes
        // with the signed-install unknown-field fail-closed gate below (review
        // 086/089): a signed FUTURE policy field is only admissible if it is DECLARED
        // here AND the client supports it; an undeclared signed future field is
        // refused by the signature gate — the two gates agree.
        self.negotiate_required_features(&manifest)?;

        // SC-15 / MP-4: verify the package signature when one is carried, BEFORE
        // any state is touched, and BIND it to the actual install sources so a
        // valid signature can only bless the exact code being installed (review
        // 080 #1). The signed package's MANIFEST/policy is also bound to the
        // top-level `manifest` that is stored and enforced (review 082 #1 / 083):
        // a signed install must enforce the SIGNED capability boundary — the same
        // app id, every resource limit, the full net rule, and the entrypoint —
        // not a broader one. `Unsigned` when the install carries no signature.
        //
        // SC-12 live wiring (`forge/spec/audit-log.md`): a signed-install REFUSAL is a
        // security-relevant deny decision, so when verification rejects we persist a
        // durable `package.install.refused` audit row through THIS real install path
        // (before returning the error, before any state is touched) so a refused
        // package is queryable, not merely a transient validation error. We never fail
        // OPEN on an audit-persistence error — the refusal is the security signal — so
        // the original `Err` is always returned even if the durable append itself fails.
        let trust = match verify_install_signature(cmd, &applet_id, &manifest, sources) {
            Ok(trust) => trust,
            Err(error) => {
                let _ = self.persist_signed_install_refusal(cmd, &applet_id, &error);
                return Err(error);
            }
        };

        // Compile every source so a forbidden construct in ANY file rejects the
        // whole install (CR-13: the static policy scan is layer one). Capture
        // each compiled program; the entrypoint's program is the runnable one.
        let mut warnings = Vec::new();
        let mut entry_program: Option<forge_pipeline::Program> = None;
        for (path, src) in sources {
            let ts = src.as_str().ok_or_else(|| {
                CoreError::ValidationError(format!("source {path:?} must be a string"))
            })?;
            // compile() runs enforce_policy (PermissionDenied on eval/Function/…)
            // THEN transpiles; a forbidden construct never reaches transpile.
            let program = forge_pipeline::compile(ts)?;
            if path == &manifest.entrypoint {
                entry_program = Some(program);
            }
        }
        let entry_program = entry_program.ok_or_else(|| {
            CoreError::ValidationError(format!(
                "manifest.entrypoint {:?} is not among the provided sources",
                manifest.entrypoint
            ))
        })?;

        // Lifecycle install rules (CR-7 / `forge/spec/applet-lifecycle.md`):
        //
        //  - The FIRST install of an `applet_id` creates `install_generation = 1`,
        //    `version = 1`, durable state `enabled`.
        //  - Re-installing the SAME canonical `code_hash` over the ACTIVE version is
        //    an idempotent no-op: it returns the existing version/generation and
        //    mints NO new version (the `reinstall_same_code_hash_noop` vector).
        //  - Installing different code while an active version exists bumps the
        //    version WITHIN the current generation (the M0a install-as-upgrade path).
        //  - Installing after an uninstall starts a FRESH generation back at
        //    `version = 1` (the `uninstall_then_install_fresh_generation` vector):
        //    the active record was removed but the generation counter survives, so a
        //    reinstall is `generation = highest_ever + 1`.
        let active = self.load_applet(applet_id.as_str()).ok().flatten();
        if let Some(existing) = &active {
            // Idempotency requires BOTH the same `code_hash` AND the same canonical
            // manifest over the active version (spec line 39). A same-code reinstall
            // under a DIFFERENT manifest (e.g. tighter `limits`) is a real re-install
            // that bumps the version and switches the active manifest — it must NOT be
            // collapsed to a no-op (the version-pinned-replay regression test 7b).
            let same_manifest = existing.manifest == manifest;
            if existing.code_hash == entry_program.code_hash && same_manifest {
                // Idempotent reinstall of the active version: same code identity AND
                // same manifest, so nothing changes. Do NOT mint a new version, do NOT
                // touch lifecycle.
                self.events.emit(
                    Some(applet_id.clone()),
                    "applet.install.noop",
                    serde_json::json!({
                        "applet_id": applet_id,
                        "install_generation": existing.install_generation,
                        "version": existing.version,
                        "reason": "same_manifest_and_code_hash",
                    }),
                );
                return Ok(serde_json::json!({
                    "applet_id": applet_id,
                    "install_generation": existing.install_generation,
                    "version": existing.version,
                    "code_hash": existing.code_hash,
                    "lifecycle": "enabled",
                    "idempotent": true,
                    "warnings": warnings,
                    "trust": trust.to_json(),
                }));
            }
        }

        // Resolve `(install_generation, version)` for this install.
        let highest_generation = self.load_applet_generation(applet_id.as_str())?;
        let (install_generation, version) = match &active {
            // An active version exists with DIFFERENT code → same-generation version
            // bump (the M0a install-as-upgrade path; `applet.upgrade` is the explicit
            // staged variant).
            Some(existing) => (existing.install_generation, existing.version + 1),
            // No active version. A first install is generation 1; a reinstall after
            // uninstall is the next generation past the highest ever assigned. Either
            // way the new generation starts at `version = 1`.
            None => (highest_generation + 1, 1),
        };

        let installed = InstalledApplet {
            manifest,
            js_code: entry_program.js_code,
            code_hash: entry_program.code_hash,
            version,
            install_generation,
            trust: trust.clone(),
        };
        self.store_applet(applet_id.as_str(), &installed)?;
        // Record the highest generation ever assigned so a later uninstall +
        // reinstall mints a fresh one (the counter survives uninstall).
        self.store_applet_generation(applet_id.as_str(), install_generation)?;
        // A successful install is durably `enabled` (CR-7): clear any prior
        // suspended flag left by an earlier generation under the same id.
        self.set_applet_lifecycle(applet_id.as_str(), super::super::AppletLifecycle::Active)?;

        if sources.len() > 1 {
            warnings.push(format!(
                "{} non-entrypoint source(s) compiled but only the entrypoint is runnable in M0a",
                sources.len() - 1
            ));
        }

        self.events.emit(
            Some(applet_id.clone()),
            "applet.installed",
            serde_json::json!({
                "applet_id": applet_id,
                "install_generation": install_generation,
                "version": version,
                "state_after": "enabled",
                "trust": trust.to_json(),
            }),
        );

        Ok(serde_json::json!({
            "applet_id": applet_id,
            "install_generation": install_generation,
            "version": version,
            "code_hash": installed.code_hash,
            "lifecycle": "enabled",
            "warnings": warnings,
            // SC-15: the verified trust result for this install — `unsigned`, or
            // `signed` with the verified publisher / key id (the package passed
            // crypto + integrity, and the policy layer when enforced).
            "trust": trust.to_json(),
        }))
    }

    /// MP-8 capability negotiation: refuse an install whose manifest declares a
    /// `required_features` (or `min_app_version`) the TRUSTED client feature
    /// registry does not support (`forge/spec/required-features.md`, prd-merged/08).
    ///
    /// Reads the registry from trusted workspace state
    /// ([`client_feature_registry`](WorkspaceCore::client_feature_registry)) — never
    /// the request payload — and negotiates the manifest's
    /// [`Compatibility`](forge_domain::Compatibility). On any unsupported feature it
    /// returns a typed `ValidationError` whose message ENUMERATES every gap (id +
    /// required min + what the client has), so the caller refuses the install naming
    /// ALL of them; an empty `required_features` returns `Ok`. The error is the same
    /// `ValidationError` kind every other install-refusal uses, so a shell handles it
    /// uniformly.
    fn negotiate_required_features(&self, manifest: &Manifest) -> Result<()> {
        self.client_feature_registry()
            .negotiate(&manifest.compatibility)
            .map_err(|unsupported| {
                CoreError::ValidationError(format!(
                    "applet.install refused: {}",
                    unsupported.message()
                ))
            })
    }

    /// Append a `package.install.refused` audit row to the durable SC-12 log when an
    /// `applet.install` signature verification REJECTS (`forge/spec/audit-log.md`,
    /// the `audit-log-e2e` `signed_install_refusal_unknown_field` vector). Called from
    /// the live install path the instant [`verify_install_signature`] returns `Err`,
    /// so a refused signed package lands a queryable deny row, not merely a transient
    /// validation error.
    ///
    /// The row is `producer = signing`, `action = package.install.refused`,
    /// `decision = deny`, `resource_type = package`, `resource_id = <applet id>`. The
    /// `reason` is the verification error (the failing layer + reason); the `metadata`
    /// carries the install `command` plus whatever signature provenance the install
    /// payload's `signature` object named (`key_id`, `signed_at`) and the structured
    /// refusal context (`field`, `error_kind`). No secret value / body is present, so
    /// redaction is a no-op. `logical_time` is the EventSink clock so the durable row
    /// replays deterministically.
    fn persist_signed_install_refusal(
        &mut self,
        cmd: &forge_domain::CoreCommand,
        applet_id: &forge_domain::AppletId,
        error: &CoreError,
    ) -> Result<()> {
        let reason = match error {
            CoreError::ValidationError(msg) | CoreError::PermissionDenied(msg) => msg.clone(),
            other => other.to_string(),
        };
        // Provenance the install payload carried, if any — never the signed bytes or
        // any secret material, only the signer key id / timestamp + the structured
        // refusal context. Absent fields are simply omitted from the metadata. The
        // `signature.signature` field is the signature STRING, so the signer
        // provenance is carried alongside it in `signature.signature_meta`
        // (`{key_id, signed_at}`); the structured refusal context (`field`,
        // `error_kind`) is carried in `signature.refusal`.
        let sig = cmd.payload.get("signature");
        let sig_meta = sig.and_then(|s| s.get("signature_meta"));
        let refusal = sig.and_then(|s| s.get("refusal"));
        let mut metadata = serde_json::Map::new();
        metadata.insert("command".to_string(), serde_json::json!(cmd.name));
        let mut copy_str = |from: Option<&serde_json::Value>, key: &str| {
            if let Some(v) = from.and_then(|o| o.get(key)).and_then(|v| v.as_str()) {
                metadata.insert(key.to_string(), serde_json::json!(v));
            }
        };
        copy_str(sig_meta, "key_id");
        copy_str(sig_meta, "signed_at");
        copy_str(refusal, "field");
        copy_str(refusal, "error_kind");
        let _ = self.persist_producer_audit(
            "package.install.refused",
            serde_json::json!({
                "decision": "deny",
                "command": cmd.name,
                "package_id": applet_id.as_str(),
                "actor_id": cmd.actor.actor.as_str(),
                "reason": reason.clone(),
            }),
            "signing",
            "package.install.refused",
            "deny",
            cmd.actor.actor.as_str(),
            "package",
            Some(applet_id.as_str().to_string()),
            None,
            reason,
            serde_json::Value::Object(metadata),
        )?;
        Ok(())
    }

    /// Persist an installed applet (manifest + compiled program) in the reserved
    /// meta KV namespace. Delegates to [`store_applet_tx`](Self::store_applet_tx)
    /// inside a single transaction so the stand-alone write and the lifecycle
    /// commit (CR-7 `applet.upgrade`) share one SQL seam.
    pub(in crate::workspace) fn store_applet(
        &mut self,
        applet_id: &str,
        installed: &InstalledApplet,
    ) -> Result<()> {
        let bytes = serialize_applet(installed)?;
        self.store.transact(|tx| {
            forge_storage::kv_set_tx(tx, META_NS, &applet_key(applet_id), &bytes, "application/json")
        })
    }

    /// Persist an installed applet inside an OPEN transaction (the tx-scoped form
    /// of [`store_applet`](Self::store_applet)), so the active-pointer switch can
    /// commit atomically with the schema-registry persist + program pin in one
    /// `Store::transact` closure (CR-7 commit atomicity, lifecycle review P1).
    pub(in crate::workspace) fn store_applet_tx(
        tx: &forge_storage::Transaction<'_>,
        applet_id: &str,
        installed: &InstalledApplet,
    ) -> Result<()> {
        let bytes = serialize_applet(installed)?;
        forge_storage::kv_set_tx(tx, META_NS, &applet_key(applet_id), &bytes, "application/json")
    }

    /// Load an installed applet by id, if present.
    pub(in crate::workspace) fn load_applet(
        &self,
        applet_id: &str,
    ) -> Result<Option<InstalledApplet>> {
        match self.store.kv_get(META_NS, &applet_key(applet_id))? {
            Some(bytes) => {
                let installed = serde_json::from_slice(&bytes).map_err(|e| {
                    CoreError::StorageError(format!("applet deserialize failed: {e}"))
                })?;
                Ok(Some(installed))
            }
            None => Ok(None),
        }
    }

    /// Remove the ACTIVE installed applet record (CR-7 uninstall) inside an OPEN
    /// transaction: after this commits the applet is durably `uninstalled` (no active
    /// record), so `runtime.run` / `ui.dispatch_event` / `applet.enable` /
    /// `applet.suspend` reject for that id until a fresh install succeeds. The
    /// generation counter, run records, and pinned replay programs are NOT touched
    /// here — only the active pointer — so recorded runs remain replayable and a
    /// reinstall mints a fresh generation.
    ///
    /// This is the tx-scoped seam EVERY uninstall path uses, so a `purge_data`
    /// uninstall can tombstone owned records, switch the active pointer off, AND land
    /// its `applet.uninstalled` audit row in one atomic `Store::transact` closure
    /// (CR-7 + SC-12 §2, lifecycle review P2 / FIX ROUND 2): a crash mid-uninstall
    /// cannot leave some records purged with the applet still installed, nor the
    /// uninstall committed without its audit row.
    pub(in crate::workspace) fn delete_applet_tx(
        tx: &forge_storage::Transaction<'_>,
        applet_id: &str,
    ) -> Result<()> {
        forge_storage::kv_delete_tx(tx, META_NS, &applet_key(applet_id))
    }

    /// The highest install generation ever assigned to `applet_id` (0 when the id
    /// was never installed). Persisted separately from the active applet record so
    /// it SURVIVES uninstall and a reinstall starts the next generation
    /// (`forge/spec/applet-lifecycle.md`).
    pub(in crate::workspace) fn load_applet_generation(&self, applet_id: &str) -> Result<u32> {
        match self.store.kv_get(META_NS, &applet_generation_key(applet_id))? {
            Some(bytes) => serde_json::from_slice(&bytes).map_err(|e| {
                CoreError::StorageError(format!("applet generation deserialize failed: {e}"))
            }),
            None => Ok(0),
        }
    }

    /// Persist the highest install generation ever assigned to `applet_id`. Written
    /// on each install so a later uninstall + reinstall mints a strictly greater
    /// generation even though the active record was removed.
    pub(in crate::workspace) fn store_applet_generation(
        &mut self,
        applet_id: &str,
        generation: u32,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(&generation)
            .map_err(|e| CoreError::StorageError(format!("applet generation serialize failed: {e}")))?;
        self.store
            .kv_set(META_NS, &applet_generation_key(applet_id), &bytes, "application/json")
    }
}
