use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};

pub fn applescript_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "applescript".to_string(),
        title: "Recorded AppleScript".to_string(),
        summary: "Run and compile-check AppleScript on macOS; results are recorded for replay."
            .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "applescript.run".to_string(),
                "applescript.check".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "applescript.ran".to_string(),
                "applescript.checked".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: resource_method_docs(),
        },
        commands: applescript_commands(),
        queries: Vec::new(),
        events: applescript_events(),
        resources: vec![ResourceDoc {
            namespace: "applescript".to_string(),
            summary: "Backend resource surface installed as ctx.resource.applescript for apps \
                      that declare the applescript resource. Grants arbitrary macOS machine \
                      control — default-deny."
                .to_string(),
            methods: resource_method_docs(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Compile-check a script".to_string(),
            summary: "Validate AppleScript syntax without executing it.".to_string(),
            language: "cli".to_string(),
            code: "terrane applescript check demo \"return 2 + 2\"".to_string(),
            expected: "records applescript.checked with ok=true when the script compiles"
                .to_string(),
        }],
        constraints: vec![
            "applescript.run and applescript.check validate the app and script before returning an edge effect."
                .to_string(),
            "Scripts are limited to 64 KiB; empty scripts are rejected in decide.".to_string(),
            "osascript execution happens only at the edge; replay folds recorded events without re-running scripts."
                .to_string(),
            "This capability grants arbitrary machine control on macOS — default-deny resource grants apply."
                .to_string(),
            "Folding app.removed drops that app's run history.".to_string(),
        ],
        limits: vec![
            limit("platform", "macOS", "Requires /usr/bin/osascript at the edge."),
            limit(
                "scriptSize",
                "64 KiB",
                "Larger scripts are rejected in decide.",
            ),
            limit(
                "history",
                "100 runs/app",
                "Older runs are truncated deterministically from the front.",
            ),
        ],
        compatibility: vec![
            "Non-macOS hosts return a typed runtime error from the edge runner; decide/fold stay portable."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "Effect::AppleScriptRun/Check are transient. applescript.ran/checked are the durable replay inputs."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn applescript_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "applescript.run",
            &[
                param("app", "Existing app id that owns the run.", "app_id"),
                param("script", "AppleScript source to execute.", "string"),
            ],
            "effect",
            "Validate and return the edge effect that runs the script via osascript.",
        )
        .with_errors(&["app not found", "empty script", "script too large"])
        .with_effects(&["AppleScriptRun"])
        .with_emits(&["applescript.ran"]),
        command_doc(
            "applescript.check",
            &[
                param("app", "Existing app id that owns the check.", "app_id"),
                param("script", "AppleScript source to compile-check.", "string"),
            ],
            "effect",
            "Validate and return the edge effect that compile-checks the script via osacompile.",
        )
        .with_errors(&["app not found", "empty script", "script too large"])
        .with_effects(&["AppleScriptCheck"])
        .with_emits(&["applescript.checked"]),
    ]
}

fn applescript_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "applescript.ran",
            &[
                param("app", "App id that requested the run.", "app_id"),
                param("script", "Full script source (audit).", "string"),
                param("ok", "Whether execution succeeded.", "bool"),
                param("output", "Trimmed stdout.", "string"),
                param("error", "Trimmed stderr / runtime errors.", "string"),
                param("exit_code", "Process exit code (-1 on timeout).", "i32"),
                param("duration_ms", "Wall-clock duration.", "u64"),
            ],
            "Records the observed AppleScript run for replay.",
        )
        .with_effects(&["stores AppleScriptState.runs[app]"]),
        event_doc(
            "applescript.checked",
            &[
                param("app", "App id that requested the check.", "app_id"),
                param("script", "Checked script source.", "string"),
                param("ok", "Whether osacompile succeeded.", "bool"),
                param("error", "Compile error text when ok=false.", "string"),
            ],
            "Audit-only compile check; does not fold into state.",
        ),
    ]
}

fn resource_method_docs() -> Vec<ResourceMethodDoc> {
    let with_returns = |mut method: ResourceMethodDoc, returns: &str| {
        method.returns = returns.to_string();
        method
    };
    vec![
        with_returns(
            resource_method(
                "run",
                "call",
                &[param("script", "AppleScript source to execute.", "string")],
                "Run AppleScript for the calling app; records applescript.ran at the edge.",
            ),
            "string (JSON {ok, output, error, exitCode, durationMs}) | null",
        ),
        with_returns(
            resource_method(
                "check",
                "call",
                &[param("script", "AppleScript source to compile-check.", "string")],
                "Compile-check AppleScript via osacompile; records applescript.checked.",
            ),
            "string (JSON {ok, error}) | null",
        ),
        with_returns(
            resource_method(
                "runs",
                "read",
                &[],
                "Read-only run history for the calling app.",
            ),
            "string (JSON array of run records)",
        ),
    ]
}