use super::{OperationCatalogEntry, RESULT_SIZE_INLINE_SMALL};

pub const GROUP: &str = "mobile";

pub const CATALOG: &[OperationCatalogEntry] = &[
    OperationCatalogEntry {
        id: "camera.capture",
        group: GROUP,
        status: "planned",
        safety: "sensitive",
        policy: "refuse-until-selector",
        result_size: "blob-ref",
        summary: "Potential future camera capture using OS permission prompts and blob references.",
    },
    OperationCatalogEntry {
        id: "media.pick",
        group: GROUP,
        status: "planned",
        safety: "user-mediated",
        policy: "refuse-until-selector",
        result_size: "blob-ref",
        summary: "Potential future mobile media picker with bounded metadata in the log.",
    },
    OperationCatalogEntry {
        id: "haptics.impact",
        group: GROUP,
        status: "planned",
        safety: "safe-request",
        policy: "grant-gated",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary: "Potential future haptic feedback request on mobile hosts.",
    },
    OperationCatalogEntry {
        id: "store.purchase",
        group: GROUP,
        status: "not-operation",
        safety: "release-tooling",
        policy: "not-command",
        result_size: RESULT_SIZE_INLINE_SMALL,
        summary:
            "In-app purchase/store flows stay out until product and trust boundaries are designed.",
    },
];
