use nanoserde::SerJson;
use terrane_api::{
    CapabilityDocInfo, CapabilityExampleInfo, CapabilityInternalInfo, CapabilityLimitInfo,
    CapabilityList, CapabilityManifestInfo, CapabilityParamInfo, CapabilityResourceInfo,
    CapabilityResourceMethodInfo, CapabilitySchemaInfo, CapabilitySummary,
};
use terrane_core::{
    CapabilityDoc, ExampleDoc, InternalNote, LimitDoc, ParamDoc, ResourceDoc, ResourceMethodDoc,
    SchemaDoc,
};

pub fn capability_list(include_internal: bool) -> CapabilityList {
    CapabilityList {
        capabilities: terrane_core::capability_docs(include_internal)
            .into_iter()
            .map(|doc| CapabilitySummary {
                namespace: doc.namespace,
                title: doc.title,
                summary: doc.summary,
                status: doc.status,
                resources: doc
                    .resources
                    .iter()
                    .map(|resource| resource.namespace.clone())
                    .collect(),
                commands: doc.manifest.commands,
                events: doc.manifest.events,
            })
            .collect(),
    }
}

pub fn capability_info(
    namespace: &str,
    include_internal: bool,
) -> Result<CapabilityDocInfo, String> {
    terrane_core::capability_doc(namespace, include_internal)
        .map(capability_doc_info)
        .map_err(|e| e.to_string())
}

pub fn capability_list_json(include_internal: bool) -> String {
    capability_list(include_internal).serialize_json()
}

pub fn capability_list_markdown(include_internal: bool) -> String {
    let list = capability_list(include_internal);
    let mut out = String::from("| Namespace | Status | Summary |\n| --- | --- | --- |\n");
    for cap in list.capabilities {
        out.push_str(&format!(
            "| `{}` | {} | {} |\n",
            cap.namespace, cap.status, cap.summary
        ));
    }
    out.trim_end().to_string()
}

pub fn render_capability_info(
    namespace: &str,
    format: &str,
    include_internal: bool,
) -> Result<String, String> {
    let doc =
        terrane_core::capability_doc(namespace, include_internal).map_err(|e| e.to_string())?;
    match normalize_format(format) {
        "json" => Ok(capability_doc_info(doc).serialize_json()),
        "markdown" => Ok(render_markdown(&doc)),
        "skill" => Ok(render_skill(&doc)),
        other => Err(format!(
            "unknown capability info format: {other} (expected json, markdown, or skill)"
        )),
    }
}

fn normalize_format(format: &str) -> &str {
    match format.trim() {
        "" => "json",
        other => other,
    }
}

fn capability_doc_info(doc: CapabilityDoc) -> CapabilityDocInfo {
    CapabilityDocInfo {
        namespace: doc.namespace,
        title: doc.title,
        summary: doc.summary,
        status: doc.status,
        version: doc.version,
        audience: doc.audience,
        manifest: CapabilityManifestInfo {
            commands: doc.manifest.commands,
            queries: doc.manifest.queries,
            events: doc.manifest.events,
            subscriptions: doc.manifest.subscriptions,
            resource_methods: doc
                .manifest
                .resource_methods
                .into_iter()
                .map(resource_method_info)
                .collect(),
        },
        resources: doc.resources.into_iter().map(resource_info).collect(),
        schemas: doc.schemas.into_iter().map(schema_info).collect(),
        examples: doc.examples.into_iter().map(example_info).collect(),
        constraints: doc.constraints,
        limits: doc.limits.into_iter().map(limit_info).collect(),
        compatibility: doc.compatibility,
        internal: doc.internal.into_iter().map(internal_info).collect(),
    }
}

fn resource_info(resource: ResourceDoc) -> CapabilityResourceInfo {
    CapabilityResourceInfo {
        namespace: resource.namespace,
        summary: resource.summary,
        methods: resource
            .methods
            .into_iter()
            .map(resource_method_info)
            .collect(),
    }
}

fn resource_method_info(method: ResourceMethodDoc) -> CapabilityResourceMethodInfo {
    CapabilityResourceMethodInfo {
        name: method.name,
        kind: method.kind,
        params: method.params.into_iter().map(param_info).collect(),
        returns: method.returns,
        summary: method.summary,
        errors: method.errors,
    }
}

fn param_info(param: ParamDoc) -> CapabilityParamInfo {
    CapabilityParamInfo {
        name: param.name,
        summary: param.summary,
        required: param.required,
        schema_ref: param.schema_ref,
    }
}

fn schema_info(schema: SchemaDoc) -> CapabilitySchemaInfo {
    CapabilitySchemaInfo {
        id: schema.id,
        title: schema.title,
        media_type: schema.media_type,
        schema_json: schema.schema_json,
        public: schema.public,
    }
}

fn example_info(example: ExampleDoc) -> CapabilityExampleInfo {
    CapabilityExampleInfo {
        title: example.title,
        summary: example.summary,
        language: example.language,
        code: example.code,
        expected: example.expected,
    }
}

fn limit_info(limit: LimitDoc) -> CapabilityLimitInfo {
    CapabilityLimitInfo {
        name: limit.name,
        value: limit.value,
        reason: limit.reason,
    }
}

fn internal_info(note: InternalNote) -> CapabilityInternalInfo {
    CapabilityInternalInfo {
        title: note.title,
        body: note.body,
    }
}

fn render_markdown(doc: &CapabilityDoc) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", doc.title));
    out.push_str(&format!("{}\n\n", doc.summary));
    out.push_str(&format!(
        "- Namespace: `{}`\n- Status: {}\n- Version: {}\n\n",
        doc.namespace, doc.status, doc.version
    ));
    if !doc.resources.is_empty() {
        out.push_str("## Resource Methods\n\n");
        out.push_str("| Method | Kind | Summary |\n| --- | --- | --- |\n");
        for resource in &doc.resources {
            for method in &resource.methods {
                let params = method
                    .params
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!(
                    "| `ctx.resource.{}.{}({})` | {} | {} |\n",
                    resource.namespace, method.name, params, method.kind, method.summary
                ));
            }
        }
        out.push('\n');
    }
    if !doc.schemas.is_empty() {
        out.push_str("## Schemas\n\n");
        for schema in &doc.schemas {
            out.push_str(&format!("- `{}`: {}\n", schema.id, schema.title));
        }
        out.push('\n');
    }
    if !doc.examples.is_empty() {
        out.push_str("## Examples\n\n");
        for example in &doc.examples {
            out.push_str(&format!("### {}\n\n{}\n\n", example.title, example.summary));
            out.push_str(&format!(
                "```{}\n{}\n```\n\n",
                example.language, example.code
            ));
        }
    }
    if !doc.constraints.is_empty() {
        out.push_str("## Constraints\n\n");
        for constraint in &doc.constraints {
            out.push_str(&format!("- {constraint}\n"));
        }
        out.push('\n');
    }
    if !doc.limits.is_empty() {
        out.push_str("## Limits\n\n");
        for limit in &doc.limits {
            out.push_str(&format!(
                "- `{}`: {} ({})\n",
                limit.name, limit.value, limit.reason
            ));
        }
        out.push('\n');
    }
    if !doc.internal.is_empty() {
        out.push_str("## Internal Notes\n\n");
        for note in &doc.internal {
            out.push_str(&format!("### {}\n\n{}\n\n", note.title, note.body));
        }
    }
    out.trim_end().to_string()
}

fn render_skill(doc: &CapabilityDoc) -> String {
    let mut out = String::new();
    out.push_str(&format!("# {}\n\n", doc.namespace));
    out.push_str(&format!(
        "Use this skill when building Terrane apps or agents that need `{}` capability guidance.\n\n",
        doc.namespace
    ));
    out.push_str("## Contract\n\n");
    out.push_str(&format!("- Namespace: `{}`\n", doc.namespace));
    out.push_str(&format!("- Status: {}\n", doc.status));
    if let Some(resource) = doc.resources.first() {
        out.push_str(&format!(
            "- App resource: `ctx.resource.{}`\n",
            resource.namespace
        ));
    }
    out.push('\n');
    if !doc.resources.is_empty() {
        out.push_str("## Methods\n\n");
        out.push_str("| Method | Kind | Summary |\n| --- | --- | --- |\n");
        for resource in &doc.resources {
            for method in &resource.methods {
                let params = method
                    .params
                    .iter()
                    .map(|p| p.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push_str(&format!(
                    "| `{}({})` | {} | {} |\n",
                    method.name, params, method.kind, method.summary
                ));
            }
        }
        out.push('\n');
    }
    if !doc.schemas.is_empty() {
        out.push_str("## Schemas\n\n");
        for schema in &doc.schemas {
            out.push_str(&format!("- `schemas/{}`\n", schema.id));
        }
        out.push('\n');
    }
    if !doc.examples.is_empty() {
        out.push_str("## Examples\n\n");
        for example in &doc.examples {
            out.push_str(&format!("### {}\n\n", example.title));
            out.push_str(&format!(
                "```{}\n{}\n```\n\n",
                example.language, example.code
            ));
        }
    }
    if !doc.constraints.is_empty() {
        out.push_str("## Constraints\n\n");
        for constraint in &doc.constraints {
            out.push_str(&format!("- {constraint}\n"));
        }
    }
    out.trim_end().to_string()
}
