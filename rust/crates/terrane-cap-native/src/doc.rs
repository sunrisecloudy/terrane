use terrane_cap_interface::{
    command_doc, event_doc, param, query_doc, resource_method, CapabilityDoc,
    CapabilityManifestDoc, CommandDoc, EventDoc, ExampleDoc, InternalNote, LimitDoc, QueryDoc,
    ResourceDoc, SchemaDoc,
};

use crate::operations::{
    AUDIO_RECORD, CAMERA_CAPTURE_PHOTO, CANCEL, CLIPBOARD_READ_TEXT, CLIPBOARD_WRITE_TEXT,
    COMPLETE, DIALOG_OPEN_FILE, DIALOG_SAVE_FILE, EXTERNAL_OPEN_URL, FAIL, NOTIFICATION_SHOW,
    PLATFORM_OBSERVE, RESOURCE_AUDIO_RECORD, RESOURCE_CAMERA_CAPTURE_PHOTO,
    RESOURCE_CLIPBOARD_READ_TEXT, RESOURCE_CLIPBOARD_WRITE_TEXT, RESOURCE_DIALOG_OPEN_FILE,
    RESOURCE_DIALOG_SAVE_FILE, RESOURCE_EXTERNAL_OPEN_URL, RESOURCE_NOTIFICATION_SHOW,
    RESOURCE_SCREEN_CAPTURE, RESOURCE_SHORTCUT_REGISTER_GLOBAL, RESOURCE_TRAY_SET_MENU,
    RESOURCE_WINDOW_CONTROL, SCREEN_CAPTURE, SHORTCUT_REGISTER_GLOBAL, TRAY_SET_MENU,
    WINDOW_CONTROL,
};

pub fn native_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "native".to_string(),
        title: "Native OS Requests".to_string(),
        summary: "Async, replay-safe native OS request queue for app backends.".to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                PLATFORM_OBSERVE.to_string(),
                CLIPBOARD_WRITE_TEXT.to_string(),
                CLIPBOARD_READ_TEXT.to_string(),
                EXTERNAL_OPEN_URL.to_string(),
                NOTIFICATION_SHOW.to_string(),
                DIALOG_OPEN_FILE.to_string(),
                DIALOG_SAVE_FILE.to_string(),
                CAMERA_CAPTURE_PHOTO.to_string(),
                AUDIO_RECORD.to_string(),
                SCREEN_CAPTURE.to_string(),
                TRAY_SET_MENU.to_string(),
                SHORTCUT_REGISTER_GLOBAL.to_string(),
                WINDOW_CONTROL.to_string(),
                "native.clipboardWriteText".to_string(),
                "native.clipboardReadText".to_string(),
                "native.externalOpenUrl".to_string(),
                "native.notificationShow".to_string(),
                "native.dialogOpenFile".to_string(),
                "native.dialogSaveFile".to_string(),
                "native.cameraCapturePhoto".to_string(),
                "native.audioRecord".to_string(),
                "native.screenCapture".to_string(),
                "native.traySetMenu".to_string(),
                "native.shortcutRegisterGlobal".to_string(),
                "native.windowControl".to_string(),
                COMPLETE.to_string(),
                FAIL.to_string(),
                CANCEL.to_string(),
            ],
            queries: vec!["native.supports".to_string()],
            events: vec![
                "native.platform.observed".to_string(),
                "native.requested".to_string(),
                "native.completed".to_string(),
                "native.failed".to_string(),
                "native.cancelled".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: resource_methods(),
        },
        commands: commands(),
        queries: queries(),
        events: events(),
        resources: resources(),
        schemas: Vec::<SchemaDoc>::new(),
        examples: examples(),
        constraints: vec![
            "Native operations requested by apps are asynchronous; results are read on a later invoke."
                .to_string(),
            "The operation catalog is grouped into common, desktop, and mobile rows; v1 common/desktop rows are registered as app-callable queued requests."
                .to_string(),
            "Resource methods record native.requested only; OS work runs in a trusted host drain service."
                .to_string(),
            "native.supports reads folded platform observation state and never probes a live connector."
                .to_string(),
            "Large native bytes are not stored inline; camera.capturePhoto, audio.record, and screen.capture complete with blob CAS references."
                .to_string(),
            "clipboard.readText, camera.capturePhoto, audio.record, and screen.capture require operation-level native grants such as native:camera.capturePhoto."
                .to_string(),
        ],
        limits: vec![
            LimitDoc {
                name: "resultSize".to_string(),
                value: "none | inline-small | blob-ref".to_string(),
                reason: "The event log stores bounded JSON facts; captured media bytes live in the blob CAS."
                    .to_string(),
            },
            LimitDoc {
                name: "terminalRetention".to_string(),
                value: "100 per app".to_string(),
                reason: "Folded state keeps recent terminal native results; older events still replay deterministically."
                    .to_string(),
            },
        ],
        compatibility: vec![
            "native.requested payloads include executor_host_id and origin_replica to avoid future cross-replica double execution."
                .to_string(),
            "Trusted terminal commands refuse non-pending requests; fold keeps first terminal status."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "Queue instead of Effect".to_string(),
                body: "App-originated native work uses requested/completed facts because runtime resource writes cannot return Decision::Effect and user-mediated OS work may block."
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
            PLATFORM_OBSERVE,
            &[
                param("hostId", "Stable host/connector id.", "string"),
                param(
                    "platform",
                    "Platform label such as macos or windows.",
                    "string",
                ),
                param("connectorVersion", "Native connector version.", "string"),
                param(
                    "operations",
                    "Supported operation ids as trailing args.",
                    "string[]",
                ),
            ],
            "commit",
            "Trusted host records native platform support.",
        )
        .with_errors(&[
            "missing host id",
            "missing platform",
            "missing connector version",
        ])
        .with_emits(&["native.platform.observed"]),
        request_command(CLIPBOARD_WRITE_TEXT, "Request clipboard text write."),
        request_command(CLIPBOARD_READ_TEXT, "Request clipboard text read."),
        request_command(EXTERNAL_OPEN_URL, "Request opening an external URL."),
        request_command(NOTIFICATION_SHOW, "Request a local notification."),
        request_command(DIALOG_OPEN_FILE, "Request a native open-file dialog."),
        request_command(DIALOG_SAVE_FILE, "Request a user-mediated save-file dialog."),
        request_command(CAMERA_CAPTURE_PHOTO, "Request a camera photo into the blob CAS."),
        request_command(AUDIO_RECORD, "Request bounded microphone audio into the blob CAS."),
        request_command(SCREEN_CAPTURE, "Request screen/window capture into the blob CAS."),
        request_command(TRAY_SET_MENU, "Request installation of an app tray menu."),
        request_command(
            SHORTCUT_REGISTER_GLOBAL,
            "Request registration of an app global shortcut.",
        ),
        request_command(WINDOW_CONTROL, "Request control of the app's own shell window."),
        request_command(
            RESOURCE_CLIPBOARD_WRITE_TEXT,
            "Resource alias used by ctx.resource.native.clipboardWriteText.",
        ),
        request_command(
            RESOURCE_CLIPBOARD_READ_TEXT,
            "Resource alias used by ctx.resource.native.clipboardReadText.",
        ),
        request_command(
            RESOURCE_EXTERNAL_OPEN_URL,
            "Resource alias used by ctx.resource.native.externalOpenUrl.",
        ),
        request_command(
            RESOURCE_NOTIFICATION_SHOW,
            "Resource alias used by ctx.resource.native.notificationShow.",
        ),
        request_command(
            RESOURCE_DIALOG_OPEN_FILE,
            "Resource alias used by ctx.resource.native.dialogOpenFile.",
        ),
        request_command(
            RESOURCE_DIALOG_SAVE_FILE,
            "Resource alias used by ctx.resource.native.dialogSaveFile.",
        ),
        request_command(
            RESOURCE_CAMERA_CAPTURE_PHOTO,
            "Resource alias used by ctx.resource.native.cameraCapturePhoto.",
        ),
        request_command(
            RESOURCE_AUDIO_RECORD,
            "Resource alias used by ctx.resource.native.audioRecord.",
        ),
        request_command(
            RESOURCE_SCREEN_CAPTURE,
            "Resource alias used by ctx.resource.native.screenCapture.",
        ),
        request_command(
            RESOURCE_TRAY_SET_MENU,
            "Resource alias used by ctx.resource.native.traySetMenu.",
        ),
        request_command(
            RESOURCE_SHORTCUT_REGISTER_GLOBAL,
            "Resource alias used by ctx.resource.native.shortcutRegisterGlobal.",
        ),
        request_command(
            RESOURCE_WINDOW_CONTROL,
            "Resource alias used by ctx.resource.native.windowControl.",
        ),
        command_doc(
            COMPLETE,
            &[
                param("app", "App id.", "app_id"),
                param("requestId", "Native request id.", "string"),
                param("resultJson", "Bounded JSON result.", "json"),
            ],
            "commit",
            "Trusted host records successful native completion.",
        )
        .with_errors(&["unknown request", "request not pending", "invalid json"])
        .with_emits(&["native.completed"]),
        command_doc(
            FAIL,
            &[
                param("app", "App id.", "app_id"),
                param("requestId", "Native request id.", "string"),
                param("errorJson", "Bounded JSON failure.", "json"),
            ],
            "commit",
            "Trusted host records native failure.",
        )
        .with_errors(&["unknown request", "request not pending", "invalid json"])
        .with_emits(&["native.failed"]),
        command_doc(
            CANCEL,
            &[
                param("app", "App id.", "app_id"),
                param("requestId", "Native request id.", "string"),
                param("reason", "Cancellation reason.", "string"),
            ],
            "commit",
            "Trusted host records native cancellation.",
        )
        .with_errors(&["unknown request", "request not pending"])
        .with_emits(&["native.cancelled"]),
    ]
}

fn request_command(name: &str, summary: &str) -> CommandDoc {
    command_doc(
        name,
        &[
            param("app", "Existing app id.", "app_id"),
            param(
                "requestId",
                "Caller-provided idempotency/request id.",
                "string",
            ),
            param("payload", "Operation-specific argument.", "string"),
        ],
        "commit",
        summary,
    )
    .with_errors(&[
        "app not found",
        "native platform not observed",
        "operation unsupported",
        "duplicate request id",
    ])
    .with_emits(&["native.requested"])
}

fn queries() -> Vec<QueryDoc> {
    vec![query_doc(
        "native.supports",
        &[param("operationId", "Operation id to check.", "string")],
        "bool",
        "Read folded platform state to check whether the active host supports an operation.",
    )
    .with_errors(&["missing operation id", "unknown query"])]
}

fn events() -> Vec<EventDoc> {
    vec![
        event_doc(
            "native.platform.observed",
            &[param("hostId", "Trusted host id.", "string")],
            "Records native connector support for a host.",
        ),
        event_doc(
            "native.requested",
            &[param("requestId", "Native request id.", "string")],
            "Records an app-originated native request.",
        ),
        event_doc(
            "native.completed",
            &[param("requestId", "Native request id.", "string")],
            "Records a bounded native result.",
        ),
        event_doc(
            "native.failed",
            &[param("requestId", "Native request id.", "string")],
            "Records a native failure.",
        ),
        event_doc(
            "native.cancelled",
            &[param("requestId", "Native request id.", "string")],
            "Records native request cancellation.",
        ),
    ]
}

fn resources() -> Vec<ResourceDoc> {
    vec![ResourceDoc {
        namespace: "native".to_string(),
        summary: "Async native OS request methods.".to_string(),
        methods: resource_methods(),
    }]
}

fn resource_methods() -> Vec<terrane_cap_interface::ResourceMethodDoc> {
    vec![
        native_resource_method(
            "clipboardWriteText",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("text", "Clipboard text.", "string"),
            ],
            "Record a clipboard write request.",
            "void",
        ),
        native_resource_method(
            "clipboardReadText",
            "write",
            &[param("requestId", "Native request id.", "string")],
            "Record a sensitive clipboard read request.",
            "void",
        ),
        native_resource_method(
            "externalOpenUrl",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("url", "External URL.", "url"),
            ],
            "Record an external URL open request.",
            "void",
        ),
        native_resource_method(
            "notificationShow",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("title", "Notification title.", "string"),
                param("body", "Notification body.", "string"),
            ],
            "Record a local notification request.",
            "void",
        ),
        native_resource_method(
            "dialogOpenFile",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("optionsJson", "Open-file options JSON.", "json"),
            ],
            "Record an open-file dialog request.",
            "void",
        ),
        native_resource_method(
            "dialogSaveFile",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("suggestedName", "Suggested file name.", "string"),
                param("blobName", "Blob CAS name to save.", "string"),
            ],
            "Record a save-file dialog request.",
            "void",
        ),
        native_resource_method(
            "cameraCapturePhoto",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("inputJson", "JSON {facing?, maxWidth?}.", "json"),
            ],
            "Record a sensitive camera photo request.",
            "void",
        ),
        native_resource_method(
            "audioRecord",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("inputJson", "JSON {maxDurationMs, sampleRateHz?}.", "json"),
            ],
            "Record a sensitive microphone recording request.",
            "void",
        ),
        native_resource_method(
            "screenCapture",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("target", "screen or window.", "string"),
            ],
            "Record a sensitive screen capture request.",
            "void",
        ),
        native_resource_method(
            "traySetMenu",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("title", "Tray menu title.", "string"),
                param("itemsJson", "JSON array of {id,label}.", "json"),
            ],
            "Record a durable tray menu registration request.",
            "void",
        ),
        native_resource_method(
            "shortcutRegisterGlobal",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("accelerator", "Shortcut accelerator.", "string"),
                param("verb", "App verb to dispatch.", "string"),
            ],
            "Record a durable global shortcut registration request.",
            "void",
        ),
        native_resource_method(
            "windowControl",
            "write",
            &[
                param("requestId", "Native request id.", "string"),
                param("action", "focus, minimize, or setTitle.", "string"),
                param("title", "Required only for setTitle.", "string"),
            ],
            "Record an own-window control request.",
            "void",
        ),
        native_resource_method(
            "result",
            "read",
            &[param("requestId", "Native request id.", "string")],
            "Read a recorded native result if available.",
            "string | null",
        ),
        native_resource_method(
            "pending",
            "read",
            &[],
            "List pending request ids.",
            "string[]",
        ),
    ]
}

fn native_resource_method(
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

fn examples() -> Vec<ExampleDoc> {
    vec![ExampleDoc {
        title: "Request and later read".to_string(),
        summary: "Native work is asynchronous for app backends.".to_string(),
        language: "js".to_string(),
        code: "ctx.resource.native.externalOpenUrl('req-1', 'https://example.com');\nreturn 'pending:req-1';\n// Later: ctx.resource.native.result('req-1')".to_string(),
        expected: "First invoke records native.requested; a trusted drain records the terminal result."
            .to_string(),
    }]
}
