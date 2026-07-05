use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SchedulerState {
    pub schedules: BTreeMap<String, BTreeMap<String, ScheduleRecord>>,
    pub runs: BTreeMap<String, BTreeMap<String, RunRecord>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScheduleRecord {
    pub id: String,
    pub app: String,
    pub cron: String,
    pub timezone: String,
    pub action: String,
    pub payload_json: String,
    pub paused: bool,
    pub next_due_at: u64,
    pub active_run_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunRecord {
    pub run_id: String,
    pub schedule_id: String,
    pub app: String,
    pub action: String,
    pub payload_json: String,
    pub status: RunStatus,
    pub due_at: u64,
    pub started_at: u64,
    pub finished_at: Option<u64>,
    pub output_json: Option<String>,
    pub error_json: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Started,
    Completed,
    Failed,
}

impl RunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Completed => "completed",
            Self::Failed => "failed",
        }
    }

    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Created {
    pub app: String,
    pub id: String,
    pub cron: String,
    pub timezone: String,
    pub action: String,
    pub payload_json: String,
    pub next_due_at: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct ScheduleId {
    pub app: String,
    pub id: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct RunStarted {
    pub app: String,
    pub id: String,
    pub run_id: String,
    pub action: String,
    pub payload_json: String,
    pub due_at: u64,
    pub started_at: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct RunTerminal {
    pub app: String,
    pub id: String,
    pub run_id: String,
    pub finished_at: u64,
    pub next_due_at: u64,
    pub payload_json: String,
}
