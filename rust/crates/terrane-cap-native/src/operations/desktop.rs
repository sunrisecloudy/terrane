use super::{
    OperationCatalogEntry, OP_SCREEN_CAPTURE, OP_SHORTCUT_REGISTER_GLOBAL, OP_TRAY_SET_MENU,
    OP_WINDOW_CONTROL, RESULT_SIZE_BLOB_REF, RESULT_SIZE_INLINE_SMALL,
};

pub const GROUP: &str = "desktop";

pub const CATALOG: &[OperationCatalogEntry] = &[
    OperationCatalogEntry {
        id: OP_TRAY_SET_MENU,
        group: GROUP,
        status: "v1",
        safety: "host-plumbing",
        policy: "grant-gated",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary: "Install or replace the app's durable tray menu registration.",
    },
    OperationCatalogEntry {
        id: OP_SCREEN_CAPTURE,
        group: GROUP,
        status: "v1",
        safety: "sensitive",
        policy: "refuse-until-selector",
        result_size: RESULT_SIZE_BLOB_REF,
        summary: "Capture screen/window pixels into the blob CAS and complete with a blob reference.",
    },
    OperationCatalogEntry {
        id: OP_SHORTCUT_REGISTER_GLOBAL,
        group: GROUP,
        status: "v1",
        safety: "host-plumbing",
        policy: "grant-gated",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary: "Register one app-owned global shortcut that dispatches an app verb at the edge.",
    },
    OperationCatalogEntry {
        id: OP_WINDOW_CONTROL,
        group: GROUP,
        status: "v1",
        safety: "safe-request",
        policy: "grant-gated",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary: "Control only the requesting app's own shell window.",
    },
    OperationCatalogEntry {
        id: "shell.openPath",
        group: GROUP,
        status: "planned",
        safety: "sensitive",
        policy: "refuse-until-selector",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary: "Open a local path only after operation-level selectors and path policy exist.",
    },
    OperationCatalogEntry {
        id: "release.sign",
        group: GROUP,
        status: "not-operation",
        safety: "release-tooling",
        policy: "not-command",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary: "Packaging/signing remains release tooling outside app runtime capabilities.",
    },
];
