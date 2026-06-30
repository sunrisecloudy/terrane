//! Registry validation tests for grant resource specs (auth-plan doc 14).
//!
//! These cover arms of `validate_grant_resources` in `terrane-core/src/lib.rs`
//! that the committed `interface.rs` tests do not yet exercise. Per the project
//! rule, tests live in this file, never inline in the implementation.

use terrane_cap_interface::{
    CapManifest, Capability, CapabilityDoc, CapabilityManifestDoc, CommandCtx,
    GrantResourceCompatibility, GrantResourceSpec, ResourceMethod, StateStore,
    UnknownSelectorSchemaPolicy, NAMESPACE_SELECTOR_SCHEMA_ID, NAMESPACE_SELECTOR_SCHEMA_JSON,
};
use terrane_core::{Decision, Error, EventRecord, Registry, Result};

/// A capability whose resource methods and grant specs are fully caller-defined,
/// so each test can target exactly one validation arm.
struct SpecCap {
    namespace: &'static str,
    resources: Vec<ResourceMethod>,
    grant_resources: Vec<GrantResourceSpec>,
}

impl SpecCap {
    fn new(
        namespace: &'static str,
        resources: Vec<ResourceMethod>,
        grant_resources: Vec<GrantResourceSpec>,
    ) -> Self {
        Self {
            namespace,
            resources,
            grant_resources,
        }
    }
}

impl Capability for SpecCap {
    fn namespace(&self) -> &'static str {
        self.namespace
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: Vec::new(),
            events: Vec::new(),
            queries: Vec::new(),
            resources: self.resources.clone(),
            grant_resources: self.grant_resources.clone(),
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, _include_internal: bool) -> CapabilityDoc {
        minimal_doc(self.namespace)
    }

    fn decide(&self, _ctx: CommandCtx<'_>, name: &str, _args: &[String]) -> Result<Decision> {
        Err(Error::InvalidInput(format!("unknown command: {name}")))
    }

    fn fold(&self, _state: &mut dyn StateStore, _record: &EventRecord) -> Result<()> {
        Ok(())
    }
}

fn read(name: &'static str) -> ResourceMethod {
    ResourceMethod::Read { name, params: &[] }
}

fn write(name: &'static str) -> ResourceMethod {
    ResourceMethod::Write { name, params: &[] }
}

/// A grant spec with every field caller-controlled, for arm-specific tests.
fn spec(
    namespace: &'static str,
    selector_schema_id: &'static str,
    selector_schema_json: &'static str,
    verbs: &'static [&'static str],
    compatibility: GrantResourceCompatibility,
) -> GrantResourceSpec {
    GrantResourceSpec {
        namespace,
        selector_schema_id,
        selector_schema_json,
        verbs,
        compatibility,
        unknown_selector_schema_policy: UnknownSelectorSchemaPolicy::Deny,
        summary: "Test grant resource.",
    }
}

fn minimal_doc(namespace: &'static str) -> CapabilityDoc {
    CapabilityDoc {
        namespace: namespace.to_string(),
        title: namespace.to_string(),
        summary: format!("Test capability `{namespace}`."),
        status: "test".to_string(),
        version: "0.0.0".to_string(),
        audience: vec!["test".to_string()],
        manifest: CapabilityManifestDoc {
            commands: Vec::new(),
            queries: Vec::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: Vec::new(),
        queries: Vec::new(),
        events: Vec::new(),
        resources: Vec::new(),
        schemas: Vec::new(),
        examples: Vec::new(),
        constraints: Vec::new(),
        limits: Vec::new(),
        compatibility: Vec::new(),
        internal: Vec::new(),
    }
}

fn validate(cap: SpecCap) -> Result<()> {
    let mut registry = Registry::new();
    registry.try_register(Box::new(cap)).unwrap();
    registry.validate()
}

fn assert_invalid_contains(result: Result<()>, expected: &str) {
    match result {
        Err(Error::InvalidInput(message)) => assert!(
            message.contains(expected),
            "expected {message:?} to contain {expected:?}"
        ),
        other => panic!("expected InvalidInput containing {expected:?}, got {other:?}"),
    }
}

#[test]
fn rejects_grant_specs_without_resource_methods() {
    let cap = SpecCap::new(
        "ghost",
        Vec::new(),
        vec![GrantResourceSpec::namespace_v1("ghost", &["read"], "x")],
    );
    assert_invalid_contains(validate(cap), "without ctx.resource methods");
}

#[test]
fn rejects_spec_namespace_mismatch() {
    let cap = SpecCap::new(
        "owner",
        vec![read("get")],
        vec![GrantResourceSpec::namespace_v1("other", &["read"], "x")],
    );
    assert_invalid_contains(validate(cap), "declared by owner");
}

#[test]
fn rejects_duplicate_selector_schema_id() {
    let cap = SpecCap::new(
        "kvx",
        vec![read("get")],
        vec![
            GrantResourceSpec::namespace_v1("kvx", &["read"], "a"),
            GrantResourceSpec::namespace_v1("kvx", &["read"], "b"),
        ],
    );
    assert_invalid_contains(validate(cap), "duplicate grant selector schema");
}

#[test]
fn rejects_empty_verbs() {
    let cap = SpecCap::new(
        "kvx",
        vec![read("get")],
        vec![spec(
            "kvx",
            NAMESPACE_SELECTOR_SCHEMA_ID,
            NAMESPACE_SELECTOR_SCHEMA_JSON,
            &[],
            GrantResourceCompatibility::BACKWARD_AND_FORWARD,
        )],
    );
    assert_invalid_contains(validate(cap), "has no verbs");
}

#[test]
fn rejects_empty_selector_schema_json() {
    let cap = SpecCap::new(
        "kvx",
        vec![read("get")],
        vec![spec(
            "kvx",
            NAMESPACE_SELECTOR_SCHEMA_ID,
            "   ",
            &["read"],
            GrantResourceCompatibility::BACKWARD_AND_FORWARD,
        )],
    );
    assert_invalid_contains(validate(cap), "is empty");
}

#[test]
fn rejects_unsafe_selector_schema_id() {
    let cap = SpecCap::new(
        "kvx",
        vec![read("get")],
        vec![spec(
            "kvx",
            "bad schema/id",
            NAMESPACE_SELECTOR_SCHEMA_JSON,
            &["read"],
            GrantResourceCompatibility::BACKWARD_AND_FORWARD,
        )],
    );
    assert_invalid_contains(validate(cap), "is unsafe");
}

#[test]
fn rejects_non_compatible_spec() {
    let cap = SpecCap::new(
        "kvx",
        vec![read("get")],
        vec![spec(
            "kvx",
            NAMESPACE_SELECTOR_SCHEMA_ID,
            NAMESPACE_SELECTOR_SCHEMA_JSON,
            &["read"],
            GrantResourceCompatibility {
                backward: false,
                forward: true,
            },
        )],
    );
    assert_invalid_contains(validate(cap), "backward and forward compatible");
}

#[test]
fn rejects_resources_without_namespace_v1_spec() {
    let cap = SpecCap::new(
        "kvx",
        vec![read("get")],
        vec![spec(
            "kvx",
            "key-prefix.v1",
            NAMESPACE_SELECTOR_SCHEMA_JSON,
            &["read"],
            GrantResourceCompatibility::BACKWARD_AND_FORWARD,
        )],
    );
    assert_invalid_contains(validate(cap), "without a namespace.v1 grant spec");
}

#[test]
fn rejects_write_method_uncovered_by_verbs() {
    let cap = SpecCap::new(
        "kvx",
        vec![write("set")],
        vec![GrantResourceSpec::namespace_v1("kvx", &["read"], "x")],
    );
    assert_invalid_contains(validate(cap), "for write access");
}

#[test]
fn accepts_read_and_write_methods_with_covering_verbs() {
    let cap = SpecCap::new(
        "kvx",
        vec![read("get"), write("set")],
        vec![GrantResourceSpec::namespace_v1(
            "kvx",
            &["read", "write"],
            "x",
        )],
    );
    validate(cap).unwrap();
}
