use super::{
    OperationCatalogEntry, OP_CLIPBOARD_WRITE_TEXT, OP_DIALOG_OPEN_FILE, OP_EXTERNAL_OPEN_URL,
    OP_NOTIFICATION_SHOW, RESULT_SIZE_INLINE_SMALL, RESULT_SIZE_NONE,
};

pub const GROUP: &str = "common";

pub const CATALOG: &[OperationCatalogEntry] = &[
    OperationCatalogEntry {
        id: OP_CLIPBOARD_WRITE_TEXT,
        group: GROUP,
        status: "v1",
        safety: "safe-request",
        policy: "grant-gated",
        result_size: RESULT_SIZE_NONE,
        summary: "Write a bounded text string to the system clipboard.",
    },
    OperationCatalogEntry {
        id: OP_EXTERNAL_OPEN_URL,
        group: GROUP,
        status: "v1",
        safety: "safe-request",
        policy: "grant-gated",
        result_size: RESULT_SIZE_NONE,
        summary: "Ask the host to open an external URL through the OS.",
    },
    OperationCatalogEntry {
        id: OP_NOTIFICATION_SHOW,
        group: GROUP,
        status: "v1",
        safety: "user-mediated",
        policy: "grant-gated",
        result_size: RESULT_SIZE_NONE,
        summary: "Show a local notification if the OS/user permits it.",
    },
    OperationCatalogEntry {
        id: OP_DIALOG_OPEN_FILE,
        group: GROUP,
        status: "v1",
        safety: "user-mediated",
        policy: "grant-gated",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary: "Open a native file picker and record bounded selected path metadata.",
    },
    OperationCatalogEntry {
        id: "secureStorage.get",
        group: GROUP,
        status: "planned",
        safety: "sensitive",
        policy: "refuse-until-selector",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary: "Access OS secure storage through a future operation-level selector.",
    },
    OperationCatalogEntry {
        id: "permission.request",
        group: GROUP,
        status: "planned",
        safety: "user-mediated",
        policy: "trusted-only",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary: "Record an OS permission request and replayable outcome.",
    },
];
