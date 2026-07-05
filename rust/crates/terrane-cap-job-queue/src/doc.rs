use terrane_cap_interface::{CapabilityDoc, CapabilityManifestDoc, ResourceDoc, SchemaDoc};

pub(crate) fn job_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "job".to_string(),
        title: "Job Queue".to_string(),
        summary: "Durable app-owned background jobs with retries, progress, and replay-stable lifecycle facts.".to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: vec![
                "job.submit".to_string(),
                "job.cancel".to_string(),
                "job.progress".to_string(),
                "job.start".to_string(),
                "job.report".to_string(),
                "job.reap".to_string(),
            ],
            queries: vec!["job.due".to_string()],
            events: vec![
                "job.submitted".to_string(),
                "job.started".to_string(),
                "job.progress".to_string(),
                "job.completed".to_string(),
                "job.failed".to_string(),
                "job.stalled".to_string(),
                "job.cancelled".to_string(),
            ],
            subscriptions: vec!["app.removed".to_string()],
            resource_methods: crate::resource_docs(),
        },
        commands: crate::command_docs(),
        queries: crate::query_docs(),
        events: crate::event_docs(),
        resources: vec![ResourceDoc {
            namespace: "job".to_string(),
            summary: "App-scoped job submission, progress, cancellation, and state reads.".to_string(),
            methods: crate::resource_docs(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: crate::examples(),
        constraints: crate::constraints(),
        limits: crate::limits(),
        compatibility: vec![
            "No jitter in v1; next_attempt_at is a recorded fact and can include jitter later without changing replay.".to_string(),
            "Terminal jobs remain folded state; log compaction/pruning is a platform-wide follow-up.".to_string(),
        ],
        internal: crate::internal(include_internal),
    }
}
