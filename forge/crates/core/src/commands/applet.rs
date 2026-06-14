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
    /// Monotone install version (bumps on re-install/upgrade).
    pub(in crate::workspace) version: u32,
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

/// KV key for an applet's installed record within [`META_NS`].
pub(in crate::workspace) fn applet_key(applet_id: &str) -> String {
    format!("applet/{applet_id}")
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

        // SC-15 / MP-4: verify the package signature when one is carried, BEFORE
        // any state is touched, and BIND it to the actual install sources so a
        // valid signature can only bless the exact code being installed (review
        // 080 #1). The signed package's MANIFEST/policy is also bound to the
        // top-level `manifest` that is stored and enforced (review 082 #1 / 083):
        // a signed install must enforce the SIGNED capability boundary — the same
        // app id, every resource limit, the full net rule, and the entrypoint —
        // not a broader one. `Unsigned` when the install carries no signature.
        let trust = verify_install_signature(cmd, &applet_id, &manifest, sources)?;

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

        // Bump version if re-installing.
        let version = self
            .load_applet(applet_id.as_str())
            .ok()
            .flatten()
            .map(|a| a.version + 1)
            .unwrap_or(1);

        let installed = InstalledApplet {
            manifest,
            js_code: entry_program.js_code,
            code_hash: entry_program.code_hash,
            version,
            trust: trust.clone(),
        };
        self.store_applet(applet_id.as_str(), &installed)?;

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
                "version": version,
                "trust": trust.to_json(),
            }),
        );

        Ok(serde_json::json!({
            "applet_id": applet_id,
            "version": version,
            "code_hash": installed.code_hash,
            "warnings": warnings,
            // SC-15: the verified trust result for this install — `unsigned`, or
            // `signed` with the verified publisher / key id (the package passed
            // crypto + integrity, and the policy layer when enforced).
            "trust": trust.to_json(),
        }))
    }

    /// Persist an installed applet (manifest + compiled program) in the reserved
    /// meta KV namespace.
    pub(in crate::workspace) fn store_applet(
        &mut self,
        applet_id: &str,
        installed: &InstalledApplet,
    ) -> Result<()> {
        let bytes = serde_json::to_vec(installed)
            .map_err(|e| CoreError::StorageError(format!("applet serialize failed: {e}")))?;
        self.store
            .kv_set(META_NS, &applet_key(applet_id), &bytes, "application/json")
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
}
