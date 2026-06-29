use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessState {
    pub runs: BTreeMap<String, HarnessJsRun>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HarnessJsRun {
    pub id: String,
    pub app_id: String,
    pub prompt: String,
    pub harness: String,
    pub js: Option<String>,
    pub output: Option<String>,
    pub error: Option<String>,
}
