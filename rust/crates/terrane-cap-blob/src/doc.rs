use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    ExampleDoc, InternalNote, ResourceDoc,
};

use crate::{MAX_BLOBS_PER_APP, MAX_BLOB_SIZE, MAX_NAME_LEN};

pub fn blob_doc(include_internal: bool) -> CapabilityDoc {
    let methods = vec![
        method(
            "put",
            "call",
            &[
                param("name", "App-local blob name.", "blob_name"),
                param("base64", "Standard base64 encoded bytes.", "base64"),
                param("mime", "MIME type stored with the metadata.", "mime"),
            ],
            "Stores bytes in the host CAS and records metadata only.",
            "Lowercase SHA-256 hex hash of the stored bytes.",
        ),
        method(
            "get",
            "read",
            &[param("name", "App-local blob name.", "blob_name")],
            "Reads base64 bytes from the verified host CAS.",
            "Standard base64 encoded bytes.",
        ),
        method(
            "stat",
            "read",
            &[param("name", "App-local blob name.", "blob_name")],
            "Returns JSON metadata for one blob.",
            "JSON string with name, hash, size, and mime.",
        ),
        method(
            "list",
            "read",
            &[param("prefix", "Optional name prefix.", "string")],
            "Returns a JSON array of metadata for names with the prefix.",
            "JSON array string of {name, hash, size, mime} objects.",
        ),
        method(
            "rm",
            "write",
            &[param("name", "App-local blob name.", "blob_name")],
            "Removes one blob name from folded state.",
            "No direct value; records blob.removed.",
        ),
    ];
    CapabilityDoc {
        namespace: "blob".to_string(),
        title: "Binary Blob Storage".to_string(),
        summary: "App-scoped binary storage with SHA-256 metadata in the event log and bytes in a host-owned content-addressed SQLite sidecar.".to_string(),
        status: "stable".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "blob.put".to_string(),
                "blob.rm".to_string(),
                "blob.link".to_string(),
            ],
            queries: Vec::new(),
            events: vec!["blob.stored".to_string(), "blob.removed".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: methods.clone(),
        },
        commands: vec![
            command_doc(
                "blob.put",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("name", "App-local blob name.", "blob_name"),
                    param("mime", "MIME type for host serving.", "mime"),
                    param("bytes_base64", "Standard base64 encoded bytes.", "base64"),
                ],
                "effect",
                "Decode bytes, compute SHA-256, store bytes through the host CAS, and record metadata.",
            )
            .with_errors(&["missing app", "empty/too-long name", "invalid base64", "size cap"])
            .with_emits(&["blob.stored"]),
            command_doc(
                "blob.rm",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("name", "App-local blob name.", "blob_name"),
                ],
                "events",
                "Remove an existing app-local blob name.",
            )
            .with_errors(&["missing blob"])
            .with_emits(&["blob.removed"]),
            command_doc(
                "blob.link",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("name", "App-local blob name.", "blob_name"),
                    param("hash", "Lowercase SHA-256 hex digest.", "sha256_hex"),
                    param("size", "Byte length.", "u64"),
                    param("mime", "MIME type.", "mime"),
                ],
                "events",
                "Attach a name to already-addressed bytes; CAS presence is verified on read.",
            )
            .with_errors(&["bad hash", "size cap", "per-app count cap"])
            .with_emits(&["blob.stored"]),
        ],
        queries: Vec::new(),
        events: vec![
            event_doc(
                "blob.stored",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("name", "App-local blob name.", "blob_name"),
                    param("hash", "Lowercase SHA-256 hex digest.", "sha256_hex"),
                    param("size", "Byte length.", "u64"),
                    param("mime", "MIME type.", "mime"),
                ],
                "Upserts app/name metadata and increments the logical hash refcount.",
            ),
            event_doc(
                "blob.removed",
                &[
                    param("app", "Target app id.", "app_id"),
                    param("name", "App-local blob name.", "blob_name"),
                    param("hash", "Lowercase SHA-256 hex digest.", "sha256_hex"),
                ],
                "Drops app/name metadata and decrements the logical hash refcount.",
            ),
        ],
        resources: vec![ResourceDoc {
            namespace: "blob".to_string(),
            summary: "Backend resource surface installed as ctx.resource.blob for apps that declare the blob resource.".to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Store and read an attachment".to_string(),
            summary: "The app gets the hash from put, stat/list read folded metadata, and get verifies bytes at the host edge.".to_string(),
            language: "js".to_string(),
            code: include_str!("../examples/blob_resource_methods.js").to_string(),
            expected: "A JSON string with matching hash, metadata, and decoded content.".to_string(),
        }],
        constraints: vec![
            "Events carry only hash, size, MIME type, app, and name; raw bytes never enter the event log.".to_string(),
            "Replay folds blob metadata and logical refcounts only; it never opens the SQLite CAS or re-runs byte writes.".to_string(),
            "Reads verify the SHA-256 of sidecar bytes before returning base64; missing or corrupt bytes are typed errors.".to_string(),
            "blob.link does not check CAS presence during decide; dangling metadata is healed by syncing/copying sidecar rows or reported on read.".to_string(),
        ],
        limits: vec![
            limit("maxBlobSizeBytes", &MAX_BLOB_SIZE.to_string(), "Maximum decoded byte length for blob.put and blob.link."),
            limit("maxNameBytes", &MAX_NAME_LEN.to_string(), "Maximum UTF-8 byte length of a blob name."),
            limit("maxBlobsPerApp", &MAX_BLOBS_PER_APP.to_string(), "Soft cap on distinct names per app."),
        ],
        compatibility: vec![
            "SHA-256 lowercase hex is the wire identity format and must not change in v1.".to_string(),
            "The v1 host CAS is $TERRANE_HOME/blobs.sqlite3 with PRAGMA user_version = 1.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Sidecar split".to_string(),
                body: "The pure capability owns metadata and deterministic fold; terrane-host owns SQLite byte IO, verification, sync copying, and GC.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn method(
    name: &str,
    kind: &str,
    params: &[terrane_cap_interface::ParamDoc],
    summary: &str,
    returns: &str,
) -> terrane_cap_interface::ResourceMethodDoc {
    let mut doc = resource_method(name, kind, params, summary);
    doc.returns = returns.to_string();
    doc
}
