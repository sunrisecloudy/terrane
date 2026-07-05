use std::collections::{BTreeMap, BTreeSet};

use borsh::{BorshDeserialize, BorshSerialize};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutomationState {
    pub rules: BTreeMap<String, BTreeMap<String, RuleEntry>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleEntry {
    pub app: String,
    pub name: String,
    pub spec: RuleSpec,
    pub rule_json: String,
    pub rule_hash: String,
    pub last_fired_at: Option<u64>,
    pub fire_count: u64,
    pub suppressed_count: u64,
    pub seen_event_refs: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleSpec {
    pub trigger: TriggerSpec,
    pub action: ActionSpec,
    pub cooldown_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TriggerSpec {
    pub kind: String,
    pub source_app: Option<String>,
    pub filter: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionSpec {
    pub verb: String,
    pub args_template: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchingRule {
    pub app: String,
    pub name: String,
    pub rule_hash: String,
    pub verb: String,
    pub args_template: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MatchEvent {
    pub event_ref: String,
    pub event_json: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FireStats {
    pub app: String,
    pub name: String,
    pub rule_hash: String,
    pub event_ref: String,
    pub fired_at: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Set {
    pub app: String,
    pub name: String,
    pub rule_json: String,
    pub rule_hash: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Removed {
    pub app: String,
    pub name: String,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Fired {
    pub app: String,
    pub name: String,
    pub rule_hash: String,
    pub event_ref: String,
    pub fired_at: u64,
}

#[derive(BorshSerialize, BorshDeserialize)]
pub(crate) struct Suppressed {
    pub app: String,
    pub name: String,
    pub rule_hash: String,
    pub event_ref: String,
    pub suppressed_at: u64,
    pub reason: String,
}
