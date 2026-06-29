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
    pub subscriptions: Vec<EventPattern>,
}

impl CapManifest {
    pub fn empty() -> Self {
        Self::default()
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
}

impl ResourceMethod {
    pub fn name(&self) -> &'static str {
        match self {
            ResourceMethod::Write { name, .. } | ResourceMethod::Read { name, .. } => name,
        }
    }

    pub fn params(&self) -> &'static [&'static str] {
        match self {
            ResourceMethod::Write { params, .. } | ResourceMethod::Read { params, .. } => params,
        }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            ResourceMethod::Write { .. } => "write",
            ResourceMethod::Read { .. } => "read",
        }
    }
}
