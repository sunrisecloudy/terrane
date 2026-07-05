use terrane_cap_interface::{
    command_doc, event_doc, limit, param, query_doc, resource_method, CapabilityDoc,
    CapabilityManifestDoc, ExampleDoc, InternalNote, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};

fn resource_methods() -> Vec<ResourceMethodDoc> {
    let mut send = resource_method(
        "send",
        "call",
        &[param("messageJson", "Channel-shaped outbound message JSON.", "json")],
        "Validate and record an outbound channel send through the host edge.",
    );
    send.returns = "JSON { message_id, status, error? }".to_string();
    let mut status = resource_method(
        "status",
        "read",
        &[param("messageId", "Recorded message id.", "string")],
        "Read redacted folded send metadata.",
    );
    status.returns = "JSON status metadata, or null".to_string();
    let mut channels = resource_method(
        "channels",
        "read",
        &[],
        "List host-configured channels and whether the app has the channel grant.",
    );
    channels.returns = "JSON object keyed by channel".to_string();
    vec![send, status, channels]
}

pub fn common_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "common".to_string(),
        title: "Common Outbound Messaging".to_string(),
        summary: "Recorded outbound messaging by channel. Email is the first channel; bodies stay hash-only unless the app opts into recording.".to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec!["common.send".to_string()],
            queries: vec!["common.channels".to_string()],
            events: vec!["common.sent".to_string()],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: resource_methods(),
        },
        commands: vec![command_doc(
            "common.send",
            &[
                param("app", "Existing app id.", "app_id"),
                param("message_json", "Email message JSON with channel=email.", "json"),
            ],
            "effect",
            "Validate, canonicalize, and run a recorded channel-send effect.",
        )
        .with_errors(&[
            "app not found",
            "unknown channel",
            "missing common:send:email grant",
            "missing connection grant",
            "recipient or size limit exceeded",
            "rate limit exceeded",
        ])
        .with_effects(&["ChannelSend"])
        .with_emits(&["common.sent"])],
        queries: vec![query_doc(
            "common.channels",
            &[param("app", "App id to evaluate grants for.", "app_id")],
            "JSON object keyed by channel",
            "List configured common channels and channel-grant status.",
        )
        .with_errors(&["invalid app id", "state unavailable"])],
        events: vec![event_doc(
            "common.sent",
            &[
                param("app", "Sending app id.", "app_id"),
                param("channel", "Channel name, email in v1.", "string"),
                param("message_id", "Edge-minted provider/message id.", "string"),
                param("to", "Visible recipients.", "stringArray"),
                param("subject", "Optional subject.", "string"),
                param("body_hash", "sha256 over text plus optional html.", "sha256"),
                param("body_kind", "none, inline, or blob.", "string"),
                param("attachments", "Attachment blob metadata.", "jsonArray"),
                param("status", "sent or failed.", "string"),
                param("sent_at", "Edge-observed Unix seconds.", "u64String"),
            ],
            "Redacted outcome of one outbound send attempt.",
        )],
        resources: vec![ResourceDoc {
            namespace: "common".to_string(),
            summary: "Outbound channel messaging for app backends.".to_string(),
            methods: resource_methods(),
        }],
        schemas: vec![SchemaDoc {
            id: "common.emailMessage.v1".to_string(),
            title: "Email channel message".to_string(),
            media_type: "application/json".to_string(),
            schema_json: r#"{"type":"object","required":["channel","to","text"],"properties":{"channel":{"const":"email"},"to":{"type":"array","items":{"type":"string"}},"cc":{"type":"array","items":{"type":"string"}},"bcc":{"type":"array","items":{"type":"string"}},"subject":{"type":"string","maxLength":998},"text":{"type":"string"},"html":{"type":"string"},"attachments":{"type":"array","items":{"type":"string"}},"recordBody":{"type":"boolean"},"connection":{"type":"string"}}}"#.to_string(),
            public: true,
        }],
        examples: vec![ExampleDoc {
            title: "Send email".to_string(),
            summary: "A granted app sends through the default SMTP connection.".to_string(),
            language: "js".to_string(),
            code: r#"ctx.resource.common.send(JSON.stringify({channel:"email",to:["a@example.com"],subject:"Hi",text:"Hello"}));"#.to_string(),
            expected: "records common.sent with recipients, subject, body_hash, attachment refs, and status".to_string(),
        }],
        constraints: vec![
            "Channel sends are recorded effects; replay folds common.sent and never sends again.".to_string(),
            "Credentials are resolved through connection at the edge and never appear in events.".to_string(),
            "The channel grant is common:send:email; a namespace grant alone is not enough to send.".to_string(),
            "Message bodies are hash-only by default; recordBody=true records inline only up to 256 KiB, otherwise records a body blob hash.".to_string(),
            "Attachments are blob refs and never inline bytes in common.sent.".to_string(),
            "Failed sends still fold as common.sent status=failed and count as attempts.".to_string(),
        ],
        limits: vec![
            limit("emailRecipientsPerMessage", &super::MAX_EMAIL_RECIPIENTS.to_string(), "Total to/cc/bcc recipients."),
            limit("emailSubjectChars", &super::MAX_EMAIL_SUBJECT_CHARS.to_string(), "SMTP subject line ceiling."),
            limit("emailTextBytes", &super::MAX_EMAIL_TEXT_BYTES.to_string(), "Text part limit."),
            limit("emailHtmlBytes", &super::MAX_EMAIL_HTML_BYTES.to_string(), "HTML part limit."),
            limit("emailAttachments", &super::MAX_EMAIL_ATTACHMENTS.to_string(), "Attachment count limit."),
            limit("emailAttachmentBytesTotal", &super::MAX_EMAIL_ATTACHMENT_BYTES.to_string(), "Total attachment byte limit."),
            limit("emailSendsPerHour", &super::MAX_EMAIL_SENDS_PER_HOUR.to_string(), "Per-app channel send attempts in a one-hour window."),
            limit("emailSendsPerDay", &super::MAX_EMAIL_SENDS_PER_DAY.to_string(), "Per-app channel send attempts in a one-day window."),
        ],
        compatibility: vec![
            "common.receive remains the interop inbound app verb; this capability only implements common.send.".to_string(),
            "Future channels add schemas and transports behind the same common.send/common.sent envelope.".to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Rate-limit clock".to_string(),
                body: "Decide is pure, so rate-limit windows are computed from folded sent_at values and an optional deterministic sentAt test field; the edge records the actual sent_at outcome.".to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}
