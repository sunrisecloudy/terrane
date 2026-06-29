/// Canonical capability documentation. Edge surfaces render this into MCP
/// detail, CLI help, generated skills, and public contract docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityDoc {
    pub namespace: String,
    pub title: String,
    pub summary: String,
    pub status: String,
    pub version: String,
    pub audience: Vec<String>,
    pub manifest: CapabilityManifestDoc,
    pub commands: Vec<CommandDoc>,
    pub queries: Vec<QueryDoc>,
    pub events: Vec<EventDoc>,
    pub resources: Vec<ResourceDoc>,
    pub schemas: Vec<SchemaDoc>,
    pub examples: Vec<ExampleDoc>,
    pub constraints: Vec<String>,
    pub limits: Vec<LimitDoc>,
    pub compatibility: Vec<String>,
    pub internal: Vec<InternalNote>,
}

impl CapabilityDoc {
    pub fn without_internal(mut self) -> Self {
        self.internal.clear();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapabilityManifestDoc {
    pub commands: Vec<String>,
    pub queries: Vec<String>,
    pub events: Vec<String>,
    pub subscriptions: Vec<String>,
    pub resource_methods: Vec<ResourceMethodDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandDoc {
    pub name: String,
    pub summary: String,
    pub params: Vec<ParamDoc>,
    pub returns: String,
    pub errors: Vec<String>,
    pub emits: Vec<String>,
    pub effects: Vec<String>,
    pub examples: Vec<ExampleDoc>,
}

impl CommandDoc {
    pub fn with_errors(mut self, errors: &[&str]) -> Self {
        self.errors = strings(errors);
        self
    }

    pub fn with_emits(mut self, emits: &[&str]) -> Self {
        self.emits = strings(emits);
        self
    }

    pub fn with_effects(mut self, effects: &[&str]) -> Self {
        self.effects = strings(effects);
        self
    }

    pub fn with_examples(mut self, examples: &[ExampleDoc]) -> Self {
        self.examples = examples.to_vec();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryDoc {
    pub name: String,
    pub summary: String,
    pub params: Vec<ParamDoc>,
    pub returns: String,
    pub errors: Vec<String>,
    pub examples: Vec<ExampleDoc>,
}

impl QueryDoc {
    pub fn with_errors(mut self, errors: &[&str]) -> Self {
        self.errors = strings(errors);
        self
    }

    pub fn with_examples(mut self, examples: &[ExampleDoc]) -> Self {
        self.examples = examples.to_vec();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventDoc {
    pub kind: String,
    pub summary: String,
    pub params: Vec<ParamDoc>,
    pub effects: Vec<String>,
    pub examples: Vec<ExampleDoc>,
}

impl EventDoc {
    pub fn with_effects(mut self, effects: &[&str]) -> Self {
        self.effects = strings(effects);
        self
    }

    pub fn with_examples(mut self, examples: &[ExampleDoc]) -> Self {
        self.examples = examples.to_vec();
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceDoc {
    pub namespace: String,
    pub summary: String,
    pub methods: Vec<ResourceMethodDoc>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResourceMethodDoc {
    pub name: String,
    pub kind: String,
    pub params: Vec<ParamDoc>,
    pub returns: String,
    pub summary: String,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamDoc {
    pub name: String,
    pub summary: String,
    pub required: bool,
    pub schema_ref: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SchemaDoc {
    pub id: String,
    pub title: String,
    pub media_type: String,
    pub schema_json: String,
    pub public: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExampleDoc {
    pub title: String,
    pub summary: String,
    pub language: String,
    pub code: String,
    pub expected: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LimitDoc {
    pub name: String,
    pub value: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InternalNote {
    pub title: String,
    pub body: String,
}

pub fn command_doc(name: &str, params: &[ParamDoc], returns: &str, summary: &str) -> CommandDoc {
    CommandDoc {
        name: name.to_string(),
        summary: summary.to_string(),
        params: params.to_vec(),
        returns: returns.to_string(),
        errors: Vec::new(),
        emits: Vec::new(),
        effects: Vec::new(),
        examples: Vec::new(),
    }
}

pub fn query_doc(name: &str, params: &[ParamDoc], returns: &str, summary: &str) -> QueryDoc {
    QueryDoc {
        name: name.to_string(),
        summary: summary.to_string(),
        params: params.to_vec(),
        returns: returns.to_string(),
        errors: Vec::new(),
        examples: Vec::new(),
    }
}

pub fn event_doc(kind: &str, params: &[ParamDoc], summary: &str) -> EventDoc {
    EventDoc {
        kind: kind.to_string(),
        summary: summary.to_string(),
        params: params.to_vec(),
        effects: Vec::new(),
        examples: Vec::new(),
    }
}

pub fn resource_method(
    name: &str,
    kind: &str,
    params: &[ParamDoc],
    summary: &str,
) -> ResourceMethodDoc {
    ResourceMethodDoc {
        name: name.to_string(),
        kind: kind.to_string(),
        params: params.to_vec(),
        returns: String::new(),
        summary: summary.to_string(),
        errors: vec![
            "invalid input".to_string(),
            "unknown resource".to_string(),
            "unsupported operation".to_string(),
        ],
    }
}

pub fn param(name: &str, summary: &str, schema_ref: &str) -> ParamDoc {
    ParamDoc {
        name: name.to_string(),
        summary: summary.to_string(),
        required: true,
        schema_ref: schema_ref.to_string(),
    }
}

pub fn limit(name: &str, value: &str, reason: &str) -> LimitDoc {
    LimitDoc {
        name: name.to_string(),
        value: value.to_string(),
        reason: reason.to_string(),
    }
}

pub fn schema(id: &str, title: &str, schema_json: &str) -> SchemaDoc {
    SchemaDoc {
        id: id.to_string(),
        title: title.to_string(),
        media_type: "application/schema+json".to_string(),
        schema_json: schema_json.to_string(),
        public: true,
    }
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_string()).collect()
}
