use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};

use crate::operations::DEFAULT_TERMINAL_RETAIN;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeState {
    pub active_host_id: Option<String>,
    pub platforms: BTreeMap<String, NativePlatformObservation>,
    pub requests: BTreeMap<String, BTreeMap<String, NativeRequestRecord>>,
    pub tray_menus: BTreeMap<String, TrayMenu>,
    pub shortcuts: BTreeMap<String, BTreeMap<String, GlobalShortcut>>,
    pub next_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativePlatformObservation {
    pub host_id: String,
    pub platform: String,
    pub connector_version: String,
    pub supported_operations: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NativeRequestRecord {
    pub request_id: String,
    pub app: String,
    pub operation_id: String,
    pub status: NativeRequestStatus,
    pub executor_host_id: String,
    pub origin_replica: Option<u64>,
    pub sequence: u64,
    pub input_json: String,
    pub result_size_class: String,
    pub retention_class: String,
    pub result_json: Option<String>,
    pub error_json: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TrayMenu {
    pub title: String,
    pub items: Vec<TrayMenuItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrayMenuItem {
    pub id: String,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GlobalShortcut {
    pub accelerator: String,
    pub verb: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeRequestStatus {
    Pending,
    Completed,
    Failed,
    Cancelled,
}

impl NativeRequestStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(&self) -> bool {
        !matches!(self, Self::Pending)
    }
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct PlatformObserved {
    pub host_id: String,
    pub platform: String,
    pub connector_version: String,
    pub supported_operations: Vec<String>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Requested {
    pub request_id: String,
    pub app: String,
    pub operation_id: String,
    pub executor_host_id: String,
    pub origin_replica: Option<u64>,
    pub sequence: u64,
    pub input_json: String,
    pub result_size_class: String,
    pub retention_class: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Terminal {
    pub app: String,
    pub request_id: String,
    pub payload_json: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Cancelled {
    pub app: String,
    pub request_id: String,
    pub reason: String,
}

pub(crate) fn terminal_retention_limit() -> usize {
    DEFAULT_TERMINAL_RETAIN
}
