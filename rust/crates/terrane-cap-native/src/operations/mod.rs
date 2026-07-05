pub mod common;
pub mod desktop;
pub mod mobile;

pub const PLATFORM_OBSERVE: &str = "native.platform.observe";
pub const COMPLETE: &str = "native.complete";
pub const FAIL: &str = "native.fail";
pub const CANCEL: &str = "native.cancel";

pub const CLIPBOARD_WRITE_TEXT: &str = "native.clipboard.write-text";
pub const CLIPBOARD_READ_TEXT: &str = "native.clipboard.read-text";
pub const EXTERNAL_OPEN_URL: &str = "native.external.open-url";
pub const NOTIFICATION_SHOW: &str = "native.notification.show";
pub const DIALOG_OPEN_FILE: &str = "native.dialog.open-file";
pub const DIALOG_SAVE_FILE: &str = "native.dialog.save-file";
pub const SCREEN_CAPTURE: &str = "native.screen.capture";
pub const TRAY_SET_MENU: &str = "native.tray.set-menu";
pub const SHORTCUT_REGISTER_GLOBAL: &str = "native.shortcut.register-global";
pub const WINDOW_CONTROL: &str = "native.window.control";

pub const RESOURCE_CLIPBOARD_WRITE_TEXT: &str = "native.clipboardWriteText";
pub const RESOURCE_CLIPBOARD_READ_TEXT: &str = "native.clipboardReadText";
pub const RESOURCE_EXTERNAL_OPEN_URL: &str = "native.externalOpenUrl";
pub const RESOURCE_NOTIFICATION_SHOW: &str = "native.notificationShow";
pub const RESOURCE_DIALOG_OPEN_FILE: &str = "native.dialogOpenFile";
pub const RESOURCE_DIALOG_SAVE_FILE: &str = "native.dialogSaveFile";
pub const RESOURCE_SCREEN_CAPTURE: &str = "native.screenCapture";
pub const RESOURCE_TRAY_SET_MENU: &str = "native.traySetMenu";
pub const RESOURCE_SHORTCUT_REGISTER_GLOBAL: &str = "native.shortcutRegisterGlobal";
pub const RESOURCE_WINDOW_CONTROL: &str = "native.windowControl";

pub const OP_CLIPBOARD_WRITE_TEXT: &str = "clipboard.writeText";
pub const OP_CLIPBOARD_READ_TEXT: &str = "clipboard.readText";
pub const OP_EXTERNAL_OPEN_URL: &str = "external.openUrl";
pub const OP_NOTIFICATION_SHOW: &str = "notification.show";
pub const OP_DIALOG_OPEN_FILE: &str = "dialog.openFile";
pub const OP_DIALOG_SAVE_FILE: &str = "dialog.saveFile";
pub const OP_SCREEN_CAPTURE: &str = "screen.capture";
pub const OP_TRAY_SET_MENU: &str = "tray.setMenu";
pub const OP_SHORTCUT_REGISTER_GLOBAL: &str = "shortcut.registerGlobal";
pub const OP_WINDOW_CONTROL: &str = "window.control";

pub const RESULT_SIZE_NONE: &str = "none";
pub const RESULT_SIZE_INLINE_SMALL: &str = "inline-small";
pub const RESULT_SIZE_BLOB_REF: &str = "blob-ref";
pub const RETENTION_KEEP_LAST: &str = "keep-last";

pub const DEFAULT_TERMINAL_RETAIN: usize = 100;
pub const MAX_CLIPBOARD_TEXT_BYTES: usize = 256 * 1024;
pub const MAX_TRAY_ITEMS: usize = 20;
pub const MAX_TRAY_LABEL_CHARS: usize = 64;
pub const MAX_SHORTCUTS_PER_APP: usize = 5;
pub const NATIVE_OPERATION_SELECTOR_SCHEMA_ID: &str = "native.operation.v1";
pub const NATIVE_OPERATION_SELECTOR_SCHEMA_JSON: &str =
    r#"{"type":"object","required":["namespace","operation"],"properties":{"namespace":{"const":"native"},"operation":{"type":"string"}},"additionalProperties":false}"#;

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
        CLIPBOARD_READ_TEXT,
        EXTERNAL_OPEN_URL,
        NOTIFICATION_SHOW,
        DIALOG_OPEN_FILE,
        DIALOG_SAVE_FILE,
        SCREEN_CAPTURE,
        TRAY_SET_MENU,
        SHORTCUT_REGISTER_GLOBAL,
        WINDOW_CONTROL,
        RESOURCE_CLIPBOARD_WRITE_TEXT,
        RESOURCE_CLIPBOARD_READ_TEXT,
        RESOURCE_EXTERNAL_OPEN_URL,
        RESOURCE_NOTIFICATION_SHOW,
        RESOURCE_DIALOG_OPEN_FILE,
        RESOURCE_DIALOG_SAVE_FILE,
        RESOURCE_SCREEN_CAPTURE,
        RESOURCE_TRAY_SET_MENU,
        RESOURCE_SHORTCUT_REGISTER_GLOBAL,
        RESOURCE_WINDOW_CONTROL,
    ]
}

pub fn trusted_commands() -> &'static [&'static str] {
    &[PLATFORM_OBSERVE, COMPLETE, FAIL, CANCEL]
}

pub fn operation_for_command(name: &str) -> Option<&'static str> {
    match name {
        CLIPBOARD_WRITE_TEXT | RESOURCE_CLIPBOARD_WRITE_TEXT => Some(OP_CLIPBOARD_WRITE_TEXT),
        CLIPBOARD_READ_TEXT | RESOURCE_CLIPBOARD_READ_TEXT => Some(OP_CLIPBOARD_READ_TEXT),
        EXTERNAL_OPEN_URL | RESOURCE_EXTERNAL_OPEN_URL => Some(OP_EXTERNAL_OPEN_URL),
        NOTIFICATION_SHOW | RESOURCE_NOTIFICATION_SHOW => Some(OP_NOTIFICATION_SHOW),
        DIALOG_OPEN_FILE | RESOURCE_DIALOG_OPEN_FILE => Some(OP_DIALOG_OPEN_FILE),
        DIALOG_SAVE_FILE | RESOURCE_DIALOG_SAVE_FILE => Some(OP_DIALOG_SAVE_FILE),
        SCREEN_CAPTURE | RESOURCE_SCREEN_CAPTURE => Some(OP_SCREEN_CAPTURE),
        TRAY_SET_MENU | RESOURCE_TRAY_SET_MENU => Some(OP_TRAY_SET_MENU),
        SHORTCUT_REGISTER_GLOBAL | RESOURCE_SHORTCUT_REGISTER_GLOBAL => {
            Some(OP_SHORTCUT_REGISTER_GLOBAL)
        }
        WINDOW_CONTROL | RESOURCE_WINDOW_CONTROL => Some(OP_WINDOW_CONTROL),
        _ => None,
    }
}

pub fn result_size_for_operation(operation_id: &str) -> &'static str {
    match operation_id {
        OP_CLIPBOARD_WRITE_TEXT | OP_EXTERNAL_OPEN_URL | OP_NOTIFICATION_SHOW => RESULT_SIZE_NONE,
        OP_SCREEN_CAPTURE => RESULT_SIZE_BLOB_REF,
        OP_DIALOG_OPEN_FILE
        | OP_CLIPBOARD_READ_TEXT
        | OP_DIALOG_SAVE_FILE
        | OP_TRAY_SET_MENU
        | OP_SHORTCUT_REGISTER_GLOBAL
        | OP_WINDOW_CONTROL => RESULT_SIZE_INLINE_SMALL,
        _ => RESULT_SIZE_INLINE_SMALL,
    }
}

pub fn default_supported_operations() -> Vec<String> {
    operation_catalog()
        .iter()
        .filter(|entry| entry.status == "v1")
        .map(|entry| entry.id.to_string())
        .collect()
}
