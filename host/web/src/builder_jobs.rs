use std::collections::HashMap;
use std::thread::JoinHandle;

use terrane_host::{EventRecord, HarnessStaging};

/// Background app-generation jobs.
///
/// The web host serves one request at a time, so a minutes-long harness run
/// cannot live inside the request loop. `POST /__terrane/builder/generate`
/// starts a worker thread running the harness effect standalone and returns
/// immediately; `POST /__terrane/builder/status` polls, and the first poll
/// that finds the worker finished stages its records and commits them through
/// an ordinary `harness.generate-app` dispatch on the core.
pub struct BuilderJobs {
    staging: HarnessStaging,
    jobs: HashMap<String, Job>,
}

struct Job {
    args: Vec<String>,
    handle: Option<JoinHandle<Result<Vec<EventRecord>, String>>>,
}

pub enum JobPoll {
    Running,
    /// The committed draft, as its JSON view.
    Done(String),
    Failed(String),
    Unknown,
}

impl BuilderJobs {
    pub fn new(staging: HarnessStaging) -> Self {
        Self {
            staging,
            jobs: HashMap::new(),
        }
    }

    pub fn running(&self, draft_id: &str) -> bool {
        self.jobs.contains_key(draft_id)
    }

    pub fn start(&mut self, draft_id: &str, app_id: &str, name: &str, harness: &str, prompt: &str) {
        let args = vec![
            "--harness".to_string(),
            harness.to_string(),
            draft_id.to_string(),
            app_id.to_string(),
            name.to_string(),
            prompt.to_string(),
        ];
        let (draft, app, name, harness, prompt) = (
            draft_id.to_string(),
            app_id.to_string(),
            name.to_string(),
            harness.to_string(),
            prompt.to_string(),
        );
        let handle = std::thread::spawn(move || {
            terrane_host::generate_app_records(&draft, &app, &name, &harness, &prompt)
        });
        self.jobs.insert(
            draft_id.to_string(),
            Job {
                args,
                handle: Some(handle),
            },
        );
    }

    pub fn poll(&mut self, core: &mut terrane_host::HostCore, draft_id: &str) -> JobPoll {
        let Some(job) = self.jobs.get_mut(draft_id) else {
            // Polled after completion (or a page reload): serve the committed
            // draft from state.
            return match terrane_host::builder_draft_json(core, draft_id) {
                Some(json) => JobPoll::Done(json),
                None => JobPoll::Unknown,
            };
        };
        if job.handle.as_ref().is_some_and(|h| !h.is_finished()) {
            return JobPoll::Running;
        }
        let Some(handle) = job.handle.take() else {
            self.jobs.remove(draft_id);
            return JobPoll::Failed("generation worker already drained".to_string());
        };
        let args = job.args.clone();
        self.jobs.remove(draft_id);

        let records = match handle.join() {
            Ok(Ok(records)) => records,
            Ok(Err(e)) => return JobPoll::Failed(e),
            Err(_) => return JobPoll::Failed("generation worker panicked".to_string()),
        };
        self.staging.stage_generated(draft_id, records);
        if let Err(e) = terrane_host::dispatch_on_core(core, "harness.generate-app", &args) {
            return JobPoll::Failed(e);
        }
        match terrane_host::builder_draft_json(core, draft_id) {
            Some(json) => JobPoll::Done(json),
            None => JobPoll::Failed(format!("builder draft missing after generation: {draft_id}")),
        }
    }
}
