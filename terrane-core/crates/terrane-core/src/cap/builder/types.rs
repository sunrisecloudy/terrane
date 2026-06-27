use std::collections::BTreeMap;

use borsh::{BorshDeserialize, BorshSerialize};
use nanoserde::{DeJson, SerJson};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuilderState {
    pub drafts: BTreeMap<String, BuilderDraft>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BuilderDraft {
    pub id: String,
    pub app_id: String,
    pub name: String,
    pub prompt: String,
    pub agent: String,
    pub files: Vec<BuilderFile>,
    pub error: Option<String>,
}

#[derive(
    Debug, Clone, Default, PartialEq, Eq, BorshSerialize, BorshDeserialize, DeJson, SerJson,
)]
pub struct BuilderFile {
    pub path: String,
    pub content: String,
}
