use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, CapabilityDoc, CapabilityManifestDoc,
    EventDoc, ExampleDoc, InternalNote, ResourceDoc, SchemaDoc,
};

pub fn share_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "share".to_string(),
        title: "Share Invites".to_string(),
        summary: "Records app share invites, accepted grants, and revocations for sync edge policy."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec!["host-implementer".to_string(), "owner".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "share.invite".to_string(),
                "share.redeem".to_string(),
                "share.revoke".to_string(),
            ],
            queries: vec!["share.list".to_string(), "share.invites".to_string()],
            events: vec![
                "share.invited".to_string(),
                "share.redeemed".to_string(),
                "share.revoked".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: Vec::new(),
        },
        commands: vec![
            command_doc(
                "share.invite",
                &[
                    param("app", "App id being shared.", "app_id"),
                    param("rights", "read or write; write implies read.", "read|write"),
                    param("note", "Optional out-of-band note.", "string"),
                ],
                "effect",
                "Validate a share invite and ask the edge to mint a one-time token; only the token hash is recorded.",
            )
            .with_emits(&["share.invited"])
            .with_errors(&["unknown app", "invalid rights", "note too long"]),
            command_doc(
                "share.redeem",
                &[
                    param("app", "App id being shared.", "app_id"),
                    param("token_hash", "SHA-256 lowercase hex hash of the out-of-band token.", "hex"),
                    param("grantee", "replica:<hex> or user/member subject.", "subject"),
                ],
                "commit",
                "Accept an open invite and record the grantee's app sync right.",
            )
            .with_emits(&["share.redeemed"])
            .with_errors(&["unknown invite", "already redeemed", "invalid grantee"]),
            command_doc(
                "share.revoke",
                &[
                    param("app", "App id being revoked.", "app_id"),
                    param("grantee", "replica:<hex> or user/member subject.", "subject"),
                ],
                "commit",
                "Stop future sync for a grantee. This cannot remove data already synced.",
            )
            .with_emits(&["share.revoked"])
            .with_errors(&["unknown app", "invalid grantee"]),
        ],
        queries: vec![
            query_doc(
                "share.list",
                &[param("app", "App id.", "app_id")],
                "json",
                "List folded shares for one app.",
            )
            .with_errors(&["missing app argument", "empty app id"]),
            query_doc(
                "share.invites",
                &[param("app", "App id.", "app_id")],
                "json",
                "List open invite hashes for one app; plaintext tokens never appear.",
            )
            .with_errors(&["missing app argument", "empty app id"]),
        ],
        events: events(),
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Invite a peer".to_string(),
            summary: "The owner shares an app with write sync rights.".to_string(),
            language: "cli".to_string(),
            code: "terrane share invite notes --rights write".to_string(),
            expected: "Prints the one-time token; the log records only share.invited with token_hash."
                .to_string(),
        }],
        constraints: vec![
            "Pairing without a share grants no app data by itself.".to_string(),
            "read serves outbound sync data and refuses inbound writes; write permits both directions.".to_string(),
            "Revocation stops future sync. It does not and cannot claw back data already synced; the grantee's home already has those events in its own log."
                .to_string(),
            "Tokens never appear in events, describe output, or queries; only SHA-256 lowercase hex hashes are recorded."
                .to_string(),
        ],
        limits: vec![
            limit("inviteTtl", "7 days", "Hosts should refuse expired invite tokens at redeem time."),
            limit("failedRedeems", "5", "Hosts should burn an invite after five failed redemptions."),
            limit("noteBytes", "512", "Invite notes are bounded and optional."),
        ],
        compatibility: vec![
            "Share state mirrors into auth.grant/auth.revoke at the host edge so existing permission tooling sees one permission surface."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "share folds only recorded facts. Sync routes enforce folded ShareState at the edge."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "share.invited",
            &[
                param("app", "Shared app id.", "app_id"),
                param("rights", "read or write.", "read|write"),
                param("token_hash", "SHA-256 lowercase hex invite token hash.", "hex"),
                param("note", "Optional note.", "string"),
            ],
            "Records an open invite without the plaintext token.",
        ),
        event_doc(
            "share.redeemed",
            &[
                param("app", "Shared app id.", "app_id"),
                param("token_hash", "Invite token hash.", "hex"),
                param("grantee", "Granted subject.", "subject"),
                param("rights", "read or write.", "read|write"),
            ],
            "Closes an invite and upserts the folded grantee share.",
        ),
        event_doc(
            "share.revoked",
            &[
                param("app", "Shared app id.", "app_id"),
                param("grantee", "Revoked subject.", "subject"),
            ],
            "Drops the folded share for future sync checks.",
        ),
    ]
}
