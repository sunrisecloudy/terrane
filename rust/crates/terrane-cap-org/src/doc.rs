use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, CapabilityDoc, CapabilityManifestDoc,
    ExampleDoc, InternalNote,
};

pub fn org_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "org".to_string(),
        title: "Shared Home — Organization".to_string(),
        summary: "An organization is a Terrane home of its own. The org cap records the org identity, open invites, and person-signed role grants; members sync under those grants. Premium hosting is a convenience and never a limitation — a self-hosted org home works identically."
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
                "org.create".to_string(),
                "org.invite".to_string(),
                "org.join".to_string(),
                "org.leave".to_string(),
                "org.role.set".to_string(),
            ],
            queries: vec!["org.info".to_string(), "org.members".to_string()],
            events: vec![
                "org.created".to_string(),
                "org.invited".to_string(),
                "org.invite.redeemed".to_string(),
                "org.member.granted".to_string(),
                "org.member.left".to_string(),
            ],
            subscriptions: Vec::new(),
            resource_methods: Vec::new(),
        },
        commands: org_commands(),
        queries: org_queries(),
        events: org_events(),
        resources: Vec::new(),
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Found a self-hosted org".to_string(),
            summary: "The founder mints a person key, then creates the org; the org home rides the existing home and the founder becomes the first owner."
                .to_string(),
            language: "sh".to_string(),
            code: "terrane person create\nterrane org create <founder-person-id>\nterrane org info"
                .to_string(),
            expected: "records org.created { org_id, pubkey, founder } plus an org.member.granted owner grant signed by the founder; org.info returns the folded org"
                .to_string(),
        }],
        constraints: vec![
            "An org is a shared Terrane home; Premium hosting is a convenience and never a limitation."
                .to_string(),
            "Membership facts are person-signed role grants: org.member.granted carries an ed25519 signature over (org_id, member, role) by the signer's person key."
                .to_string(),
            "Replay verifies each grant against the signer's folded person public key, so rebuilt state matches byte-for-byte without keychain access."
                .to_string(),
            "Enforcement (who may issue a grant, who may redeem an invite) is edge policy over folded state at the sync routes and host helpers — the same stance as share-invite."
                .to_string(),
            "The org keypair's private seed never appears in events, folded State, descriptions, docs, or query output."
                .to_string(),
            "Capabilities never set the actor; the engine stamps provenance. The org context rides the ExecutionPrincipal stamp."
                .to_string(),
        ],
        limits: vec![
            limit(
                "orgId",
                "sha256(pubkey) hex-16 prefix",
                "Compact stable handle for the org; the full public key is still recorded in org.created.",
            ),
            limit(
                "orgsPerHome",
                &super::MAX_ORGS_PER_HOME.to_string(),
                "Soft cap to avoid an unbounded folded org map in one home.",
            ),
            limit(
                "membersPerOrg",
                &super::MAX_MEMBERS_PER_ORG.to_string(),
                "Soft cap to avoid an unbounded folded membership table per org.",
            ),
            limit(
                "openInvitesPerOrg",
                &super::MAX_OPEN_INVITES_PER_ORG.to_string(),
                "Soft cap to avoid unbounded open invite state per org.",
            ),
            limit(
                "inviteNoteBytes",
                &super::MAX_INVITE_NOTE_BYTES.to_string(),
                "Maximum UTF-8 bytes for an org invite note.",
            ),
        ],
        compatibility: vec![
            "Future cross-org federation can add new roles to validate_role without changing the event shape; existing grants keep replaying."
                .to_string(),
            "Premium always-on hosting can be added by provisioning the same org home on a remote machine — the Rust side needs nothing Premium-specific."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Secret storage".to_string(),
                body: "The host stores the org ed25519 seed in the existing connection secret store under org-<org_id>.ed25519, mirroring the person primitive's keychain handling. The event log records only public keys, invite token hashes, and signatures."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn org_commands() -> Vec<terrane_cap_interface::CommandDoc> {
    vec![
        command_doc(
            "org.create",
            &[param("founder", "Person id of the founding owner.", "person_id")],
            "effect",
            "Mint the org keypair once and record the org home plus the founder's owner grant.",
        )
        .with_errors(&[
            "invalid founder person_id",
            "org limit exceeded",
            "edge key generation or secret-store failure",
        ])
        .with_effects(&["OrgKeygen"])
        .with_emits(&["org.created", "org.member.granted"]),
        command_doc(
            "org.invite",
            &[
                param("org_id", "Existing org id.", "org_id"),
                param("role", "Role the invite grants on redemption (owner/admin/member).", "string"),
                param("token_hash", "sha256 hex of the invite token; the host mints the token out-of-band.", "hex"),
                param("note", "Optional human-readable note (empty allowed).", "string"),
            ],
            "commit",
            "Record an open org invite for a role.",
        )
        .with_errors(&[
            "unknown org",
            "invalid role",
            "invalid token_hash",
            "note too large",
            "open invite limit exceeded",
        ])
        .with_emits(&["org.invited"]),
        command_doc(
            "org.join",
            &[
                param("org_id", "Existing org id.", "org_id"),
                param("token_hash", "sha256 hex of the invite token being redeemed.", "hex"),
                param("member", "Person id of the joining member.", "person_id"),
            ],
            "effect",
            "Redeem an open invite and self-sign the role grant with the member's person key.",
        )
        .with_errors(&[
            "unknown org",
            "invite not open or already redeemed",
            "invalid member person_id",
            "edge signing failure",
        ])
        .with_effects(&["OrgRoleSign"])
        .with_emits(&["org.member.granted", "org.invite.redeemed"]),
        command_doc(
            "org.leave",
            &[
                param("org_id", "Existing org id.", "org_id"),
                param("member", "Person id of the member leaving the org.", "person_id"),
            ],
            "commit",
            "Mark a folded org membership inactive.",
        )
        .with_errors(&["unknown org", "invalid member person_id"])
        .with_emits(&["org.member.left"]),
        command_doc(
            "org.role.set",
            &[
                param("org_id", "Existing org id.", "org_id"),
                param("member", "Person id of the target member.", "person_id"),
                param("role", "New role to grant (owner/admin/member).", "string"),
                param("signer", "Person id of the admin/owner signing the grant.", "person_id"),
            ],
            "effect",
            "Issue a person-signed role grant, replacing the member's current role.",
        )
        .with_errors(&[
            "unknown org",
            "invalid role",
            "invalid member/signer person_id",
            "edge signing failure",
        ])
        .with_effects(&["OrgRoleSign"])
        .with_emits(&["org.member.granted"]),
    ]
}

fn org_queries() -> Vec<terrane_cap_interface::QueryDoc> {
    vec![
        query_doc(
            "org.info",
            &[param("org_id", "Optional org id; defaults to the primary org.", "org_id")],
            "json|null",
            "Return the folded org record or null when no org exists.",
        )
        .with_errors(&["invalid org_id", "invalid folded org JSON serialization"]),
        query_doc(
            "org.members",
            &[param("org_id", "Optional org id; defaults to the primary org.", "org_id")],
            "json",
            "Return the folded membership list for an org (member, role, signer, active).",
        )
        .with_errors(&["invalid org_id"]),
    ]
}

fn org_events() -> Vec<terrane_cap_interface::EventDoc> {
    vec![
        event_doc(
            "org.created",
            &[
                param("org_id", "sha256(pubkey) hex-16 prefix.", "org_id"),
                param("pubkey", "ed25519 verifying key as 64 hex chars.", "hex"),
                param("founder", "Person id of the founding owner.", "person_id"),
            ],
            "Records the org home's identity; contains no private key material.",
        ),
        event_doc(
            "org.invited",
            &[
                param("org_id", "Existing org id.", "org_id"),
                param("role", "Role the invite grants on redemption.", "string"),
                param("token_hash", "sha256 hex of the invite token.", "hex"),
                param("note", "Human-readable note.", "string"),
            ],
            "Records an open org invite; the token itself never enters the log.",
        ),
        event_doc(
            "org.invite.redeemed",
            &[
                param("org_id", "Existing org id.", "org_id"),
                param("token_hash", "Token hash of the redeemed invite.", "hex"),
                param("member", "Person id that redeemed the invite.", "person_id"),
            ],
            "Marks an invite closed when a member redeems it.",
        ),
        event_doc(
            "org.member.granted",
            &[
                param("org_id", "Existing org id.", "org_id"),
                param("member", "Person id granted the role.", "person_id"),
                param("role", "Granted role (owner/admin/member).", "string"),
                param("sig", "ed25519 signature over (org_id, member, role) as 128 hex chars.", "hex"),
                param("signer", "Person id whose public key verifies the signature.", "person_id"),
            ],
            "Records a person-signed role grant; fold verifies the signature against the signer's folded person pubkey.",
        )
        .with_effects(&["upserts active membership after signature verification"]),
        event_doc(
            "org.member.left",
            &[
                param("org_id", "Existing org id.", "org_id"),
                param("member", "Person id leaving the org.", "person_id"),
            ],
            "Marks a matching membership inactive in folded state.",
        ),
    ]
}