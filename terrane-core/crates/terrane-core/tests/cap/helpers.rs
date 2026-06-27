//! Shared fixtures for the per-capability engine tests.

use std::sync::atomic::{AtomicU64, Ordering};

use terrane_core::cap::builder::{generated_event, requested_event, BuilderFile};
use terrane_core::cap::model::responded_event;
use terrane_core::cap::net::fetched_event;
use terrane_core::cap::replica::initialized_event;
use terrane_core::{Effect, EffectRunner};
use terrane_domain::{EventRecord, Request, Result};

/// Build a `Request` from a dotted name and string args.
pub(crate) fn req(name: &str, args: &[&str]) -> Request {
    Request::new(name, args.iter().map(|s| s.to_string()).collect())
}

/// A deterministic stand-in for the edge: canned responses for every effect, so
/// tests never touch the network or spawn a real agent.
pub(crate) struct FakeEdge;

impl EffectRunner for FakeEdge {
    fn run(&self, effect: &Effect) -> Result<Vec<EventRecord>> {
        match effect {
            Effect::HttpGet { app, url } => Ok(vec![fetched_event(
                app,
                url,
                200,
                format!("body for {url}"),
            )?]),
            Effect::ModelCall { app, agent, prompt } => Ok(vec![responded_event(
                app,
                agent,
                prompt,
                format!("{agent} says: {prompt}"),
                0,
            )?]),
            Effect::BuildAppWithAgent {
                draft_id,
                app_id,
                name,
                agent,
                prompt,
            } => Ok(vec![
                requested_event(draft_id, app_id, name, prompt, agent)?,
                generated_event(
                    draft_id,
                    vec![
                        BuilderFile {
                            path: "index.html".into(),
                            content: "<!doctype html><title>Fake</title>".into(),
                        },
                        BuilderFile {
                            path: "main.js".into(),
                            content: "var actions={hello:{summary:\"Say hello.\",args:[],run:function(){return \"hi\";}}};".into(),
                        },
                        BuilderFile {
                            path: "manifest.json".into(),
                            content: format!(
                                r#"{{"id":"{app_id}","name":"{name}","version":"0.1.0","backend":"main.js","ui":"index.html","resources":[]}}"#
                            ),
                        },
                        BuilderFile {
                            path: "style.css".into(),
                            content: "body { font-family: system-ui; }".into(),
                        },
                    ],
                )?,
            ]),
            // Distinct id per call so two replicas built with FakeEdge get
            // different peers (real edge uses OS entropy; tests just need unique).
            Effect::NewReplicaId => {
                static NEXT: AtomicU64 = AtomicU64::new(1);
                Ok(vec![initialized_event(
                    NEXT.fetch_add(1, Ordering::Relaxed),
                )?])
            }
        }
    }
}
