//! MP-8 capability negotiation: the TRUSTED client feature registry + the
//! install-only-if-all-supported rule (`forge/spec/required-features.md`).
//!
//! prd-merged/08 **MP-8**: a marketplace package declares the
//! `required_features` it needs (a list of capability/runtime feature ids + min
//! versions, carried on `manifest.compatibility`); the installing CLIENT uses
//! capability negotiation to **refuse or run limited-mode gracefully**. This
//! module is the negotiation: it holds the set of features THIS client supports
//! and decides whether an install may proceed.
//!
//! The registry is TRUSTED workspace state, NOT request payload — exactly like
//! the SC-10 [`RunPolicy`](crate::RunPolicy) / the `db.read` grant table (review
//! 048/050): a package (or a shell) cannot widen what the client claims to
//! support by editing the command body. The registry is built from a fixed,
//! deterministic baseline ([`ClientFeatureRegistry::current`]) — the features
//! this build actually implements/enforces — and a host MAY extend it.
//!
//! ## The rule
//!
//! An install proceeds ONLY when the client supports EVERY required feature at a
//! version `>=` the declared `min_version`. A required feature the client does
//! not know, or knows only at a LOWER version, is **unsupported**; the negotiation
//! returns the ENUMERATED list of ALL unsupported features (id + required min +
//! what the client has), and the install is refused naming each one. An empty
//! `required_features` always proceeds.
//!
//! ## Signed-package composition (review 086 / 089)
//!
//! The signed-install path (`crate::workspace::signing`) fails CLOSED on any
//! UNKNOWN signed policy field this core cannot enforce. MP-8 is the matching
//! ACCEPT side: a signed FUTURE policy field is only admissible if the package
//! DECLARES it in `required_features` *and* this client supports that feature.
//! Both gates therefore agree on the same fact — "does this client support the
//! feature?" A signed future field that is NOT declared in `required_features`
//! is refused (the signed gate rejects the unknown field; the negotiation gate
//! never had a chance to admit it). A future field that IS declared but the
//! client does NOT support is refused by THIS gate, before the signed gate runs.
//! See `forge/spec/required-features.md` "Composition".

use std::collections::BTreeMap;

use forge_domain::{normalize_feature_id, version_at_least, Compatibility, FeatureRequirement};

/// The synthetic feature id MP-8 negotiation uses for the host app version, so a
/// package's `compatibility.min_app_version` is checked through the SAME
/// supported-at-version machinery as any other required feature. The client
/// registry advertises this id at the running app version.
pub const APP_FEATURE_ID: &str = "app";

/// The TRUSTED set of capability/runtime features THIS client supports, each at a
/// version (`feature_id -> supported_version`).
///
/// This is the SOURCE OF TRUTH the MP-8 install gate reads — never the request
/// payload (review 048/050). Build the deterministic baseline with
/// [`current`](Self::current); tests construct an explicit registry with
/// [`from_pairs`](Self::from_pairs) to model a specific client. Feature ids are
/// stored already-normalized ([`normalize_feature_id`]) so a lookup is a simple,
/// case-insensitive map hit.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ClientFeatureRegistry {
    /// `normalized feature id -> supported version`. Private so every id that
    /// enters is normalized through the constructors.
    supported: BTreeMap<String, String>,
}

impl ClientFeatureRegistry {
    /// The deterministic feature set THIS build supports (MP-8). Each entry is a
    /// feature the client can actually run/enforce, at the version it implements:
    ///
    ///   - the synthetic [`APP_FEATURE_ID`] at the running app version;
    ///   - the capability/runtime surfaces wired into the live spine
    ///     (`ctx.db.query`, `db.watch`, `ctx.net.fetch`, `ctx.files`,
    ///     `ctx.secrets`, signing) — the features a package can legitimately
    ///     require today;
    ///   - the signed-policy fields the unknown-field gate (review 086/089) CAN
    ///     enforce, advertised under the `signed.policy.*` namespace so a package
    ///     that declares a signed future field also declares the matching required
    ///     feature (see the module composition note).
    ///
    /// Kept deterministic (a fixed list, sorted by the map) so the install
    /// decision — and therefore the demo — replays identically.
    pub fn current() -> Self {
        Self::from_pairs([
            // Host app version (the `min_app_version` floor maps to this id).
            (APP_FEATURE_ID, APP_VERSION),
            // Live capability/runtime surfaces a package may require today.
            ("ctx.db.query", "1.0.0"),
            ("ctx.db.watch", "1.0.0"),
            ("ctx.net.fetch", "1.0.0"),
            ("ctx.files", "1.0.0"),
            ("ctx.secrets", "1.0.0"),
            ("ctx.ui", "1.0.0"),
            ("signing.ed25519", "1.0.0"),
            // Signed-policy fields this core can enforce. A signed package that
            // carries one of these must also declare it in required_features; the
            // negotiation admits it because the client supports it (review 086/089).
            ("signed.policy.network_policy", "1.0.0"),
            ("signed.policy.resource_budget", "1.0.0"),
            ("signed.policy.capabilities", "1.0.0"),
        ])
    }

    /// Build a registry from explicit `(feature_id, version)` pairs (tests model a
    /// specific client; a host extends the baseline). Each id is normalized on the
    /// way in, and a duplicate id keeps the LAST pair, so a caller can override a
    /// baseline entry by re-listing it.
    pub fn from_pairs<I, K, V>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (K, V)>,
        K: Into<String>,
        V: Into<String>,
    {
        let supported = pairs
            .into_iter()
            .map(|(id, v)| (normalize_feature_id(&id.into()), v.into()))
            .collect();
        Self { supported }
    }

    /// The version this client supports for `feature_id`, if any (after
    /// normalization). `None` ⇒ the client does not know the feature at all.
    pub fn supported_version(&self, feature_id: &str) -> Option<&str> {
        self.supported
            .get(&normalize_feature_id(feature_id))
            .map(String::as_str)
    }

    /// Whether this client supports `req` at or above its required `min_version`.
    /// A missing feature OR a supported version below the floor is unsupported.
    fn satisfies(&self, req: &FeatureRequirement) -> bool {
        match self.supported_version(&req.feature_id) {
            Some(have) => version_at_least(have, &req.min_version),
            None => false,
        }
    }

    /// Negotiate a package's `compatibility` against this client (MP-8).
    ///
    /// Returns `Ok(())` when the client supports EVERY required feature at
    /// `>= min_version` (and the `min_app_version` floor, modeled as the synthetic
    /// [`APP_FEATURE_ID`]); otherwise `Err(`[`UnsupportedFeatures`]`)` enumerating
    /// ALL unsupported requirements — never just the first — so the refusal can
    /// name every gap. An empty `required_features` with no `min_app_version`
    /// always succeeds.
    ///
    /// The check is order-independent and deterministic: requirements are folded in
    /// declaration order, and the synthetic app-version requirement (when present)
    /// is evaluated first so a too-old host is reported alongside any missing
    /// features.
    pub fn negotiate(&self, compat: &Compatibility) -> Result<(), UnsupportedFeatures> {
        let mut unsupported = Vec::new();

        // The `min_app_version` floor is negotiated as the synthetic `app` feature
        // so the host-version gap is enumerated in the SAME list as feature gaps.
        if let Some(min_app) = &compat.min_app_version {
            let req = FeatureRequirement {
                feature_id: APP_FEATURE_ID.to_string(),
                min_version: min_app.clone(),
            };
            if !self.satisfies(&req) {
                unsupported.push(Unsupported::of(&req, self));
            }
        }

        for req in &compat.required_features {
            if !self.satisfies(req) {
                unsupported.push(Unsupported::of(req, self));
            }
        }

        if unsupported.is_empty() {
            Ok(())
        } else {
            Err(UnsupportedFeatures { unsupported })
        }
    }
}

/// The running app version advertised under [`APP_FEATURE_ID`]. A fixed constant
/// (not read from the environment) so the negotiation — and the demo — is
/// deterministic and replays identically.
const APP_VERSION: &str = "1.0.0";

/// One required feature the client does not support, with enough context to name
/// it in a refusal: the required id + min version, and what the client has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unsupported {
    /// The required feature id, as the package declared it (un-normalized, so the
    /// refusal echoes the package's spelling).
    pub feature_id: String,
    /// The min version the package required.
    pub required_min_version: String,
    /// The version the client supports for this feature, or `None` when the client
    /// does not know the feature at all (distinguishes "too old" from "missing").
    pub client_has: Option<String>,
}

impl Unsupported {
    /// Build the [`Unsupported`] context for a failed requirement.
    fn of(req: &FeatureRequirement, registry: &ClientFeatureRegistry) -> Self {
        Unsupported {
            feature_id: req.feature_id.clone(),
            required_min_version: req.min_version.clone(),
            client_has: registry.supported_version(&req.feature_id).map(str::to_string),
        }
    }

    /// A stable one-line description for the enumerated refusal message, naming the
    /// required min and whether the client is missing the feature or too old.
    pub fn describe(&self) -> String {
        match &self.client_has {
            Some(have) => format!(
                "{} (required >= {}, client has {})",
                self.feature_id, self.required_min_version, have
            ),
            None => format!(
                "{} (required >= {}, client has none)",
                self.feature_id, self.required_min_version
            ),
        }
    }
}

/// The negotiation failure: the package requires features this client does not
/// support. Carries the FULL enumerated list (id + required min + client-has) so
/// the install refusal names EVERY gap, not just the first (the MP-8
/// "enumerated unsupported list" contract).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnsupportedFeatures {
    /// Every unsupported requirement, in the order encountered (app-version floor
    /// first, then declared features in declaration order).
    pub unsupported: Vec<Unsupported>,
}

impl UnsupportedFeatures {
    /// The feature ids that were unsupported, for a caller that only needs the
    /// names (e.g. an assertion or a structured response field).
    pub fn feature_ids(&self) -> Vec<String> {
        self.unsupported.iter().map(|u| u.feature_id.clone()).collect()
    }

    /// The enumerated, deterministic refusal message naming EVERY unsupported
    /// feature with its required min and what the client has. Joined with `; ` so a
    /// single `ValidationError` string carries the whole list.
    pub fn message(&self) -> String {
        let list = self
            .unsupported
            .iter()
            .map(Unsupported::describe)
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "package requires {} unsupported feature(s) this client cannot satisfy: {}",
            self.unsupported.len(),
            list
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req(id: &str, min: &str) -> FeatureRequirement {
        FeatureRequirement { feature_id: id.into(), min_version: min.into() }
    }

    fn compat(min_app: Option<&str>, reqs: &[(&str, &str)]) -> Compatibility {
        Compatibility {
            min_app_version: min_app.map(str::to_string),
            required_features: reqs.iter().map(|(id, m)| req(id, m)).collect(),
        }
    }

    #[test]
    fn empty_required_features_installs() {
        let reg = ClientFeatureRegistry::current();
        assert!(reg.negotiate(&Compatibility::default()).is_ok());
    }

    #[test]
    fn all_supported_installs() {
        let reg = ClientFeatureRegistry::current();
        let c = compat(None, &[("ctx.db.query", "1.0.0"), ("ctx.files", "1.0.0")]);
        assert!(reg.negotiate(&c).is_ok());
    }

    #[test]
    fn one_unsupported_refuses_naming_it() {
        let reg = ClientFeatureRegistry::current();
        let c = compat(None, &[("ctx.db.query", "1.0.0"), ("ctx.timetravel", "1.0.0")]);
        let err = reg.negotiate(&c).unwrap_err();
        assert_eq!(err.feature_ids(), vec!["ctx.timetravel".to_string()]);
        assert!(err.message().contains("ctx.timetravel"));
        assert!(err.message().contains("client has none"));
    }

    #[test]
    fn higher_required_min_version_refuses() {
        let reg = ClientFeatureRegistry::from_pairs([("ctx.db.query", "1.0.0")]);
        let c = compat(None, &[("ctx.db.query", "2.0.0")]);
        let err = reg.negotiate(&c).unwrap_err();
        assert_eq!(err.unsupported.len(), 1);
        assert_eq!(err.unsupported[0].client_has.as_deref(), Some("1.0.0"));
        assert!(err.message().contains("client has 1.0.0"));
    }

    #[test]
    fn multiple_unsupported_lists_all() {
        let reg = ClientFeatureRegistry::from_pairs([("ctx.db.query", "1.0.0")]);
        let c = compat(None, &[("ctx.db.query", "2.0.0"), ("ctx.future", "1.0.0")]);
        let err = reg.negotiate(&c).unwrap_err();
        // BOTH gaps are enumerated, not just the first.
        assert_eq!(err.feature_ids(), vec!["ctx.db.query".to_string(), "ctx.future".to_string()]);
    }

    #[test]
    fn feature_ids_are_case_normalized() {
        let reg = ClientFeatureRegistry::from_pairs([("CTX.DB.Query", "1.0.0")]);
        // The package spells it differently; normalization makes them the same.
        let c = compat(None, &[("ctx.db.query", "1.0.0")]);
        assert!(reg.negotiate(&c).is_ok());
    }

    #[test]
    fn forward_compat_superset_client_installs() {
        // The client supports a SUPERSET at higher versions than required.
        let reg = ClientFeatureRegistry::from_pairs([
            ("ctx.db.query", "2.5.0"),
            ("ctx.files", "1.4.0"),
            ("ctx.future", "3.0.0"),
        ]);
        let c = compat(None, &[("ctx.db.query", "2.0.0"), ("ctx.files", "1.0.0")]);
        assert!(reg.negotiate(&c).is_ok());
    }

    #[test]
    fn min_app_version_floor_is_negotiated_as_a_feature() {
        let reg = ClientFeatureRegistry::from_pairs([(APP_FEATURE_ID, "1.0.0")]);
        // A higher app floor than the client refuses, naming the `app` feature.
        let too_new = compat(Some("2.0.0"), &[]);
        let err = reg.negotiate(&too_new).unwrap_err();
        assert_eq!(err.feature_ids(), vec![APP_FEATURE_ID.to_string()]);
        // An app floor the client meets installs.
        assert!(reg.negotiate(&compat(Some("1.0.0"), &[])).is_ok());
    }

    #[test]
    fn signed_policy_feature_is_supported_by_default_client() {
        // The composition fact: a declared signed-policy feature the client can
        // enforce is supported, so the negotiation admits it (review 086/089).
        let reg = ClientFeatureRegistry::current();
        let c = compat(None, &[("signed.policy.network_policy", "1.0.0")]);
        assert!(reg.negotiate(&c).is_ok());
        // An UNDECLARED-but-unknown signed feature the client lacks is refused.
        let c2 = compat(None, &[("signed.policy.quantum_budget", "1.0.0")]);
        assert!(reg.negotiate(&c2).is_err());
    }
}
