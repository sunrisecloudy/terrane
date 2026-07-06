use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, resource_method, CapabilityDoc,
    CapabilityManifestDoc, ResourceDoc, ResourceMethodDoc,
    ExampleDoc,
};

pub fn interop_doc(_include_internal: bool) -> CapabilityDoc {
    let with_returns = |mut method: ResourceMethodDoc, returns: &str| {
        method.returns = returns.to_string();
        method
    };
    let resources = vec![
        with_returns(resource_method(
            "call",
            "call",
            &[
                param("target", "Target app id.", "app_id"),
                param("verb", "Target backend verb.", "string"),
                param("args", "String arguments passed to the target.", "string[]"),
            ],
            "Call a target app verb and return the recorded reply.",
        ), "string reply or blob reference JSON"),
        with_returns(resource_method(
            "send",
            "call",
            &[
                param("interface", "Interface to route through.", "string"),
                param("kind", "common.receive kind hint.", "string"),
                param("payloadJson", "JSON payload string.", "json"),
            ],
            "Deliver a payload to the picked default target's common.receive; raises the picker if no target is chosen.",
        ), "string reply from common.receive"),
        with_returns(resource_method(
            "pick",
            "call",
            &[param("interface", "Interface to pick a default target for.", "string")],
            "Raise the powerbox picker for an interface; the user's choice is recorded as a scoped grant.",
        ), "grant status string"),
    ];
    CapabilityDoc {
        namespace: "interop".to_string(),
        title: "App Interop".to_string(),
        summary: "Recorded host-mediated app-to-app backend calls over the normal verb surface."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["interop.call".to_string(), "interop.pick".to_string()],
            queries: vec!["interop.apps".to_string()],
            events: vec!["interop.called".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: resources.clone(),
        },
        commands: vec![
            command_doc(
                "interop.call",
                &[
                    param("caller", "Calling app id.", "app_id"),
                    param("target", "Target app id.", "app_id"),
                    param("verb", "Target backend verb.", "string"),
                ],
                "effect",
                "Run a granted target app verb and record the reply.",
            )
            .with_errors(&["permission required", "InteropCycle", "InteropDepthExceeded"])
            .with_effects(&["AppCall"])
            .with_emits(&["interop.called"]),
            command_doc(
                "interop.pick",
                &[
                    param("caller", "Calling app id.", "app_id"),
                    param("interface", "Interface name.", "string"),
                    param("target", "Chosen target app id.", "app_id"),
                ],
                "commit",
                "Record a chosen target as a scoped interop default; a bare two-arg pick raises the picker instead.",
            )
            .with_errors(&[
                "interop_pick_required",
                "app not found",
                "target does not declare interface",
            ])
            .with_emits(&["auth.granted"]),
        ],
        queries: vec![query_doc(
            "interop.apps",
            &[param("interface", "Interface name.", "string")],
            "JSON array",
            "List apps declaring an interface.",
        )
        .with_errors(&["missing interface"])],
        events: vec![event_doc(
            "interop.called",
            &[
                param("caller", "Calling app id.", "app_id"),
                param("target", "Target app id.", "app_id"),
                param("verb", "Target backend verb.", "string"),
            ],
            "Recorded reply for one app-to-app call.",
        )],
        resources: vec![ResourceDoc {
            namespace: "interop".to_string(),
            summary: "Recorded app-to-app calls for app backends.".to_string(),
            methods: resources,
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Call another app through a declared interface".to_string(),
            summary: "A backend can call a granted target app verb through the host-mediated interop resource."
                .to_string(),
            language: "js".to_string(),
            code: "export const actions = {\n  async readSharedItem(id, ctx) {\n    return await ctx.resource.interop.call('notes', 'common.get', id);\n  }\n};"
                .to_string(),
            expected: "The reply is recorded as interop.called so replay returns the same value without rerunning the target."
                .to_string(),
        }],
        constraints: vec![
            "The target runs under its own manifest resource scope.".to_string(),
            "Internal __-prefixed verbs are rejected.".to_string(),
            "Replay folds interop.called instead of rerunning the target.".to_string(),
        ],
        limits: vec![
            limit("args", "64 KiB", "Maximum serialized argument bytes per call."),
            limit("depth", "4", "Maximum interop chain depth."),
            limit(
                "reply",
                "256 KiB inline / 8 MiB blob",
                "Large replies are stored in the blob CAS and referenced by hash.",
            ),
            limit(
                "calls",
                "100",
                "Recorded interop calls per backend run.",
            ),
        ],
        compatibility: Vec::new(),
        internal: Vec::new(),
    }
}
