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
            commands: vec![
                "auth.member.ensure-local-owner".to_string(),
                "auth.grant".to_string(),
                "auth.revoke".to_string(),
                "auth.permission.request".to_string(),
                "auth.permission.approve".to_string(),
                "auth.permission.deny".to_string(),
                "auth.permission.cancel".to_string(),
                "auth.agent.register".to_string(),
                "auth.agent.delegate".to_string(),
                "auth.agent.revoke".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "auth.member.added".to_string(),
                "auth.granted".to_string(),
                "auth.revoked".to_string(),
                "auth.permission.requested".to_string(),
                "auth.permission.approved".to_string(),
                "auth.permission.denied".to_string(),
                "auth.permission.cancelled".to_string(),
                "auth.agent.registered".to_string(),
                "auth.agent.delegated".to_string(),
                "auth.agent.revoked".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: Vec::new(),
        },
        commands: vec![
            command_doc(
                "auth.member.ensure-local-owner",
                &[],
                "commit",
                "Ensure the local owner user has an active owner membership in the local org.",
            )
            .with_errors(&["unexpected argument"])
            .with_emits(&["auth.member.added"]),
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
            command_doc(
                "auth.permission.request",
                &[
                    param("request_id", "Stable permission request id.", "string"),
                    param("subject", "Subject needing access.", "subject_id"),
                    param("app", "App id the request applies to.", "app_id"),
                    param("operation", "Requested operation, such as __actions__ or list.", "string"),
                    param("source", "Host adapter that created the request.", "string"),
                    param("resources", "Comma-separated namespace.v1 resources.", "csv"),
                ],
                "commit",
                "Record an idempotent pending app permission request.",
            )
            .with_errors(&["missing request_id", "missing app", "unknown app"])
            .with_emits(&["auth.permission.requested"]),
            command_doc(
                "auth.permission.approve",
                &[
                    param("request_id", "Permission request id.", "string"),
                    param("reason", "Optional decision reason.", "string"),
                ],
                "commit",
                "Approve a pending request and emit normal auth.granted facts.",
            )
            .with_errors(&["unknown request", "request not pending"])
            .with_emits(&["auth.granted", "auth.permission.approved"]),
            command_doc(
                "auth.permission.deny",
                &[
                    param("request_id", "Permission request id.", "string"),
                    param("reason", "Optional decision reason.", "string"),
                ],
                "commit",
                "Deny a pending request without granting runtime access.",
            )
            .with_errors(&["unknown request", "request not pending"])
            .with_emits(&["auth.permission.denied"]),
            command_doc(
                "auth.permission.cancel",
                &[
                    param("request_id", "Permission request id.", "string"),
                    param("reason", "Optional cancellation reason.", "string"),
                ],
                "commit",
                "Cancel a pending request without granting runtime access.",
            )
            .with_errors(&["unknown request", "request not pending"])
            .with_emits(&["auth.permission.cancelled"]),
            command_doc(
                "auth.agent.register",
                &[
                    param("agent", "Agent subject id.", "subject_id"),
                    param("display_name", "Human-readable local label.", "string"),
                    param("owner_user", "Owning user subject.", "subject_id"),
                    param("max_role", "Delegation role ceiling.", "role"),
                    param("can_install_apps", "Whether the agent may install apps.", "bool"),
                    param(
                        "can_request_permissions",
                        "Whether the agent may create permission requests.",
                        "bool",
                    ),
                    param(
                        "can_grant_permissions",
                        "Whether the agent may grant permissions directly.",
                        "bool",
                    ),
                ],
                "commit",
                "Register an idempotent local AI-agent subject with delegated authority.",
            )
            .with_errors(&["missing agent", "unsafe agent subject"])
            .with_emits(&["auth.agent.registered"]),
            command_doc(
                "auth.agent.delegate",
                &[
                    param("agent", "Agent subject id.", "subject_id"),
                    param("max_role", "Delegation role ceiling.", "role"),
                    param("can_install_apps", "Whether the agent may install apps.", "bool"),
                    param(
                        "can_request_permissions",
                        "Whether the agent may create permission requests.",
                        "bool",
                    ),
                    param(
                        "can_grant_permissions",
                        "Whether the agent may grant permissions directly.",
                        "bool",
                    ),
                ],
                "commit",
                "Update an active agent delegation.",
            )
            .with_errors(&["unknown agent", "revoked agent"])
            .with_emits(&["auth.agent.delegated"]),
            command_doc(
                "auth.agent.revoke",
                &[param("agent", "Agent subject id.", "subject_id")],
                "commit",
                "Mark an agent delegation revoked without deleting its audit history.",
            )
            .with_errors(&["unsafe agent subject"])
            .with_emits(&["auth.agent.revoked"]),
        ],
        queries: Vec::new(),
        events: vec![
            event_doc(
                "auth.member.added",
                &[
                    param("org", "Organization boundary.", "string"),
                    param("subject", "Member subject id.", "subject_id"),
                    param("role", "Membership role.", "role"),
                ],
                "Durable organization membership fact.",
            ),
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
            event_doc(
                "auth.permission.requested",
                &[
                    param("request_id", "Stable request id.", "string"),
                    param("subject", "Subject needing access.", "subject_id"),
                    param("app", "App id.", "app_id"),
                ],
                "Durable pending permission request workflow fact.",
            ),
            event_doc(
                "auth.permission.approved",
                &[param("request_id", "Stable request id.", "string")],
                "Durable request approval fact; grants are emitted separately as auth.granted.",
            ),
            event_doc(
                "auth.permission.denied",
                &[param("request_id", "Stable request id.", "string")],
                "Durable request denial fact.",
            ),
            event_doc(
                "auth.permission.cancelled",
                &[param("request_id", "Stable request id.", "string")],
                "Durable request cancellation fact.",
            ),
            event_doc(
                "auth.agent.registered",
                &[
                    param("org", "Organization boundary.", "string"),
                    param("agent", "Agent subject id.", "subject_id"),
                    param("owner_user", "Owning user subject.", "subject_id"),
                    param("max_role", "Delegation role ceiling.", "role"),
                ],
                "Durable local AI-agent registration fact.",
            ),
            event_doc(
                "auth.agent.delegated",
                &[
                    param("org", "Organization boundary.", "string"),
                    param("agent", "Agent subject id.", "subject_id"),
                    param("max_role", "Delegation role ceiling.", "role"),
                ],
                "Durable update to an active AI-agent delegation.",
            ),
            event_doc(
                "auth.agent.revoked",
                &[
                    param("org", "Organization boundary.", "string"),
                    param("agent", "Agent subject id.", "subject_id"),
                ],
                "Durable AI-agent revocation fact.",
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
            "Auth projects folded facts to reserved internal storage under __terrane/auth using the selected KV backend family."
                .to_string(),
            "Runtime authorization reads AuthState through a typed helper; public ctx.resource.kv never exposes auth data."
                .to_string(),
            "The v1 gate is namespace-level and does not enforce descriptive verbs.".to_string(),
            "Folding app.removed removes grants scoped to the removed app.".to_string(),
            "Approving a permission request emits ordinary auth.granted facts; deny/cancel never grant access."
                .to_string(),
            "AI agents are separate subjects from their owner user and are governed by agent delegation plus resource grants."
                .to_string(),
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
            "Reserved projection keys use __terrane/auth/v1 and are derived from auth events; projection rows are not policy source."
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
