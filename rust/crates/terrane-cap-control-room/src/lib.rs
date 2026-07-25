//! Read-only, redacted catalog powering the built-in Control Room app.

use serde_json::{json, Value};
use terrane_cap_interface::{
    state_ref, CapManifest, Capability, CapabilityDoc, CommandCtx, Decision, Error, EventRecord,
    GrantResourceSpec, ReadValue, ResourceMethod, ResourceReadCtx, Result, StateStore,
};

mod doc;

pub struct ControlRoomCapability;

impl Capability for ControlRoomCapability {
    fn namespace(&self) -> &'static str {
        "control-room"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: Vec::new(),
            events: Vec::new(),
            queries: Vec::new(),
            resources: vec![ResourceMethod::Read {
                name: "catalog",
                params: &[],
            }],
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "control-room",
                &["read"],
                "Read-only redacted metadata for installed apps, registered capabilities, grants, models, runtimes, MCP surfaces, and safe aggregate storage health.",
            )],
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> CapabilityDoc {
        doc::control_room_doc(include_internal)
    }

    fn decide(&self, _ctx: CommandCtx<'_>, name: &str, _args: &[String]) -> Result<Decision> {
        Err(Error::InvalidInput(format!("unknown command: {name}")))
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
        if name != "catalog" || !args.is_empty() {
            return Err(Error::InvalidInput(format!(
                "unknown resource read: control-room.{name}"
            )));
        }
        Ok(ReadValue::OptString(Some(catalog_json(ctx)?)))
    }
}

fn catalog_json(ctx: ResourceReadCtx<'_>) -> Result<String> {
    let docs = ctx.bus.capability_docs(false);
    let capabilities = docs
        .iter()
        .map(|doc| capability_json(ctx, doc))
        .collect::<Vec<_>>();
    let apps = app_catalog(ctx)?;
    let grants = grant_catalog(ctx)?;
    let models = model_catalog(ctx)?;
    let connections = connection_catalog(ctx)?;
    let tools = terrane_api::mcp_tools()
        .into_iter()
        .map(|tool| {
            json!({
                "name": tool.name,
                "purpose": tool.description,
                "inputSchema": serde_json::from_str::<Value>(tool.input_schema).unwrap_or(Value::Null),
                "owner": "terrane-api / host MCP",
                "availability": "registered",
                "factKind": "static-contract"
            })
        })
        .collect::<Vec<_>>();
    let resources = terrane_api::mcp_resources()
        .into_iter()
        .map(|resource| {
            json!({
                "uri": resource.uri,
                "name": resource.name,
                "purpose": resource.description,
                "mediaType": resource.mime_type,
                "factKind": "static-contract"
            })
        })
        .collect::<Vec<_>>();
    let storage = storage_summary(ctx)?;

    let cataloged_app_count = apps
        .iter()
        .filter(|app| {
            app.pointer("/availability/cataloged")
                .and_then(Value::as_bool)
                == Some(true)
        })
        .count();

    serde_json::to_string(&json!({
        "schemaVersion": 1,
        "generatedFrom": {
            "registeredCapabilities": "live folded core registry",
            "appsGrantsModelsStorage": "live folded state plus optional sanitized host manifest reads",
            "mcpToolsAndDocs": "static compiled contract",
            "rawSensitiveContentsIncluded": false
        },
        "privacy": {
            "mode": "metadata-only",
            "redacted": [
                "passwords", "tokens", "master passwords", "secret bytes",
                "connection transports", "grant selector JSON", "private chat text",
                "model prompts and responses", "raw KV values", "document bodies",
                "MCP arguments and results"
            ]
        },
        "system": {
            "availableAppCount": apps.len(),
            "registeredAppCount": cataloged_app_count,
            "registeredCapabilityCount": capabilities.len(),
            "mcpToolCount": tools.len(),
            "grantCount": grants.len(),
            "factKind": "live-and-static-labelled"
        },
        "apps": apps,
        "capabilities": capabilities,
        "mcp": {
            "tools": tools,
            "documentationResources": resources,
            "transportHealth": {
                "status": "not-probed-by-app",
                "reason": "The catalog does not open or mutate MCP transports.",
                "factKind": "explicit-unavailable"
            }
        },
        "grants": grants,
        "models": models,
        "connections": connections,
        "storage": storage,
        "documentation": [
            {"title":"App API","path":"docs/APP_API.md","scope":"app manifests, resources, grants, UI bridge"},
            {"title":"Server API","path":"docs/SERVER_API.md","scope":"host and MCP endpoints"},
            {"title":"Capability operations","path":"host/mcp/docs/CAPABILITY_OPERATIONS.md","scope":"capability commands, safety, permission flow"},
            {"title":"Local models","path":"docs/LOCAL_MODELS.md","scope":"model registration and runtime behavior"},
            {"title":"MCP security","path":"host/mcp/docs/SECURITY.md","scope":"trust and redaction boundaries"}
        ]
    }))
    .map_err(|error| Error::Runtime(format!("control-room catalog encode failed: {error}")))
}

fn capability_json(ctx: ResourceReadCtx<'_>, doc: &CapabilityDoc) -> Value {
    let resource_methods = doc
        .resources
        .iter()
        .flat_map(|resource| {
            resource.methods.iter().map(move |method| {
                json!({
                    "namespace": resource.namespace,
                    "name": method.name,
                    "kind": method.kind,
                    "purpose": method.summary,
                    "returns": method.returns
                })
            })
        })
        .collect::<Vec<_>>();
    let commands = doc
        .commands
        .iter()
        .map(|command| {
            let policy = ctx
                .host
                .and_then(|host| {
                    host.sample(
                        "control-room.command-policy",
                        std::slice::from_ref(&command.name),
                    )
                        .ok()
                })
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .unwrap_or_else(|| {
                    json!({
                        "classification": if command.effects.is_empty() {"recorded-core"} else {"effectful"},
                        "source": "capability documentation",
                        "reason": if command.effects.is_empty() {
                            "No external effects are declared; transport-specific authorization may still apply."
                        } else {
                            "The command declares effects; consult its documentation and host policy before use."
                        }
                    })
                });
            json!({
                "name": command.name,
                "purpose": command.summary,
                "effects": command.effects,
                "returns": command.returns,
                "safety": policy
            })
        })
        .collect::<Vec<_>>();
    let permission = if resource_methods.is_empty() {
        json!({
            "appAccess": "not-exposed-as-app-resource",
            "note": "Host/core commands and queries remain subject to their documented transport policy."
        })
    } else {
        json!({
            "appAccess": "default-deny-grant-required",
            "verbs": resource_methods.iter().filter_map(|item| item.get("kind")).collect::<Vec<_>>()
        })
    };
    json!({
        "namespace": doc.namespace,
        "title": doc.title,
        "purpose": doc.summary,
        "owner": owner_for(&doc.namespace),
        "category": category_for(&doc.namespace),
        "availability": {
            "registered": true,
            "status": doc.status,
            "liveness": if resource_methods.is_empty() {"registered-core"} else {"registered-resource; edge liveness method-specific"},
            "factKind": "live-registry"
        },
        "permission": permission,
        "commands": commands,
        "queries": doc.queries.iter().map(|query| json!({
            "name": query.name, "purpose": query.summary, "returns": query.returns
        })).collect::<Vec<_>>(),
        "resources": resource_methods,
        "events": doc.events.iter().map(|event| json!({
            "kind": event.kind, "purpose": event.summary
        })).collect::<Vec<_>>(),
        "safeDataSummary": safe_summary(ctx.state, &doc.namespace),
        "documentation": {
            "source": format!("rust/crates/{}/src/doc.rs", owner_for(&doc.namespace)),
            "usage": "docs/APP_API.md",
            "mcp": format!("terrane://capabilities/{}", doc.namespace)
        },
        "factKind": "static-contract-with-live-registration"
    })
}

fn app_catalog(ctx: ResourceReadCtx<'_>) -> Result<Vec<Value>> {
    let state = state_ref::<terrane_cap_app::AppState>(ctx.state, "app")?;
    let mut apps = state
        .apps
        .values()
        .map(|app| {
            let manifest = app
                .source
                .as_ref()
                .and_then(|source| {
                    ctx.host.and_then(|host| {
                        host.sample("control-room.manifest", std::slice::from_ref(source))
                            .ok()
                    })
                })
                .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
                .unwrap_or_else(|| json!({
                    "status": "unavailable",
                    "reason": "No live host manifest sampler is attached.",
                    "resources": [],
                    "browserPermissions": [],
                    "publicVerbs": []
                }));
            json!({
                "id": app.id,
                "name": app.name,
                "purpose": manifest.get("purpose").cloned().unwrap_or(Value::Null),
                "version": app.version,
                "runtime": app.runtime,
                "interfaces": app.interfaces,
                "manifest": manifest,
                "availability": {
                    "cataloged": true,
                    "runtimeHealth": "not-invoked-by-control-room",
                    "reason": "Control Room does not execute another app merely to probe it.",
                    "factKind": "live-catalog"
                },
                "actions": {
                    "declaredPublicVerbs": manifest.get("publicVerbs").cloned().unwrap_or_else(|| json!([])),
                    "dynamicDiscovery": "Use MCP app_actions for live backend-declared actions; Control Room does not execute app code by default."
                },
                "data": app_data_summary(ctx.state, &app.id),
                "documentation": {
                    "manifest": "manifest.json",
                    "actionDiscovery": "MCP app_actions",
                    "general": "docs/APP_API.md"
                },
                "factKind": "live-catalog-plus-sanitized-manifest"
            })
        })
        .collect::<Vec<_>>();
    let discovered = ctx
        .host
        .and_then(|host| host.sample("control-room.apps", &[]).ok())
        .and_then(|raw| serde_json::from_str::<Vec<Value>>(&raw).ok())
        .unwrap_or_default();
    for manifest in discovered {
        let Some(id) = manifest
            .get("id")
            .and_then(Value::as_str)
            .map(str::to_string)
        else {
            continue;
        };
        if apps
            .iter()
            .any(|app| app.get("id").and_then(Value::as_str) == Some(id.as_str()))
        {
            continue;
        }
        let name = manifest
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or(&id)
            .to_string();
        let purpose = manifest.get("purpose").cloned().unwrap_or(Value::Null);
        let version = manifest.get("version").cloned().unwrap_or(Value::Null);
        let runtime = manifest.get("runtime").cloned().unwrap_or(Value::Null);
        let interfaces = manifest
            .get("interfaces")
            .cloned()
            .unwrap_or_else(|| json!([]));
        let public_verbs = manifest
            .get("publicVerbs")
            .cloned()
            .unwrap_or_else(|| json!([]));
        apps.push(json!({
            "id": id,
            "name": name,
            "purpose": purpose,
            "version": version,
            "runtime": runtime,
            "interfaces": interfaces,
            "manifest": manifest,
            "availability": {
                "cataloged": false,
                "discovered": true,
                "runtimeHealth": "not-invoked-by-control-room",
                "reason": "The live host discovered this bundle, but it is not currently cataloged in folded Core state.",
                "factKind": "live-host-discovery"
            },
            "actions": {
                "declaredPublicVerbs": public_verbs,
                "dynamicDiscovery": "Use MCP app_actions after the app is cataloged; Control Room does not execute app code by default."
            },
            "data": app_data_summary(ctx.state, &id),
            "documentation": {
                "manifest": "manifest.json",
                "actionDiscovery": "MCP app_actions after cataloging",
                "general": "docs/APP_API.md"
            },
            "factKind": "live-host-discovery-plus-sanitized-manifest"
        }));
    }
    apps.sort_by(|left, right| {
        left.get("name")
            .and_then(Value::as_str)
            .cmp(&right.get("name").and_then(Value::as_str))
    });
    Ok(apps)
}

fn grant_catalog(ctx: ResourceReadCtx<'_>) -> Result<Vec<Value>> {
    let auth = state_ref::<terrane_cap_auth::AuthState>(ctx.state, "auth")?;
    Ok(auth
        .grants
        .values()
        .map(|grant| {
            json!({
                "app": grant.app,
                "namespace": grant.namespace,
                "verbs": grant.verbs,
                "selectorSchema": grant.selector_schema_id,
                "scope": if grant.selector_schema_id == "namespace.v1" {"namespace"} else {"scoped-redacted"},
                "selectorDetails": "redacted",
                "resourceId": "redacted",
                "status": "granted",
                "factKind": "live-folded-state"
            })
        })
        .collect())
}

fn model_catalog(ctx: ResourceReadCtx<'_>) -> Result<Vec<Value>> {
    let models = state_ref::<terrane_cap_local_model::LocalModelState>(ctx.state, "local-model")?;
    Ok(models
        .specs
        .iter()
        .map(|(id, spec)| {
            json!({
                "id": id,
                "backend": spec.backend,
                "format": spec.format,
                "kind": if spec.embedding.is_some() {"embedding"} else {"generation"},
                "sizeBytes": spec.size_bytes,
                "isDefault": models.default_model.as_deref() == Some(id.as_str()),
                "isDefaultEmbedding": models.default_embed_model.as_deref() == Some(id.as_str()),
                "localPath": "redacted",
                "source": spec.source.as_ref().map(|_| "configured-redacted"),
                "availability": "registered; engine liveness not probed",
                "factKind": "live-folded-state"
            })
        })
        .collect())
}

fn connection_catalog(ctx: ResourceReadCtx<'_>) -> Result<Vec<Value>> {
    let state = state_ref::<terrane_cap_connection::ConnectionState>(ctx.state, "connection")?;
    Ok(state
        .connections
        .iter()
        .map(|(name, connection)| {
            json!({
                "name": name,
                "kind": connection.kind,
                "authorized": connection.authorized,
                "scopes": connection.scopes,
                "expiresAt": connection.expires_at,
                "configuration": "redacted",
                "secretBytes": "redacted",
                "factKind": "live-folded-state"
            })
        })
        .collect())
}

fn storage_summary(ctx: ResourceReadCtx<'_>) -> Result<Value> {
    let kv = state_ref::<terrane_cap_kv::KvState>(ctx.state, "kv")?;
    let docs = state_ref::<terrane_cap_document::DocumentState>(ctx.state, "document")?;
    let blobs = state_ref::<terrane_cap_blob::BlobState>(ctx.state, "blob")?;
    let mcp = state_ref::<terrane_cap_mcp_client::McpClientState>(ctx.state, "mcp")?;
    Ok(json!({
        "kv": {
            "apps": kv.data.len(),
            "records": kv.data.values().map(|items| items.len()).sum::<usize>(),
            "valueBytes": kv.data.values().flat_map(|items| items.values()).map(String::len).sum::<usize>(),
            "contents": "redacted"
        },
        "documents": {
            "apps": docs.docs.len(),
            "records": docs.docs.values().map(|items| items.len()).sum::<usize>(),
            "bodyBytes": docs.docs.values().flat_map(|items| items.values()).map(|doc| doc.body.len()).sum::<usize>(),
            "contents": "redacted"
        },
        "blobs": {
            "apps": blobs.blobs.len(),
            "records": blobs.blobs.values().map(|items| items.len()).sum::<usize>(),
            "bytes": blobs.blobs.values().flat_map(|items| items.values()).map(|blob| blob.size).sum::<u64>(),
            "contents": "not-read"
        },
        "mcpCalls": {
            "connections": mcp.connections.len(),
            "records": mcp.calls.values().map(|items| items.len()).sum::<usize>(),
            "resultBytes": mcp.calls.values().flat_map(|items| items.values()).map(|call| call.result_size).sum::<u64>(),
            "argumentsAndResults": "redacted"
        },
        "health": "folded metadata readable",
        "factKind": "live-safe-aggregate"
    }))
}

fn app_data_summary(state: &dyn StateStore, app: &str) -> Value {
    let kv = state_ref::<terrane_cap_kv::KvState>(state, "kv")
        .ok()
        .and_then(|state| state.data.get(app));
    let documents = state_ref::<terrane_cap_document::DocumentState>(state, "document")
        .ok()
        .and_then(|state| state.docs.get(app));
    let blobs = state_ref::<terrane_cap_blob::BlobState>(state, "blob")
        .ok()
        .and_then(|state| state.blobs.get(app));
    json!({
        "kv": {"records": kv.map_or(0, |items| items.len()), "contents": "redacted"},
        "documents": {"records": documents.map_or(0, |items| items.len()), "contents": "redacted"},
        "blobs": {
            "records": blobs.map_or(0, |items| items.len()),
            "bytes": blobs.map_or(0, |items| items.values().map(|blob| blob.size).sum::<u64>()),
            "contents": "not-read"
        },
        "factKind": "live-safe-aggregate"
    })
}

fn safe_summary(state: &dyn StateStore, namespace: &str) -> Value {
    match namespace {
        "app" => state_ref::<terrane_cap_app::AppState>(state, "app")
            .map(|state| json!({"apps": state.apps.len()}))
            .unwrap_or(Value::Null),
        "auth" => state_ref::<terrane_cap_auth::AuthState>(state, "auth")
            .map(|state| json!({
                "members": state.members.len(),
                "grants": state.grants.len(),
                "permissionRequests": state.permission_requests.len(),
                "selectorDetails": "redacted"
            }))
            .unwrap_or(Value::Null),
        "automation" => state_ref::<terrane_cap_automation::AutomationState>(state, "automation")
            .map(|state| json!({"apps": state.rules.len(), "rules": state.rules.values().map(|items| items.len()).sum::<usize>()}))
            .unwrap_or(Value::Null),
        "blob" => state_ref::<terrane_cap_blob::BlobState>(state, "blob")
            .map(|state| json!({"apps": state.blobs.len(), "records": state.blobs.values().map(|items| items.len()).sum::<usize>(), "contents": "not-read"}))
            .unwrap_or(Value::Null),
        "builder" => state_ref::<terrane_cap_builder::BuilderState>(state, "builder")
            .map(|state| json!({"drafts": state.drafts.len(), "promptsAndFiles": "redacted"}))
            .unwrap_or(Value::Null),
        "connection" => state_ref::<terrane_cap_connection::ConnectionState>(state, "connection")
            .map(|state| json!({"connections": state.connections.len(), "configurationAndSecrets": "redacted"}))
            .unwrap_or(Value::Null),
        "document" => state_ref::<terrane_cap_document::DocumentState>(state, "document")
            .map(|state| json!({"apps": state.docs.len(), "records": state.docs.values().map(|items| items.len()).sum::<usize>(), "contents": "redacted"}))
            .unwrap_or(Value::Null),
        "job" => state_ref::<terrane_cap_job_queue::JobState>(state, "job")
            .map(|state| json!({"apps": state.jobs.len(), "jobs": state.jobs.values().map(|items| items.len()).sum::<usize>(), "argumentsAndOutput": "redacted"}))
            .unwrap_or(Value::Null),
        "kv" => state_ref::<terrane_cap_kv::KvState>(state, "kv")
            .map(|state| json!({"apps": state.data.len(), "records": state.data.values().map(|items| items.len()).sum::<usize>(), "contents": "redacted"}))
            .unwrap_or(Value::Null),
        "local-model" => state_ref::<terrane_cap_local_model::LocalModelState>(state, "local-model")
            .map(|state| json!({"models": state.specs.len(), "recordedTurns": state.turns.values().map(Vec::len).sum::<usize>(), "promptsAndResponses": "redacted"}))
            .unwrap_or(Value::Null),
        "mcp" => state_ref::<terrane_cap_mcp_client::McpClientState>(state, "mcp")
            .map(|state| json!({"connections": state.connections.len(), "calls": state.calls.values().map(|items| items.len()).sum::<usize>(), "transportArgumentsResults": "redacted"}))
            .unwrap_or(Value::Null),
        "scheduler" => state_ref::<terrane_cap_scheduler::SchedulerState>(state, "scheduler")
            .map(|state| json!({"apps": state.schedules.len(), "schedules": state.schedules.values().map(|items| items.len()).sum::<usize>()}))
            .unwrap_or(Value::Null),
        _ => json!({
            "status": "unavailable",
            "reason": "No safe aggregate is exposed for this capability in the initial read-only catalog."
        }),
    }
}

fn owner_for(namespace: &str) -> String {
    let crate_name = match namespace {
        "job" => "terrane-cap-job-queue".to_string(),
        other => format!("terrane-cap-{other}"),
    };
    crate_name
}

fn category_for(namespace: &str) -> &'static str {
    match namespace {
        "kv" | "blob" | "document" | "relational_db" | "crdt" | "query" | "search" | "history" => {
            "Data & storage"
        }
        "auth" | "connection" | "crypto" | "person" | "org" | "replica" => {
            "Identity, permissions & security"
        }
        "model" | "local-model" | "stt" | "tts" | "media" => "Models & media",
        "mcp" | "interop" | "common" | "net" | "webhook" | "web-publish" | "publish" | "share"
        | "sync" | "presence" | "push" => "Integration & transport",
        "scheduler" | "automation" | "job" | "stream" | "time" | "telemetry" => {
            "Operations & automation"
        }
        "js-runtime" | "wasm-runtime" | "native" | "browser" | "applescript" | "sysinfo"
        | "geo" => "Runtime & host",
        "app" | "builder" | "build" | "harness" | "migration" | "agent" | "control-room" => {
            "Apps & platform"
        }
        _ => "Other",
    }
}
