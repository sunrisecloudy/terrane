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
            expected: "records replica.initialized once; later init calls are no-ops".to_string(),
        }],
        constraints: vec![
            "replica.init returns Effect::NewReplicaId until a peer exists, then commits no events."
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
        "effect|commit",
        "Ensure this home has one stable peer id, minting it at the edge when absent.",
    )
    .with_effects(&["NewReplicaId"])
    .with_emits(&["replica.initialized"])
    .with_errors(&[
        "edge runner unavailable when a new peer id must be minted",
        "storage failure while recording replica.initialized",
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
