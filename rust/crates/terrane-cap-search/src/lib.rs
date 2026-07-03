//! Hybrid search capability — a rebuildable KV projection with BM25 + vector RRF.

use terrane_cap_interface::{
    arg, CapManifest, Capability, CommandCtx, CommandSpec, Decision, Error, EventRecord,
    GrantResourceSpec, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

mod commands;
mod config;
mod document;
mod doc;
mod key;
mod query;

pub use config::SearchConfig;
pub use key::SEARCH_PREFIX;
pub use query::{rrf_score, SearchHit};

pub struct SearchCapability;

impl Capability for SearchCapability {
    fn namespace(&self) -> &'static str {
        "search"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: vec![
                CommandSpec {
                    name: "search.upsert",
                },
                CommandSpec {
                    name: "search.upsertJson",
                },
                CommandSpec {
                    name: "search.remove",
                },
                CommandSpec {
                    name: "search.configure",
                },
                CommandSpec {
                    name: "search.setEmbedding",
                },
            ],
            events: Vec::new(),
            queries: Vec::new(),
            resources: resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "search",
                &["read", "write"],
                "App-scoped hybrid search namespace.",
            )],
            subscriptions: Vec::new(),
        }
    }

    fn decide(&self, ctx: CommandCtx<'_>, name: &str, args: &[String]) -> Result<Decision> {
        match name {
            "search.upsert" => commands::decide_upsert(ctx, args),
            "search.upsertJson" => commands::decide_upsert_json(ctx, args),
            "search.remove" => commands::decide_remove(ctx, args),
            "search.configure" => commands::decide_configure(ctx, args),
            "search.setEmbedding" => commands::decide_set_embedding(ctx, args),
            other => Err(Error::InvalidInput(format!("unknown command: {other}"))),
        }
    }

    fn fold(&self, _state: &mut dyn StateStore, _record: &EventRecord) -> Result<()> {
        Ok(())
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        let config = commands::load_config(ctx.state, ctx.app)?;
        match name {
            "query" => {
                let query_text = arg(args, 0, "text")?;
                let options_json = optional_joined_arg(args, 1);
                let options = config::parse_query_options(&options_json)?;
                Ok(ReadValue::OptString(Some(query::hybrid_query(
                    ctx.state,
                    ctx.app,
                    &config,
                    &query_text,
                    &options,
                )?)))
            }
            "bm25" => {
                let query_text = arg(args, 0, "text")?;
                let options_json = optional_joined_arg(args, 1);
                let options = config::parse_query_options(&options_json)?;
                Ok(ReadValue::OptString(Some(query::bm25_query(
                    ctx.state,
                    ctx.app,
                    &config,
                    &query_text,
                    &options,
                )?)))
            }
            "vectorSearch" => {
                // The query vector is a single compact-JSON argument; joining
                // args[0..] here would swallow the options JSON that follows.
                let query_vec_json = arg(args, 0, "queryVecJson")?;
                let options_json = optional_joined_arg(args, 1);
                let options = config::parse_query_options(&options_json)?;
                Ok(ReadValue::OptString(Some(query::vector_query(
                    ctx.state,
                    ctx.app,
                    &config,
                    &query_vec_json,
                    &options,
                )?)))
            }
            "status" => Ok(ReadValue::OptString(Some(query::status_json(
                ctx.state,
                ctx.app,
                &config,
            )?))),
            other => Err(Error::InvalidInput(format!(
                "unknown resource read: search.{other}"
            ))),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::search_doc(include_internal)
    }
}

pub fn resource_methods() -> Vec<ResourceMethod> {
    vec![
        ResourceMethod::Write {
            name: "upsert",
            params: &["docId", "text"],
        },
        ResourceMethod::Write {
            name: "upsertJson",
            params: &["docId", "docJson"],
        },
        ResourceMethod::Write {
            name: "remove",
            params: &["docId"],
        },
        ResourceMethod::Write {
            name: "configure",
            params: &["configJson"],
        },
        ResourceMethod::Write {
            name: "setEmbedding",
            params: &["docId", "embeddingJson"],
        },
        ResourceMethod::Read {
            name: "query",
            params: &["text", "optionsJson"],
        },
        ResourceMethod::Read {
            name: "bm25",
            params: &["text", "optionsJson"],
        },
        ResourceMethod::Read {
            name: "vectorSearch",
            params: &["queryVecJson", "optionsJson"],
        },
        ResourceMethod::Read {
            name: "status",
            params: &[],
        },
    ]
}

fn optional_joined_arg(args: &[String], index: usize) -> String {
    args.get(index..)
        .filter(|rest| !rest.is_empty())
        .map(|rest| rest.join(" "))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests;