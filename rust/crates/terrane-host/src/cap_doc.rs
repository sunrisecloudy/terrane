use nanoserde::SerJson;
use terrane_api::{
    CapabilityCommandHelpInfo, CapabilityCommandInfo, CapabilityDocInfo, CapabilityEventInfo,
    CapabilityExampleInfo, CapabilityInternalInfo, CapabilityLimitInfo, CapabilityList,
    CapabilityManifestInfo, CapabilityParamInfo, CapabilityQueryInfo, CapabilityResourceInfo,
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
                queries: doc.manifest.queries,
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

pub fn capability_command_help_json(name: &str) -> Result<String, String> {
    capability_command_help(name).map(|help| help.serialize_json())
}

fn normalize_format(format: &str) -> &str {
    match format.trim() {
        "" => "json",
        other => other,
    }
}

fn capability_command_help(name: &str) -> Result<CapabilityCommandHelpInfo, String> {
    let (namespace, _) = name.split_once('.').ok_or_else(|| {
        format!("invalid command name '{name}': expected dotted capability command like app.add")
    })?;
    let doc = terrane_core::capability_doc(namespace, false).map_err(|e| e.to_string())?;
    let command = doc
        .commands
        .into_iter()
        .find(|command| command.name == name)
        .ok_or_else(|| format!("unknown capability command: {name}"))?;
    let argument_order = command
        .params
        .iter()
        .map(|param| param.name.clone())
        .collect();
    Ok(CapabilityCommandHelpInfo {
        name: command.name,
        summary: command.summary,
        argument_order,
        params: command.params.into_iter().map(param_info).collect(),
        returns: command.returns,
        errors: command.errors,
        emits: command.emits,
        effects: command.effects,
        examples: command.examples.into_iter().map(example_info).collect(),
        notes: vec![
            "Pass capability_command.args as a JSON array of strings in the documented order."
                .to_string(),
            "Examples show exact literal flag tokens such as --source when a command uses them."
                .to_string(),
            "help:true never dispatches, commits, appends events, or runs effects.".to_string(),
        ],
    })
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
        commands: doc
            .commands
            .into_iter()
            .map(|command| CapabilityCommandInfo {
                name: command.name,
                summary: command.summary,
                params: command.params.into_iter().map(param_info).collect(),
                returns: command.returns,
                errors: command.errors,
                emits: command.emits,
                effects: command.effects,
                examples: command.examples.into_iter().map(example_info).collect(),
            })
            .collect(),
        queries: doc
            .queries
            .into_iter()
            .map(|query| CapabilityQueryInfo {
                name: query.name,
                summary: query.summary,
                params: query.params.into_iter().map(param_info).collect(),
                returns: query.returns,
                errors: query.errors,
                examples: query.examples.into_iter().map(example_info).collect(),
            })
            .collect(),
        events: doc
            .events
            .into_iter()
            .map(|event| CapabilityEventInfo {
                kind: event.kind,
                summary: event.summary,
                params: event.params.into_iter().map(param_info).collect(),
                effects: event.effects,
                examples: event.examples.into_iter().map(example_info).collect(),
            })
            .collect(),
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
    render_command_docs(&mut out, doc);
    render_query_docs(&mut out, doc);
    render_event_docs(&mut out, doc);
    render_resource_docs(&mut out, doc, true);
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
    if !doc.compatibility.is_empty() {
        out.push_str("## Compatibility\n\n");
        for item in &doc.compatibility {
            out.push_str(&format!("- {item}\n"));
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
    out.push_str(&format!("- Version: {}\n", doc.version));
    if let Some(resource) = doc.resources.first() {
        out.push_str(&format!(
            "- App resource: `ctx.resource.{}`\n",
            resource.namespace
        ));
    }
    out.push('\n');
    render_command_docs(&mut out, doc);
    render_query_docs(&mut out, doc);
    render_event_docs(&mut out, doc);
    render_resource_docs(&mut out, doc, false);
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
    if !doc.compatibility.is_empty() {
        out.push_str("## Compatibility\n\n");
        for item in &doc.compatibility {
            out.push_str(&format!("- {item}\n"));
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

fn render_command_docs(out: &mut String, doc: &CapabilityDoc) {
    if doc.commands.is_empty() {
        return;
    }
    out.push_str("## Commands\n\n");
    out.push_str(
        "| Command | Params | Returns | Emits | Effects | Errors | Summary |\n\
         | --- | --- | --- | --- | --- | --- | --- |\n",
    );
    for command in &doc.commands {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} |\n",
            table_cell(&command.name),
            params_inline(&command.params),
            return_label(&command.returns),
            code_list(&command.emits, "none"),
            text_list(&command.effects, "none"),
            text_list(&command.errors, "none"),
            table_cell(&command.summary),
        ));
    }
    out.push('\n');
    for command in &doc.commands {
        if command.examples.is_empty() {
            continue;
        }
        out.push_str(&format!("### `{}` Examples\n\n", command.name));
        render_examples(out, &command.examples, 4);
    }
}

fn render_query_docs(out: &mut String, doc: &CapabilityDoc) {
    if doc.queries.is_empty() {
        return;
    }
    out.push_str("## Queries\n\n");
    out.push_str(
        "| Query | Params | Returns | Errors | Summary |\n\
         | --- | --- | --- | --- | --- |\n",
    );
    for query in &doc.queries {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            table_cell(&query.name),
            params_inline(&query.params),
            return_label(&query.returns),
            text_list(&query.errors, "none"),
            table_cell(&query.summary),
        ));
    }
    out.push('\n');
    for query in &doc.queries {
        if query.examples.is_empty() {
            continue;
        }
        out.push_str(&format!("### `{}` Examples\n\n", query.name));
        render_examples(out, &query.examples, 4);
    }
}

fn render_event_docs(out: &mut String, doc: &CapabilityDoc) {
    if doc.events.is_empty() {
        return;
    }
    out.push_str("## Events\n\n");
    out.push_str("| Event | Payload | Effects | Summary |\n| --- | --- | --- | --- |\n");
    for event in &doc.events {
        out.push_str(&format!(
            "| `{}` | {} | {} | {} |\n",
            table_cell(&event.kind),
            params_inline(&event.params),
            text_list(&event.effects, "none"),
            table_cell(&event.summary),
        ));
    }
    out.push('\n');
    for event in &doc.events {
        if event.examples.is_empty() {
            continue;
        }
        out.push_str(&format!("### `{}` Examples\n\n", event.kind));
        render_examples(out, &event.examples, 4);
    }
}

fn render_resource_docs(out: &mut String, doc: &CapabilityDoc, fully_qualified: bool) {
    if doc.resources.is_empty() {
        return;
    }
    out.push_str("## Resource Methods\n\n");
    out.push_str(
        "| Method | Kind | Params | Returns | Errors | Summary |\n\
         | --- | --- | --- | --- | --- | --- |\n",
    );
    for resource in &doc.resources {
        for method in &resource.methods {
            let method_name = if fully_qualified {
                format!("ctx.resource.{}.{}()", resource.namespace, method.name)
            } else {
                format!("{}()", method.name)
            };
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} |\n",
                table_cell(&method_name),
                table_cell(&method.kind),
                params_inline(&method.params),
                return_label(&method.returns),
                text_list(&method.errors, "none"),
                table_cell(&method.summary),
            ));
        }
    }
    out.push('\n');
}

fn params_inline(params: &[ParamDoc]) -> String {
    if params.is_empty() {
        return "none".to_string();
    }
    params
        .iter()
        .map(|param| {
            let mut out = format!("`{}`", table_cell(&param.name));
            if !param.required {
                out.push_str(" optional");
            }
            if !param.schema_ref.is_empty() {
                out.push_str(&format!(" ({})", table_cell(&param.schema_ref)));
            }
            if !param.summary.is_empty() {
                out.push_str(&format!(": {}", table_cell(&param.summary)));
            }
            out
        })
        .collect::<Vec<_>>()
        .join("<br>")
}

fn return_label(value: &str) -> String {
    if value.trim().is_empty() {
        "none".to_string()
    } else {
        table_cell(value)
    }
}

fn code_list(values: &[String], empty: &str) -> String {
    if values.is_empty() {
        return empty.to_string();
    }
    values
        .iter()
        .map(|value| format!("`{}`", table_cell(value)))
        .collect::<Vec<_>>()
        .join("<br>")
}

fn text_list(values: &[String], empty: &str) -> String {
    if values.is_empty() {
        return empty.to_string();
    }
    values
        .iter()
        .map(|value| table_cell(value))
        .collect::<Vec<_>>()
        .join("<br>")
}

fn render_examples(out: &mut String, examples: &[ExampleDoc], heading_level: usize) {
    let marker = "#".repeat(heading_level);
    for example in examples {
        out.push_str(&format!("{} {}\n\n", marker, example.title));
        if !example.summary.is_empty() {
            out.push_str(&format!("{}\n\n", example.summary));
        }
        out.push_str(&format!(
            "```{}\n{}\n```\n\n",
            example.language, example.code
        ));
        if !example.expected.is_empty() {
            out.push_str(&format!("Expected: {}\n\n", example.expected));
        }
    }
}

fn table_cell(value: &str) -> String {
    value.replace('\n', "<br>").replace('|', "\\|")
}
