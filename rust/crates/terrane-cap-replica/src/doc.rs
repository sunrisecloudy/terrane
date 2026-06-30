use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, QueryDoc, ResourceDoc, SchemaDoc,
};

pub fn replica_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "replica".to_string(),
        title: "Replica Identity".to_string(),
        summary: "Stable local identity for a Terrane home, used to author replay-safe CRDT edits."
            .to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["replica.init".to_string()],
            queries: vec!["replica.peer".to_string()],
            events: vec!["replica.initialized".to_string()],
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: replica_commands(),
        queries: replica_queries(),
        events: replica_events(),
        resources: Vec::<ResourceDoc>::new(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Ensure local identity".to_string(),
            summary: "Initialize the home once before CRDT writes need a Loro PeerID.".to_string(),
            language: "cli".to_string(),
            code: "terrane replica init".to_string(),
            expected:
                "first initialization records replica.initialized; later init calls can return records:0 because the home already has a peer"
                    .to_string(),
        }],
        constraints: vec![
            "replica.init is idempotent: if replica.peer already exists, the command is successful and commits no new events, often surfaced as records:0."
                .to_string(),
            "Use capability_query replica.peer after replica.init; a numeric u64 peer is the proof of initialized identity even when replica.init reports records:0."
                .to_string(),
            "When no peer exists, replica.init returns Effect::NewReplicaId and the edge records replica.initialized."
                .to_string(),
            "The edge runner mints the peer id with OS entropy and records replica.initialized."
                .to_string(),
            "Replay never re-mints identity; it restores the peer solely from replica.initialized."
                .to_string(),
            "The first initialized event wins so a duplicated event cannot change stable replica identity."
                .to_string(),
            "replica.peer is a derived query over folded ReplicaState and is not recorded.".to_string(),
        ],
        limits: vec![
            limit("peerBits", "64", "Matches the Loro PeerID stored by this capability."),
            limit("identitiesPerHome", "1", "A TERRANE_HOME has one stable author identity."),
        ],
        compatibility: vec![
            "The crdt capability reads replica.peer and authors local edits under this stable id."
                .to_string(),
            "Replica identity is local to a home; sync transports exchange recorded app data, not fresh replica ids."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Effect boundary".to_string(),
                body: "NewReplicaId is the only nondeterministic part. The recorded replica.initialized event is the replay boundary."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn replica_commands() -> Vec<CommandDoc> {
    vec![command_doc(
        "replica.init",
        &[],
        "effect|commit; records:0 is a successful no-op when already initialized",
        "Ensure this home has one stable peer id, minting it at the edge when absent and doing nothing when a peer already exists.",
    )
    .with_effects(&["NewReplicaId"])
    .with_emits(&["replica.initialized"])
    .with_errors(&[
        "edge runner unavailable when a new peer id must be minted",
        "storage failure while recording replica.initialized",
    ])
    .with_examples(&[
        ExampleDoc {
            title: "Initialize then read peer".to_string(),
            summary: "Agents should query replica.peer after init instead of inferring success from record count alone."
                .to_string(),
            language: "mcp".to_string(),
            code: r#"capability_command {"name":"replica.init"}
capability_query {"capability":"replica","query":"peer","args":[]}"#
                .to_string(),
            expected: "replica.peer returns a u64. If init reports records:0, the peer was already present."
                .to_string(),
        },
    ])]
}

fn replica_queries() -> Vec<QueryDoc> {
    vec![query_doc(
        "replica.peer",
        &[],
        "u64|null",
        "Return the folded stable peer id for this home, if it has been initialized.",
    )
    .with_errors(&["unknown query capability or query name"])]
}

fn replica_events() -> Vec<EventDoc> {
    vec![event_doc(
        "replica.initialized",
        &[param("peer", "Edge-minted stable Loro PeerID.", "u64")],
        "Records the stable replica identity for this home.",
    )
    .with_effects(&["sets ReplicaState.peer when empty"])]
}
