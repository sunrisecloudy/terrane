use nanoserde::SerJson;

use super::{BuilderDraft, BuilderFile};

#[derive(SerJson)]
struct DraftJson {
    id: String,
    #[nserde(rename = "appId")]
    app_id: String,
    name: String,
    prompt: String,
    agent: String,
    status: String,
    error: String,
    files: Vec<BuilderFile>,
}

pub fn draft_json(draft: &BuilderDraft) -> String {
    DraftJson {
        id: draft.id.clone(),
        app_id: draft.app_id.clone(),
        name: draft.name.clone(),
        prompt: draft.prompt.clone(),
        agent: draft.agent.clone(),
        status: if draft.error.is_some() {
            "failed".to_string()
        } else if draft.files.is_empty() {
            "requested".to_string()
        } else {
            "generated".to_string()
        },
        error: draft.error.clone().unwrap_or_default(),
        files: draft.files.clone(),
    }
    .serialize_json()
}
