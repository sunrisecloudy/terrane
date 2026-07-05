use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, CapabilityDoc, CapabilityManifestDoc,
    CommandDoc, EventDoc, ExampleDoc, InternalNote, QueryDoc,
};

pub fn person_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "person".to_string(),
        title: "Durable Person Identity".to_string(),
        summary: "Local ed25519 identity. The public key and attestations replay from the log; the private key stays in the host secret store."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "host-implementer".to_string(),
            "agent".to_string(),
            "app-author".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "person.create".to_string(),
                "person.attest".to_string(),
                "person.revoke-attestation".to_string(),
                "person.rotate".to_string(),
            ],
            queries: vec!["person.whoami".to_string(), "person.get".to_string()],
            events: vec![
                "person.created".to_string(),
                "person.attested".to_string(),
                "person.attestation-revoked".to_string(),
                "person.rotated".to_string(),
            ],
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: person_commands(),
        queries: person_queries(),
        events: person_events(),
        resources: Vec::new(),
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Create a local person".to_string(),
            summary: "The host mints an ed25519 keypair once, stores the private key outside replay, and records only the public key."
                .to_string(),
            language: "sh".to_string(),
            code: "terrane person create\nterrane person whoami".to_string(),
            expected: "records person.created { person_id, pubkey }; whoami returns folded public identity"
                .to_string(),
        }],
        constraints: vec![
            "Person identity is the local ed25519 keypair; Premium accounts, email addresses, replicas, and devices attach only as attestations."
                .to_string(),
            "Private key bytes never appear in events, folded State, descriptions, docs, or query output."
                .to_string(),
            "person.create returns Effect::PersonKeygen; the edge stores signing material in the connection secret store and records person.created."
                .to_string(),
            "person.attest returns Effect::PersonSign; replay verifies person.attested from the folded public key and signature."
                .to_string(),
            "person.rotate records a new public key signed by the current key or an active device-key attestation."
                .to_string(),
            "Replay identity is public-only: folded PersonState rebuilds from person.* events without keychain access."
                .to_string(),
        ],
        limits: vec![
            limit(
                "personId",
                "sha256(pubkey) hex-16 prefix",
                "The full public key is still recorded in person.created; the id is a compact stable handle.",
            ),
            limit(
                "attestationsPerPerson",
                &super::MAX_ATTESTATIONS_PER_PERSON.to_string(),
                "Soft cap to avoid unbounded folded identity state.",
            ),
            limit(
                "claimLength",
                &super::MAX_CLAIM_LEN.to_string(),
                "Maximum UTF-8 bytes for an attestation claim.",
            ),
        ],
        compatibility: vec![
            "Future auth subject migration can rebind user:local-owner to user:<person_id> without changing the replay event shape."
                .to_string(),
            "Publishing can converge on the person key because signatures are already edge effects over this keypair."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Secret storage".to_string(),
                body: "The host stores the ed25519 seed via terrane-host secret_store using a person-scoped connection-secret name; fallback file mode stays encrypted by terrane-cap-crypto primitives."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn person_commands() -> Vec<CommandDoc> {
    vec![
        command_doc(
            "person.create",
            &[],
            "effect",
            "Mint the local ed25519 keypair once and record the public identity.",
        )
        .with_errors(&["unexpected arguments", "edge key generation or secret-store failure"])
        .with_effects(&["PersonKeygen"])
        .with_emits(&["person.created"]),
        command_doc(
            "person.attest",
            &[
                param("person_id", "Existing person id.", "person_id"),
                param("kind", "replica, premium-account, email, or device-key.", "string"),
                param("claim", "Claim value attached to the person.", "string"),
            ],
            "effect",
            "Sign one attestation with the person key at the edge.",
        )
        .with_errors(&["unknown person", "invalid attestation kind", "claim too large"])
        .with_effects(&["PersonSign"])
        .with_emits(&["person.attested"]),
        command_doc(
            "person.revoke-attestation",
            &[
                param("person_id", "Existing person id.", "person_id"),
                param("kind", "Attestation kind to revoke.", "string"),
                param("claim", "Claim value to revoke.", "string"),
            ],
            "commit",
            "Mark a folded attestation revoked.",
        )
        .with_errors(&["unknown person", "invalid attestation kind", "claim too large"])
        .with_emits(&["person.attestation-revoked"]),
        command_doc(
            "person.rotate",
            &[param("person_id", "Existing person id.", "person_id")],
            "effect",
            "Mint a replacement key and record it signed by the old key.",
        )
        .with_errors(&["unknown person", "edge key generation or secret-store failure"])
        .with_effects(&["PersonRotate"])
        .with_emits(&["person.rotated"]),
    ]
}

fn person_queries() -> Vec<QueryDoc> {
    vec![
        query_doc(
            "person.whoami",
            &[],
            "json|null",
            "Return the primary folded person and active attestations.",
        )
        .with_errors(&["invalid folded person JSON serialization"]),
        query_doc(
            "person.get",
            &[param("person_id", "Person id to inspect.", "person_id")],
            "json|null",
            "Return one folded person by id.",
        )
        .with_errors(&["invalid person_id", "invalid folded person JSON serialization"]),
    ]
}

fn person_events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "person.created",
            &[
                param("person_id", "sha256(pubkey) hex-16 prefix.", "person_id"),
                param("pubkey", "ed25519 verifying key as 64 hex chars.", "hex"),
            ],
            "Records the public identity; contains no private key material.",
        )
        .with_effects(&["creates PersonState.persons[person_id]"]),
        event_doc(
            "person.attested",
            &[
                param("person_id", "Existing person id.", "person_id"),
                param("kind", "Attestation kind.", "string"),
                param("claim", "Claim value.", "string"),
                param("sig", "ed25519 signature as 128 hex chars.", "hex"),
            ],
            "Records a signed public claim attached to a person.",
        )
        .with_effects(&["upserts active attestation after signature verification"]),
        event_doc(
            "person.attestation-revoked",
            &[
                param("person_id", "Existing person id.", "person_id"),
                param("kind", "Attestation kind.", "string"),
                param("claim", "Claim value.", "string"),
            ],
            "Marks a matching attestation inactive in folded state.",
        ),
        event_doc(
            "person.rotated",
            &[
                param("old", "Stable person id whose key is rotating.", "person_id"),
                param("new_pubkey", "Replacement ed25519 public key.", "hex"),
                param("sig", "Signature by current key or active device-key.", "hex"),
            ],
            "Moves signing to a replacement public key while preserving the person id.",
        ),
    ]
}
