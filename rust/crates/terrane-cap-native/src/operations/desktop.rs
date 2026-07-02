use super::{OperationCatalogEntry, RESULT_SIZE_INLINE_SMALL};

pub const GROUP: &str = "desktop";

pub const CATALOG: &[OperationCatalogEntry] = &[
    OperationCatalogEntry {
        id: "tray.setMenu",
        group: GROUP,
        status: "planned",
        safety: "host-plumbing",
        policy: "not-command",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary: "Host shell chrome, not a v1 app-originated native operation.",
    },
    OperationCatalogEntry {
        id: "screen.capture",
        group: GROUP,
        status: "planned",
        safety: "sensitive",
        policy: "refuse-until-selector",
        result_size: "blob-ref",
        summary: "Potential future screen capture using a blob reference, not inline event data.",
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
