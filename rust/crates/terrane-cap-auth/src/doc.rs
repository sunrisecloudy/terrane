use terrane_cap_interface::{
    command_doc, event_doc, limit, param, CapabilityDoc, CapabilityManifestDoc, ExampleDoc,
    InternalNote, ResourceDoc, SchemaDoc,
};

pub fn auth_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "auth".to_string(),
        title: "Authorization".to_string(),
        summary: "Durable local authorization facts folded from auth-owned events.".to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "host-implementer".to_string(),
            "admin-ui".to_string(),
            "agent".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["auth.grant".to_string(), "auth.revoke".to_string()],
            queries: Vec::new(),
            events: vec!["auth.granted".to_string(), "auth.revoked".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: Vec::new(),
        },
        commands: vec![
            command_doc(
                "auth.grant",
                &[
                    param("subject", "Subject id receiving the grant.", "subject_id"),
                    param("app", "App id the grant applies to.", "app_id"),
                    param("namespace", "Resource namespace being granted.", "resource_namespace"),
                    param("verbs", "Optional comma-separated descriptive verbs.", "csv"),
                ],
                "commit",
                "Record an idempotent namespace-level local resource grant.",
            )
            .with_errors(&["missing subject", "missing app", "missing namespace", "unknown app"])
            .with_emits(&["auth.granted"]),
            command_doc(
                "auth.revoke",
                &[
                    param("subject", "Subject id losing the grant.", "subject_id"),
                    param("app", "App id the grant applies to.", "app_id"),
                    param("namespace", "Resource namespace being revoked.", "resource_namespace"),
                ],
                "commit",
                "Record an idempotent namespace-level local resource revocation.",
            )
            .with_errors(&["missing subject", "missing app", "missing namespace", "unknown app"])
            .with_emits(&["auth.revoked"]),
        ],
        queries: Vec::new(),
        events: vec![
            event_doc(
                "auth.granted",
                &[
                    param("org", "Organization boundary.", "string"),
                    param("subject", "Subject id receiving the grant.", "subject_id"),
                    param("app", "App id.", "app_id"),
                    param("resource_id", "Canonical resource id.", "string"),
                ],
                "Durable fact that a subject may use a resource for an app.",
            ),
            event_doc(
                "auth.revoked",
                &[
                    param("org", "Organization boundary.", "string"),
                    param("subject", "Subject id losing the grant.", "subject_id"),
                    param("app", "App id.", "app_id"),
                    param("resource_id", "Canonical resource id.", "string"),
                ],
                "Durable fact that removes a previously folded grant.",
            ),
        ],
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Grant app KV access".to_string(),
            summary: "Grant the local owner namespace-level access to an app's kv resource."
                .to_string(),
            language: "cli".to_string(),
            code: "terrane auth grant user:local-owner calendar kv".to_string(),
            expected: "records auth.granted; runtime may install ctx.resource.kv for calendar"
                .to_string(),
        }],
        constraints: vec![
            "Auth owns auth.* events and folded AuthState.".to_string(),
            "Runtime authorization reads AuthState through a typed helper; public ctx.resource.kv never exposes auth data."
                .to_string(),
            "The v1 gate is namespace-level and does not enforce descriptive verbs.".to_string(),
            "Folding app.removed removes grants scoped to the removed app.".to_string(),
            "Authorization is checked only during live runtime resource installation, never during fold/replay."
                .to_string(),
        ],
        limits: vec![limit(
            "selectorSchema",
            "namespace.v1",
            "The first gate collapses resource grants to namespaces.",
        )],
        compatibility: vec![
            "namespace.v1 resource ids are the namespace itself, for example resources/kv."
                .to_string(),
            "Grant key path segments are percent-encoded to keep subject and resource ids unambiguous."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay invariant".to_string(),
                body: "fold applies auth.granted/auth.revoked and app.removed cleanup facts; it never re-runs authorization."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}
