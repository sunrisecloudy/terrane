use terrane_cap_interface::{
    command_doc, event_doc, limit, param, CapabilityDoc, CapabilityManifestDoc, ExampleDoc,
    InternalNote, ResourceDoc, ResourceMethodDoc,
};

pub fn connection_doc(include_internal: bool) -> CapabilityDoc {
    let mut doc = CapabilityDoc {
        namespace: "connection".to_string(),
        title: "Connection".to_string(),
        summary: "Replayable metadata for host-edge OAuth and named secret connections.".to_string(),
        status: "experimental".to_string(),
        version: "v1".to_string(),
        audience: vec!["app-author".to_string(), "host-implementer".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "connection.define".to_string(),
                "connection.remove".to_string(),
                "connection.mark_authorized".to_string(),
            ],
            queries: Vec::new(),
            events: vec![
                "connection.defined".to_string(),
                "connection.authorized".to_string(),
                "connection.refreshed".to_string(),
                "connection.removed".to_string(),
            ],
            subscriptions: Vec::new(),
            resource_methods: vec![
                method_doc("list", "read", &[], "JSON map of visible connection metadata.", "List granted connection metadata."),
                method_doc(
                    "stat",
                    "read",
                    &[param("name", "Connection name.", "connectionName")],
                    "JSON metadata object or null.",
                    "Return one granted connection's metadata.",
                ),
            ],
        },
        commands: vec![
            command_doc(
                "connection.define",
                &[
                    param("name", "Connection name.", "connectionName"),
                    param("kind", "apiKey, oauth2, or smtp.", "connectionKind"),
                    param("config_public_json", "Public provider/account config only.", "jsonObject"),
                ],
                "connection.defined",
                "Record non-secret connection metadata.",
            )
            .with_errors(&[
                "invalid name",
                "unsupported kind",
                "secret field in public config",
                "connection limit exceeded",
            ])
            .with_emits(&["connection.defined"]),
            command_doc(
                "connection.remove",
                &[param("name", "Connection name.", "connectionName")],
                "connection.removed",
                "Remove replayable metadata; the host deletes edge secret fields.",
            )
            .with_errors(&["invalid name"])
            .with_emits(&["connection.removed"]),
            command_doc(
                "connection.mark_authorized",
                &[
                    param("name", "Connection name.", "connectionName"),
                    param("scopes", "Comma-separated public scopes.", "string"),
                    param("expires_at", "Public token expiry timestamp.", "string"),
                ],
                "connection.authorized or connection.refreshed",
                "Trusted host records successful OAuth acquisition or refresh facts.",
            )
            .with_errors(&["invalid name", "unknown connection", "missing expires_at"])
            .with_emits(&["connection.authorized", "connection.refreshed"]),
        ],
        queries: Vec::new(),
        events: vec![
            event_doc(
                "connection.defined",
                &[
                    param("name", "Connection name.", "connectionName"),
                    param("kind", "apiKey, oauth2, or smtp.", "connectionKind"),
                    param("config_public_json", "Public provider/account config only.", "jsonObject"),
                ],
                "Public metadata for a named connection.",
            ),
            event_doc(
                "connection.authorized",
                &[
                    param("name", "Connection name.", "connectionName"),
                    param("scopes", "Public scopes granted by the provider.", "stringArray"),
                    param("expires_at", "Public token expiry timestamp.", "string"),
                ],
                "A connection has usable edge-held tokens.",
            ),
            event_doc(
                "connection.refreshed",
                &[
                    param("name", "Connection name.", "connectionName"),
                    param("expires_at", "Public token expiry timestamp.", "string"),
                ],
                "A connection token expiry changed after refresh.",
            ),
            event_doc(
                "connection.removed",
                &[param("name", "Connection name.", "connectionName")],
                "A connection metadata record was removed.",
            ),
        ],
        resources: vec![ResourceDoc {
            namespace: "connection".to_string(),
            summary: "Read connection metadata only; never returns secret bytes.".to_string(),
            methods: vec![
                method_doc("list", "read", &[], "JSON map of visible connection metadata.", "List metadata."),
                method_doc(
                    "stat",
                    "read",
                    &[param("name", "Connection name.", "connectionName")],
                    "JSON metadata object or null.",
                    "Read metadata for one connection.",
                ),
            ],
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Use a named connection in an HTTP call".to_string(),
            summary: "The app sees metadata through ctx.resource.connection, then passes a $secret marker to net; the host edge resolves it after the connection grant check.".to_string(),
            language: "javascript".to_string(),
            code: r#"function handle() {
  const meta = ctx.resource.connection.stat("github");
  return ctx.resource.net.call(JSON.stringify({
    url: "https://api.github.com/user",
    headers: { authorization: { "$secret": "github" } }
  }));
}"#.to_string(),
            expected: "The event log records connection metadata and the $secret marker, never the resolved token.".to_string(),
        }],
        constraints: vec![
            "Secret bytes never appear in events, state, describe output, or resource reads.".to_string(),
            "$secret markers resolve only at the host edge after a per-name connection grant.".to_string(),
            "Replay rebuilds only metadata; keychain/file secret material is a side artifact.".to_string(),
        ],
        limits: vec![
            limit("connectionsPerHome", "64", "Keeps the operator inventory bounded."),
            limit("nameChars", "64", "Names must fit compact grant ids."),
            limit("secretFieldBytes", "65536", "Prevents oversized credential blobs."),
        ],
        compatibility: vec![
            "net v2 records the $secret marker verbatim; request keys stay stable across secret rotation.".to_string(),
        ],
        internal: vec![InternalNote {
            title: "Replay boundary".to_string(),
            body: "The host resolver fetches secret fields by reference immediately before effects execute.".to_string(),
        }],
    };
    if !include_internal {
        doc = doc.without_internal();
    }
    doc
}

fn method_doc(
    name: &str,
    kind: &str,
    params: &[terrane_cap_interface::ParamDoc],
    returns: &str,
    summary: &str,
) -> ResourceMethodDoc {
    ResourceMethodDoc {
        name: name.to_string(),
        kind: kind.to_string(),
        params: params.to_vec(),
        returns: returns.to_string(),
        summary: summary.to_string(),
        errors: vec!["invalid input".to_string(), "missing connection grant".to_string()],
    }
}
