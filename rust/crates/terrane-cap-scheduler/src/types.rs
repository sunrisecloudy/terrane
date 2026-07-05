use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchedulerState {
    pub schedules: BTreeMap<String, BTreeMap<String, ScheduleEntry>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleEntry {
    pub app: String,
    pub name: String,
    pub spec: ScheduleSpec,
    pub last_scheduled_for: Option<u64>,
    pub last_fired_at: Option<u64>,
    pub skipped_total: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleSpec {
    pub kind: ScheduleKind,
    pub verb: String,
    pub args: Vec<String>,
    pub spec_json: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScheduleKind {
    At(u64),
    Cron(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DueSchedule {
    pub app: String,
    pub name: String,
    pub scheduled_for: u64,
    pub skipped: u64,
    pub verb: String,
    pub args: Vec<String>,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Set {
    pub app: String,
    pub name: String,
    pub spec_json: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Cleared {
    pub app: String,
    pub name: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Fired {
    pub app: String,
    pub name: String,
    pub scheduled_for: u64,
    pub fired_at: u64,
    pub skipped: u64,
}
