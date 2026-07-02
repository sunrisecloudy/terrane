/// A command this capability owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandSpec {
    pub name: &'static str,
}

/// An event kind this capability emits/owns.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventSpec {
    pub kind: &'static str,
}

/// A read-only query this capability exposes to other capabilities.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuerySpec {
    pub name: &'static str,
}

/// An event kind this capability reacts to without owning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventPattern {
    pub kind: &'static str,
}

/// The declarative surface a capability exposes to the registry.
#[derive(Default)]
pub struct CapManifest {
    pub commands: Vec<CommandSpec>,
    pub events: Vec<EventSpec>,
    pub queries: Vec<QuerySpec>,
    pub resources: Vec<ResourceMethod>,
    pub grant_resources: Vec<GrantResourceSpec>,
    pub subscriptions: Vec<EventPattern>,
}

impl CapManifest {
    pub fn empty() -> Self {
        Self::default()
    }
}

pub const NAMESPACE_SELECTOR_SCHEMA_ID: &str = "namespace.v1";

pub const NAMESPACE_SELECTOR_SCHEMA_JSON: &str = r#"{"type":"object","required":["namespace"],"properties":{"namespace":{"type":"string"}},"additionalProperties":false}"#;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GrantResourceCompatibility {
    pub backward: bool,
    pub forward: bool,
}

impl GrantResourceCompatibility {
    pub const BACKWARD_AND_FORWARD: Self = Self {
        backward: true,
        forward: true,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnknownSelectorSchemaPolicy {
    Deny,
}

/// Capability-owned metadata that defines how auth grants target a resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrantResourceSpec {
    pub namespace: &'static str,
    pub selector_schema_id: &'static str,
    pub selector_schema_json: &'static str,
    pub verbs: &'static [&'static str],
    pub compatibility: GrantResourceCompatibility,
    pub unknown_selector_schema_policy: UnknownSelectorSchemaPolicy,
    pub summary: &'static str,
}

impl GrantResourceSpec {
    pub fn namespace_v1(
        namespace: &'static str,
        verbs: &'static [&'static str],
        summary: &'static str,
    ) -> Self {
        Self {
            namespace,
            selector_schema_id: NAMESPACE_SELECTOR_SCHEMA_ID,
            selector_schema_json: NAMESPACE_SELECTOR_SCHEMA_JSON,
            verbs,
            compatibility: GrantResourceCompatibility::BACKWARD_AND_FORWARD,
            unknown_selector_schema_policy: UnknownSelectorSchemaPolicy::Deny,
            summary,
        }
    }
}

/// One method a capability exposes on `ctx.resource.<namespace>`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceMethod {
    Write {
        name: &'static str,
        params: &'static [&'static str],
    },
    Read {
        name: &'static str,
        params: &'static [&'static str],
    },
    /// An effectful invocation that records events *and* returns a value
    /// (e.g. a local-model generation). Routed through decide like a write,
    /// with `Decision::Effect` allowed; the result comes from
    /// `Capability::resource_call_output`.
    Call {
        name: &'static str,
        params: &'static [&'static str],
    },
}

impl ResourceMethod {
    pub fn name(&self) -> &'static str {
        match self {
            ResourceMethod::Write { name, .. }
            | ResourceMethod::Read { name, .. }
            | ResourceMethod::Call { name, .. } => name,
        }
    }

    pub fn params(&self) -> &'static [&'static str] {
        match self {
            ResourceMethod::Write { params, .. }
            | ResourceMethod::Read { params, .. }
            | ResourceMethod::Call { params, .. } => params,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ResourceMethod::Write { .. } => "write",
            ResourceMethod::Read { .. } => "read",
            ResourceMethod::Call { .. } => "call",
        }
    }
}
