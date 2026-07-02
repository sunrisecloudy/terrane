pub mod common;
pub mod desktop;
pub mod mobile;

pub const PLATFORM_OBSERVE: &str = "native.platform.observe";
pub const COMPLETE: &str = "native.complete";
pub const FAIL: &str = "native.fail";
pub const CANCEL: &str = "native.cancel";

pub const CLIPBOARD_WRITE_TEXT: &str = "native.clipboard.write-text";
pub const EXTERNAL_OPEN_URL: &str = "native.external.open-url";
pub const NOTIFICATION_SHOW: &str = "native.notification.show";
pub const DIALOG_OPEN_FILE: &str = "native.dialog.open-file";

pub const RESOURCE_CLIPBOARD_WRITE_TEXT: &str = "native.clipboardWriteText";
pub const RESOURCE_EXTERNAL_OPEN_URL: &str = "native.externalOpenUrl";
pub const RESOURCE_NOTIFICATION_SHOW: &str = "native.notificationShow";
pub const RESOURCE_DIALOG_OPEN_FILE: &str = "native.dialogOpenFile";

pub const OP_CLIPBOARD_WRITE_TEXT: &str = "clipboard.writeText";
pub const OP_EXTERNAL_OPEN_URL: &str = "external.openUrl";
pub const OP_NOTIFICATION_SHOW: &str = "notification.show";
pub const OP_DIALOG_OPEN_FILE: &str = "dialog.openFile";

pub const RESULT_SIZE_NONE: &str = "none";
pub const RESULT_SIZE_INLINE_SMALL: &str = "inline-small";
pub const RETENTION_KEEP_LAST: &str = "keep-last";

pub const DEFAULT_TERMINAL_RETAIN: usize = 100;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperationCatalogEntry {
    pub id: &'static str,
    pub group: &'static str,
    pub status: &'static str,
    pub safety: &'static str,
    pub policy: &'static str,
    pub result_size: &'static str,
    pub summary: &'static str,
}

pub fn operation_catalog() -> Vec<OperationCatalogEntry> {
    let mut out = Vec::new();
    out.extend_from_slice(common::CATALOG);
    out.extend_from_slice(desktop::CATALOG);
    out.extend_from_slice(mobile::CATALOG);
    out
}

pub fn app_callable_commands() -> &'static [&'static str] {
    &[
        CLIPBOARD_WRITE_TEXT,
        EXTERNAL_OPEN_URL,
        NOTIFICATION_SHOW,
        DIALOG_OPEN_FILE,
        RESOURCE_CLIPBOARD_WRITE_TEXT,
        RESOURCE_EXTERNAL_OPEN_URL,
        RESOURCE_NOTIFICATION_SHOW,
        RESOURCE_DIALOG_OPEN_FILE,
    ]
}

pub fn trusted_commands() -> &'static [&'static str] {
    &[PLATFORM_OBSERVE, COMPLETE, FAIL, CANCEL]
}

pub fn operation_for_command(name: &str) -> Option<&'static str> {
    match name {
        CLIPBOARD_WRITE_TEXT | RESOURCE_CLIPBOARD_WRITE_TEXT => Some(OP_CLIPBOARD_WRITE_TEXT),
        EXTERNAL_OPEN_URL | RESOURCE_EXTERNAL_OPEN_URL => Some(OP_EXTERNAL_OPEN_URL),
        NOTIFICATION_SHOW | RESOURCE_NOTIFICATION_SHOW => Some(OP_NOTIFICATION_SHOW),
        DIALOG_OPEN_FILE | RESOURCE_DIALOG_OPEN_FILE => Some(OP_DIALOG_OPEN_FILE),
        _ => None,
    }
}

pub fn result_size_for_operation(operation_id: &str) -> &'static str {
    match operation_id {
        OP_CLIPBOARD_WRITE_TEXT | OP_EXTERNAL_OPEN_URL | OP_NOTIFICATION_SHOW => RESULT_SIZE_NONE,
        OP_DIALOG_OPEN_FILE => RESULT_SIZE_INLINE_SMALL,
        _ => RESULT_SIZE_INLINE_SMALL,
    }
}

pub fn default_supported_operations() -> Vec<String> {
    common::CATALOG
        .iter()
        .filter(|entry| entry.status == "v1")
        .map(|entry| entry.id.to_string())
        .collect()
}
