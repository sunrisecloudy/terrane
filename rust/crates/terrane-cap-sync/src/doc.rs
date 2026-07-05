use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, QueryDoc, ResourceDoc, SchemaDoc,
};

pub fn sync_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "sync".to_string(),
        title: "Sync Session Facts".to_string(),
        summary: "Records paired peers and accepted foreign-event cursors for host-driven sync."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec!["host-implementer".to_string(), "agent".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "sync.pair".to_string(),
                "sync.unpair".to_string(),
                "sync.apply".to_string(),
            ],
            queries: vec!["sync.peers".to_string(), "sync.cursor".to_string()],
            events: vec![
                "sync.peer.paired".to_string(),
                "sync.peer.unpaired".to_string(),
                "sync.applied".to_string(),
            ],
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: commands(),
        queries: queries(),
        events: events(),
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Apply a host-accepted batch".to_string(),
            summary: "The host validates peer authority, encodes allowlisted foreign events, then dispatches sync.apply."
                .to_string(),
            language: "cli".to_string(),
            code: "terrane sync notes --peer http://127.0.0.1:8780".to_string(),
            expected: "sync.applied advances the cursor, followed by the foreign kv events in arrival order."
                .to_string(),
        }],
        constraints: vec![
            "sync has no app-facing ctx.resource surface; apps never drive sync.".to_string(),
            "Only kv.set and kv.deleted are accepted in v2 event batches; CRDT uses crdt.update deltas and blobs use the host blob pass."
                .to_string(),
            "kv conflict resolution is last-writer-wins by local log order: accepted foreign events fold after earlier local writes."
                .to_string(),
            "sync.apply validates cursor monotonicity and strictly increasing origin_seq values before committing anything."
                .to_string(),
            "Bearer tokens and pairing codes live at the host edge, never in the event log.".to_string(),
        ],
        limits: vec![
            limit("batchBytes", "64 MiB", "Matches the existing sync frame cap."),
            limit("batchEvents", "5000", "Larger backlogs page through repeated sync.apply batches."),
        ],
        compatibility: vec![
            "CRDT semantics are unchanged; the host still merges crdt.update deltas through crdt.merge."
                .to_string(),
            "Blob metadata is not event-synced in v2; the host copies CAS rows referenced by folded blob state after the event pass."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Replay boundary".to_string(),
                body: "sync.apply records sync.applied plus copied foreign events; replay folds those facts without network access."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "sync.pair",
            &[
                param("peer_hex", "Remote replica peer id in hex.", "hex"),
                param("display_name", "Human-readable peer label.", "string"),
            ],
            "commit",
            "Record or refresh an idempotent paired-peer fact.",
        )
        .with_emits(&["sync.peer.paired"])
        .with_errors(&["invalid peer_hex", "empty display_name"]),
        command_doc(
            "sync.unpair",
            &[param("peer_hex", "Remote replica peer id in hex.", "hex")],
            "commit",
            "Mark a peer unpaired; repeated unpair is a successful no-op.",
        )
        .with_emits(&["sync.peer.unpaired"])
        .with_errors(&["invalid peer_hex"]),
        command_doc(
            "sync.apply",
            &[
                param("peer_hex", "Origin peer for this page.", "hex"),
                param("app", "App id being synced.", "app_id"),
                param("from_seq", "First origin log sequence in the page.", "u64"),
                param("to_seq", "Last origin log sequence in the page.", "u64"),
                param("batch_hex", "Borsh Vec<SyncEnvelope>, hex encoded.", "hex"),
            ],
            "commit",
            "Validate a page of allowlisted foreign events and record it in local arrival order.",
        )
        .with_emits(&["sync.applied", "kv.set", "kv.deleted"])
        .with_errors(&[
            "cursor mismatch",
            "non-increasing origin_seq",
            "batch too large",
            "event kind outside the v2 allowlist",
            "kv payload app mismatch",
        ]),
    ]
}

fn queries() -> Vec<QueryDoc> {
    vec![
        query_doc("sync.peers", &[], "json", "List folded peer roster facts.")
            .with_errors(&["unknown query capability or query name"]),
        query_doc(
            "sync.cursor",
            &[
                param("peer", "Origin peer in hex.", "hex"),
                param("app", "App id.", "app_id"),
            ],
            "u64",
            "Return the folded cursor for one origin peer and app, or 0 when absent.",
        )
        .with_errors(&["missing peer or app argument", "unknown query capability or query name"]),
    ]
}

fn events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "sync.peer.paired",
            &[
                param("peer", "Remote replica peer id.", "hex"),
                param("display_name", "Human-readable peer label.", "string"),
            ],
            "Durable fact that a peer is paired.",
        ),
        event_doc(
            "sync.peer.unpaired",
            &[param("peer", "Remote replica peer id.", "hex")],
            "Durable fact that a peer was unpaired.",
        ),
        event_doc(
            "sync.applied",
            &[
                param("peer", "Origin peer.", "hex"),
                param("app", "App id.", "app_id"),
                param("from_seq", "First accepted origin seq.", "u64"),
                param("to_seq", "Last accepted origin seq.", "u64"),
            ],
            "Advances the local cursor for one accepted foreign-event page.",
        ),
    ]
}
