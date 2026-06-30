//! Recorded grant verbs must match each namespace's registered
//! `GrantResourceSpec`. This locks auth's `default_verbs_for_namespace`
//! (a hardcoded namespace→verbs map) against the real spec catalog, so a
//! read-only namespace like `build` can never silently record a spurious
//! `write` (review-013 PR1), and future spec/verb changes can't drift away
//! from what grants record. Own file; never inline in `src/`.

use tempfile::tempdir;
use terrane_core::{grant_resource_specs, Core, LOCAL_OWNER_SUBJECT};

use crate::helpers::req;

#[test]
fn default_grant_verbs_match_registered_specs() {
    let dir = tempdir().unwrap();
    let mut core = Core::open(dir.path().join("log.bin")).unwrap();
    core.dispatch(req("app.add", &["demo", "Demo"])).unwrap();

    let mut checked = 0;
    for spec in grant_resource_specs() {
        if spec.selector_schema_id != "namespace.v1" {
            continue;
        }
        let ns = spec.namespace;
        core.dispatch(req("auth.grant", &[LOCAL_OWNER_SUBJECT, "demo", ns]))
            .unwrap();
        let grant = core
            .state()
            .auth
            .grants
            .values()
            .find(|g| g.app == "demo" && g.namespace == ns)
            .unwrap_or_else(|| panic!("no recorded grant for namespace `{ns}`"));
        let spec_verbs: Vec<String> = spec.verbs.iter().map(|v| (*v).to_string()).collect();
        assert_eq!(
            grant.verbs, spec_verbs,
            "default grant verbs for `{ns}` must match its namespace.v1 spec verbs"
        );
        checked += 1;
    }
    assert!(checked > 0, "expected at least one namespace.v1 grant spec");
}
