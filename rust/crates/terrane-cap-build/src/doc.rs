use terrane_cap_interface::{
    CapabilityDoc, CapabilityManifestDoc, ExampleDoc, InternalNote, LimitDoc, ParamDoc,
    ResourceDoc, ResourceMethodDoc,
};

use crate::resource_methods;

pub fn build_doc(include_internal: bool) -> CapabilityDoc {
    let methods = resource_method_docs();
    CapabilityDoc {
        namespace: "build".to_string(),
        title: "Build Helpers".to_string(),
        summary:
            "Pure in-sandbox JavaScript, TypeScript, JSX, and TSX compilation for Terrane backend code."
                .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: Vec::new(),
            queries: Vec::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
            resource_methods: methods.clone(),
        },
        commands: Vec::new(),
        queries: Vec::new(),
        events: Vec::new(),
        resources: vec![ResourceDoc {
            namespace: "build".to_string(),
            summary:
                "Backend resource surface installed as ctx.resource.build for compilation-only helper calls."
                    .to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Compile a TypeScript module from JS".to_string(),
            summary:
                "The compiler returns a JSON string instead of throwing for syntax/type transform errors."
                    .to_string(),
            language: "js".to_string(),
            code: include_str!("../examples/compile_ts.js").to_string(),
            expected: "Compiled JavaScript when ok is true, or the compile error string.".to_string(),
        }],
        constraints: vec![
            "compileTs is a read resource: it records no events and performs no filesystem or shell access."
                .to_string(),
            "The path argument is used for parser/loader selection and diagnostics; source is the complete module string."
                .to_string(),
            "The return value is always a JSON string with ok/code or ok/error fields.".to_string(),
        ],
        limits: vec![limit(
            "moduleInput",
            "single source string",
            "The current helper compiles one module at a time inside the runtime sandbox.",
        )],
        compatibility: vec![
            "Supported extensions follow terrane-app-build compile_script_source behavior, including JS, TS, JSX, and TSX."
                .to_string(),
            "Because the method is pure, replay never needs to fold build state.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Compiler owner".to_string(),
                body:
                    "terrane-cap-build delegates transformation to terrane-app-build and only exposes the sandboxed resource wrapper."
                        .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn resource_method_docs() -> Vec<ResourceMethodDoc> {
    resource_methods()
        .into_iter()
        .map(|method| match method.name() {
            "compileTs" => ResourceMethodDoc {
                name: "compileTs".to_string(),
                kind: method.kind().to_string(),
                params: vec![
                    param(
                        "path",
                        "Virtual filename used for loader selection and diagnostics.",
                        "path",
                    ),
                    param("source", "Complete JS/TS/JSX/TSX module source.", "string"),
                ],
                returns: "string".to_string(),
                summary:
                    "Compile one script module and return a JSON string containing either code or error."
                        .to_string(),
                errors: vec![
                    "Compiler failures are returned inside the JSON string as {ok:false,error}."
                        .to_string(),
                    "Unknown resource methods return invalid input errors.".to_string(),
                ],
            },
            other => unreachable!("unexpected build resource method: {other}"),
        })
        .collect()
}

fn param(name: &str, summary: &str, schema_ref: &str) -> ParamDoc {
    ParamDoc {
        name: name.to_string(),
        summary: summary.to_string(),
        required: true,
        schema_ref: schema_ref.to_string(),
    }
}

fn limit(name: &str, value: &str, reason: &str) -> LimitDoc {
    LimitDoc {
        name: name.to_string(),
        value: value.to_string(),
        reason: reason.to_string(),
    }
}
