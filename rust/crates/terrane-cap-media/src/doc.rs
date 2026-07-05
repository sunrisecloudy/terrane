use terrane_cap_interface::{
    command_doc, event_doc, limit, param, resource_method, CapabilityDoc, CapabilityManifestDoc,
    ExampleDoc, InternalNote, ResourceDoc,
};

use crate::{MAX_PIXEL_BUDGET, MAX_TRANSFORMS_PER_APP};
use crate::ops::MAX_OPS;

pub fn media_doc(include_internal: bool) -> CapabilityDoc {
    let mut info = resource_method(
        "info",
        "read",
        &[param("blobName", "App-local blob name to probe.", "blob_name")],
        "Probe image/audio/video metadata at the live edge without recording events.",
    );
    info.returns = "JSON media metadata string.".to_string();
    let methods = vec![info];
    CapabilityDoc {
        namespace: "media".to_string(),
        title: "Media Understanding".to_string(),
        summary: "Image/audio/video metadata and deterministic transform records over app-scoped blob CAS bytes.".to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec!["app-author".to_string(), "agent".to_string(), "host-implementer".to_string()],
        manifest: CapabilityManifestDoc {
            commands: vec!["media.transform".to_string()],
            queries: Vec::new(),
            events: vec!["media.transformed".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: methods.clone(),
        },
        commands: vec![command_doc(
            "media.transform",
            &[
                param("app", "Target app id.", "app_id"),
                param("source_name", "Existing app-local source blob name.", "blob_name"),
                param("ops_json", "Ordered JSON op array.", "json"),
                param("dest_name", "App-local derived blob name.", "blob_name"),
            ],
            "effect",
            "Validate source metadata and requested operations, then ask the edge to transform and store derived bytes.",
        )
        .with_errors(&["missing source blob", "unsupported media", "invalid ops", "output too large"])
        .with_effects(&["MediaTransform"])
        .with_emits(&["media.transformed", "blob.stored"])],
        queries: Vec::new(),
        events: vec![event_doc(
            "media.transformed",
            &[
                param("app", "Target app id.", "app_id"),
                param("source_hash", "Source blob SHA-256.", "sha256_hex"),
                param("ops_json", "Ordered transform op JSON.", "json"),
                param("dest_name", "Derived blob name.", "blob_name"),
                param("dest_hash", "Derived blob SHA-256.", "sha256_hex"),
                param("dest_size", "Derived byte length.", "u64"),
                param("dest_mime", "Derived MIME type.", "mime"),
            ],
            "Records the edge-computed transform output identity; replay never re-encodes.",
        )],
        resources: vec![ResourceDoc {
            namespace: "media".to_string(),
            summary: "Backend resource surface installed as ctx.resource.media for apps that declare the media resource.".to_string(),
            methods,
        }],
        schemas: Vec::new(),
        examples: vec![ExampleDoc {
            title: "Create a shell thumbnail".to_string(),
            summary: "Probe a stored photo and create a derived thumbnail blob that the shell can serve via blobUrl.".to_string(),
            language: "js".to_string(),
            code: include_str!("../examples/media_thumbnail.js").to_string(),
            expected: "A JSON string naming the generated __thumb__ blob.".to_string(),
        }],
        constraints: vec![
            "Media bytes live in blob CAS; media events carry refs and derived metadata only.".to_string(),
            "Transforms and probes that touch codecs run at the edge. Replay folds recorded facts only.".to_string(),
            "Video transforms are not supported in v1; ffprobe absence for video info returns probe unavailable.".to_string(),
        ],
        limits: vec![
            limit("maxOpsPerTransform", &MAX_OPS.to_string(), "Maximum ordered ops per transform."),
            limit("maxDecodedPixels", &MAX_PIXEL_BUDGET.to_string(), "Maximum decoded image pixel budget before full transform."),
            limit("maxTransformsPerApp", &MAX_TRANSFORMS_PER_APP.to_string(), "Keep-last transform records per app."),
        ],
        compatibility: vec![
            "media.transformed.dest_hash is the stable result identity; encoder choice is not replayed.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Edge split".to_string(),
                body: "terrane-cap-media validates and folds metadata; terrane-host decodes, transforms, encodes, and writes CAS bytes.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}
