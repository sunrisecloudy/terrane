use crate::manifest::{CapManifest, ResourceMethod};

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
    pub resources: Vec<ResourceDoc>,
    pub schemas: Vec<SchemaDoc>,
    pub examples: Vec<ExampleDoc>,
    pub constraints: Vec<String>,
    pub limits: Vec<LimitDoc>,
    pub compatibility: Vec<String>,
    pub internal: Vec<InternalNote>,
}

impl CapabilityDoc {
    pub fn from_manifest(namespace: &str, manifest: CapManifest, include_internal: bool) -> Self {
        let resource_methods: Vec<ResourceMethodDoc> = manifest
            .resources
            .iter()
            .map(ResourceMethodDoc::from_resource_method)
            .collect();
        let resources = if resource_methods.is_empty() {
            Vec::new()
        } else {
            vec![ResourceDoc {
                namespace: namespace.to_string(),
                summary: format!("Backend resource surface for `{namespace}`."),
                methods: resource_methods.clone(),
            }]
        };
        Self {
            namespace: namespace.to_string(),
            title: namespace.to_string(),
            summary: format!("Capability namespace `{namespace}`."),
            status: "stable".to_string(),
            version: "0.1.0".to_string(),
            audience: vec![
                "app-author".to_string(),
                "agent".to_string(),
                "host-implementer".to_string(),
            ],
            manifest: CapabilityManifestDoc {
                commands: manifest
                    .commands
                    .iter()
                    .map(|command| command.name.to_string())
                    .collect(),
                queries: manifest
                    .queries
                    .iter()
                    .map(|query| query.name.to_string())
                    .collect(),
                events: manifest
                    .events
                    .iter()
                    .map(|event| event.kind.to_string())
                    .collect(),
                subscriptions: manifest
                    .subscriptions
                    .iter()
                    .map(|subscription| subscription.kind.to_string())
                    .collect(),
                resource_methods,
            },
            resources,
            schemas: Vec::new(),
            examples: Vec::new(),
            constraints: Vec::new(),
            limits: Vec::new(),
            compatibility: Vec::new(),
            internal: if include_internal {
                vec![InternalNote {
                    title: "Generated from manifest".to_string(),
                    body: "This fallback doc was generated from Capability::manifest()."
                        .to_string(),
                }]
            } else {
                Vec::new()
            },
        }
    }

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

impl ResourceMethodDoc {
    fn from_resource_method(method: &ResourceMethod) -> Self {
        Self {
            name: method.name().to_string(),
            kind: method.kind().to_string(),
            params: method
                .params()
                .iter()
                .map(|name| ParamDoc {
                    name: (*name).to_string(),
                    summary: String::new(),
                    required: true,
                    schema_ref: String::new(),
                })
                .collect(),
            returns: String::new(),
            summary: format!("{} resource method `{}`.", method.kind(), method.name()),
            errors: Vec::new(),
        }
    }
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
