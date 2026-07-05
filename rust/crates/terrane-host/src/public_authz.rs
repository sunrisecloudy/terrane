use terrane_core::{namespace_of, ExecutionPrincipal};

use crate::HostCore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicCommandAuthz {
    Allow,
    Refuse { reason: String },
    NeedsGrant { app: String, namespace: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicCommandDisposition {
    Allow,
    Refuse {
        reason: &'static str,
    },
    GrantGated {
        namespace: &'static str,
        app_arg_index: usize,
    },
    Unclassified,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublicQueryDisposition {
    Allow,
    Unclassified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PublicQueryAuthz {
    Allow,
    Refuse { reason: String },
}

pub fn classify_public_command(name: &str) -> PublicCommandDisposition {
    match name {
        "kv.set" | "kv.rm" | "kv.delete" => PublicCommandDisposition::GrantGated {
            namespace: "kv",
            app_arg_index: 0,
        },
        "blob.put" | "blob.rm" | "blob.link" => PublicCommandDisposition::GrantGated {
            namespace: "blob",
            app_arg_index: 0,
        },
        "crdt.mapSet"
        | "crdt.mapDel"
        | "crdt.listPush"
        | "crdt.listInsert"
        | "crdt.listDel"
        | "crdt.textInsert"
        | "crdt.textDel"
        | "crdt.merge" => PublicCommandDisposition::GrantGated {
            namespace: "crdt",
            app_arg_index: 0,
        },
        "relational_db.defineTable" | "relational_db.put" | "relational_db.delete" => {
            PublicCommandDisposition::GrantGated {
                namespace: "relational_db",
                app_arg_index: 0,
            }
        }
        "document.create" | "document.patch" | "document.append" | "document.delete" => {
            PublicCommandDisposition::GrantGated {
                namespace: "document",
                app_arg_index: 0,
            }
        }
        "search.upsert"
        | "search.upsertJson"
        | "search.remove"
        | "search.configure"
        | "search.setEmbedding" => PublicCommandDisposition::GrantGated {
            namespace: "search",
            app_arg_index: 0,
        },
        "query.view.define" | "query.materialize" | "query.view.drop" => {
            PublicCommandDisposition::GrantGated {
                namespace: "query",
                app_arg_index: 0,
            }
        }
        "history.revert" => PublicCommandDisposition::GrantGated {
            namespace: "history",
            app_arg_index: 0,
        },
        "interop.call" | "interop.send" => PublicCommandDisposition::GrantGated {
            namespace: "interop",
            app_arg_index: 0,
        },
        "interop.pick" => PublicCommandDisposition::Refuse {
            reason: "interop picker grants must go through the recorded picker approval flow",
        },
        "native.clipboard.write-text"
        | "native.external.open-url"
        | "native.notification.show"
        | "native.dialog.open-file"
        | "native.clipboardWriteText"
        | "native.externalOpenUrl"
        | "native.notificationShow"
        | "native.dialogOpenFile" => PublicCommandDisposition::GrantGated {
            namespace: "native",
            app_arg_index: 0,
        },
        "native.platform.observe" | "native.complete" | "native.fail" | "native.cancel" => {
            PublicCommandDisposition::Refuse {
                reason: "native connector and terminal commands are trusted-host-only",
            }
        }
        "scheduler.create" | "scheduler.pause" | "scheduler.resume" | "scheduler.remove" => {
            PublicCommandDisposition::GrantGated {
                namespace: "scheduler",
                app_arg_index: 0,
            }
        }
        "scheduler.run.start" | "scheduler.run.complete" | "scheduler.run.fail" => {
            PublicCommandDisposition::Refuse {
                reason: "scheduler run commands are trusted-host-only",
            }
        }
        "kv.storage.set" | "kv.storage.clear" => PublicCommandDisposition::Refuse {
            reason: "storage configuration is trusted-admin-only",
        },
        "kv.public.set" | "kv.public.rm" | "kv.public.import" => PublicCommandDisposition::Refuse {
            reason: "public KV is trusted-host-only; app backends read it via ctx.resource.kv.public*",
        },
        "js-runtime.run" | "wasm-runtime.run" => PublicCommandDisposition::Refuse {
            reason: "run apps through the invoke tool",
        },
        "net.request" => PublicCommandDisposition::GrantGated {
            namespace: "net",
            app_arg_index: 0,
        },
        "browser.render" => PublicCommandDisposition::GrantGated {
            namespace: "browser",
            app_arg_index: 0,
        },
        "net.fetch" => PublicCommandDisposition::Refuse {
            reason: "net.fetch is not available through untrusted capability_command",
        },
        "model.ask" => PublicCommandDisposition::Refuse {
            reason: "model.ask is not available through untrusted capability_command",
        },
        "local-model.ask" | "local-model.embed" => PublicCommandDisposition::GrantGated {
            namespace: "local-model",
            app_arg_index: 0,
        },
        "time.now" => PublicCommandDisposition::GrantGated {
            namespace: "time",
            app_arg_index: 0,
        },
        "telemetry.error" => PublicCommandDisposition::GrantGated {
            namespace: "telemetry",
            app_arg_index: 0,
        },
        "local-model.register" | "local-model.pull" | "local-model.rm" | "local-model.default" => {
            PublicCommandDisposition::Refuse {
                reason: "local model specs configure machine-local weights and are trusted-admin-only",
            }
        }
        "harness.generate-app" | "harness.run-js" => PublicCommandDisposition::Refuse {
            reason: "harness commands are trusted tooling and cannot run through untrusted capability_command",
        },
        "app.remove" => PublicCommandDisposition::Refuse {
            reason: "app.remove is destructive and trusted-admin-only",
        },
        "app.import" => PublicCommandDisposition::Refuse {
            reason: "app.import installs bundles and can configure storage; use app_register/app_register_inline or a trusted path",
        },
        "app.add" | "replica.init" => PublicCommandDisposition::Allow,
        // App-callable stt surface: record a selection / stop a session. Both
        // take the app id at arg 0 and are gated by the stt grant.
        "stt.select" | "stt.stop" => PublicCommandDisposition::GrantGated {
            namespace: "stt",
            app_arg_index: 0,
        },
        // The host-edge stt verbs (session lifecycle, segment append, retention)
        // are trusted-host only — admit_command refuses them for public callers.
        "stt.session.open"
        | "stt.segment.append"
        | "stt.session.close-host"
        | "stt.retention.trim"
        | "stt.session.purge" => PublicCommandDisposition::Refuse {
            reason: "stt capture commands are trusted-host only (host-owned edge)",
        },
        _ if name.starts_with("auth.") => PublicCommandDisposition::Refuse {
            reason: "auth commands are trusted-admin-only",
        },
        _ if name.starts_with("agent.") => PublicCommandDisposition::Refuse {
            reason: "agent commands manage assistant definitions and are trusted-admin-only",
        },
        _ => PublicCommandDisposition::Unclassified,
    }
}

pub fn authorize_public_command(
    core: &HostCore,
    name: &str,
    args: &[String],
) -> Result<PublicCommandAuthz, String> {
    match classify_public_command(name) {
        PublicCommandDisposition::Allow => Ok(PublicCommandAuthz::Allow),
        PublicCommandDisposition::Refuse { reason } => Ok(PublicCommandAuthz::Refuse {
            reason: reason.to_string(),
        }),
        PublicCommandDisposition::Unclassified => Ok(PublicCommandAuthz::Refuse {
            reason: format!("{name} is not classified for untrusted capability_command"),
        }),
        PublicCommandDisposition::GrantGated {
            namespace,
            app_arg_index,
        } => {
            let command_namespace = namespace_of(name).map_err(|e| e.to_string())?;
            if command_namespace != namespace {
                return Ok(PublicCommandAuthz::Refuse {
                    reason: format!(
                        "{name} is classified for namespace {namespace}, but belongs to {command_namespace}"
                    ),
                });
            }
            let app = match args.get(app_arg_index).map(String::as_str) {
                Some(app) if !app.trim().is_empty() => app,
                _ => {
                    return Ok(PublicCommandAuthz::Refuse {
                        reason: format!(
                            "{name} requires app id at args[{app_arg_index}] for public grant check"
                        ),
                    })
                }
            };
            if !core.state().app.apps.contains_key(app) {
                return Ok(PublicCommandAuthz::Refuse {
                    reason: format!("no such app: {app}"),
                });
            }
            let principal = ExecutionPrincipal::local_owner();
            let granted =
                terrane_cap_auth::namespace_granted(core.state(), &principal, app, namespace)
                    .map_err(|e| e.to_string())?;
            if granted {
                Ok(PublicCommandAuthz::Allow)
            } else {
                Ok(PublicCommandAuthz::NeedsGrant {
                    app: app.to_string(),
                    namespace: namespace.to_string(),
                })
            }
        }
    }
}

pub fn classify_public_query_name(name: &str) -> PublicQueryDisposition {
    match name {
        "app.exists" | "interop.apps" | "native.supports" | "replica.peer" => {
            PublicQueryDisposition::Allow
        }
        _ => PublicQueryDisposition::Unclassified,
    }
}

pub fn authorize_public_query(capability: &str, query: &str) -> Result<PublicQueryAuthz, String> {
    let name = normalize_query_name(capability, query)?;
    match classify_public_query_name(&name) {
        PublicQueryDisposition::Allow => Ok(PublicQueryAuthz::Allow),
        PublicQueryDisposition::Unclassified => Ok(PublicQueryAuthz::Refuse {
            reason: format!("{name} is not classified for untrusted capability_query"),
        }),
    }
}

fn normalize_query_name(capability: &str, query: &str) -> Result<String, String> {
    match query.split_once('.') {
        Some((namespace, _)) if namespace != capability => Err(format!(
            "query {query} does not belong to capability {capability} (got {namespace})"
        )),
        Some(_) => Ok(query.to_string()),
        None => Ok(format!("{capability}.{query}")),
    }
}
