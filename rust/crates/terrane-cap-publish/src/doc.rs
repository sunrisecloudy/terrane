use terrane_cap_interface::{
    command_doc, event_doc, limit, param, CapabilityDoc, CapabilityManifestDoc, CommandDoc,
    EventDoc, ExampleDoc, InternalNote, ResourceDoc, SchemaDoc,
};

pub fn publish_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "publish".to_string(),
        title: "Signed App Publishing".to_string(),
        summary: "Records signed app bundle install provenance and trust-on-first-use publisher facts."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "host-implementer".to_string(),
            "agent".to_string(),
            "app-author".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["publish.install".to_string()],
            queries: Vec::new(),
            events: vec![
                "publish.identity-created".to_string(),
                "publish.trusted".to_string(),
                "publish.installed".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: Vec::new(),
        },
        commands: publish_commands(),
        queries: Vec::new(),
        events: publish_events(),
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Install a signed archive".to_string(),
            summary: "The host verifies the .terrane archive and records publisher provenance.".to_string(),
            language: "cli".to_string(),
            code: "terrane app install todo-1.2.0.terrane".to_string(),
            expected: "records publish.trusted on first use, app.added/app.upgraded and kv bundle file events, then publish.installed"
                .to_string(),
        }],
        constraints: vec![
            "Export is host-edge only and records no events because it changes no Terrane state."
                .to_string(),
            "publish.install returns Effect::InstallSignedBundle; signature verification, archive parsing, and TOFU prompting happen at the edge before any event is emitted."
                .to_string(),
            "Private publisher keys live in the connection secret store; public keys, trust, and provenance are the replayable facts."
                .to_string(),
            "Installing a bundle grants no app permissions; app resource grants still go through auth."
                .to_string(),
            "Replay folds publish events without re-reading an archive or verifying a signature."
                .to_string(),
        ],
        limits: vec![
            limit("archiveBytes", "16777216", "Maximum signed archive size in v1."),
            limit("filesPerArchive", "512", "Maximum bundle file count in v1."),
            limit("identitiesPerHome", "1", "A home has one publisher identity in v1."),
        ],
        compatibility: vec![
            "publish.installed extends app.import/app.upgrade batches without changing app.added or app.upgraded payloads."
                .to_string(),
            "Trust is scoped to a publisher public key, not to one app id.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "The edge records only verification outcomes. Private keys and archive bytes are never part of PublishState."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn publish_commands() -> Vec<CommandDoc> {
    vec![command_doc(
        "publish.install",
        &[param("archive_path", "Path or URL to a signed .terrane archive.", "string")],
        "effect",
        "Verify a signed app archive at the host edge, then install or upgrade the app with publisher provenance in the same recorded batch.",
    )
    .with_effects(&["InstallSignedBundle"])
    .with_emits(&["publish.trusted", "app.added", "app.upgraded", "kv.set", "kv.deleted", "publish.installed"])
    .with_errors(&[
        "archive is missing, too large, or has an unsupported formatVersion",
        "bundle hash or ed25519 signature verification fails",
        "publisher key differs from existing provenance for the same installed app id",
    ])]
}

fn publish_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "publish.identity-created",
            &[
                param("pubkey", "Publisher ed25519 verifying key as base64.", "base64"),
                param("replica_peer", "Home replica peer bound to the publisher key for display.", "string"),
            ],
            "Records this home's public publisher identity.",
        ),
        event_doc(
            "publish.trusted",
            &[
                param("pubkey", "Trusted publisher ed25519 verifying key as base64.", "base64"),
                param("label", "Human label from publish.json.", "string"),
            ],
            "Records trust-on-first-use for one publisher key.",
        ),
        event_doc(
            "publish.installed",
            &[
                param("app", "Installed app id.", "string"),
                param("version", "Installed app version.", "semver"),
                param("bundle_hash", "Canonical hash covering sorted bundle paths and file contents.", "sha256"),
                param("publisher_pubkey", "Publisher ed25519 verifying key as base64.", "base64"),
                param("publisher_label", "Human publisher label.", "string"),
            ],
            "Records signed install provenance for one app version.",
        ),
    ]
}
