use nanoserde::{DeJson, SerJson};
use terrane_host::LOCAL_OWNER_SUBJECT;
use tiny_http::{Request, Response};

use crate::http::{header, json_error, json_ok, Resp};

const ADMIN_HTML: &str = include_str!("templates/admin.html");
const ADMIN_JS: &str = include_str!("js/admin.js");

#[derive(Debug, Clone, SerJson)]
struct AdminSession {
    org: String,
    subject: String,
    source: String,
    locked: bool,
}

#[derive(Debug, Clone, SerJson)]
struct AdminAppResource {
    namespace: String,
    granted: bool,
}

#[derive(Debug, Clone, SerJson)]
struct AdminApp {
    id: String,
    name: String,
    resources: Vec<AdminAppResource>,
}

#[derive(Debug, Clone, SerJson)]
struct AdminAppsResponse {
    apps: Vec<AdminApp>,
}

#[derive(Debug, Clone, SerJson)]
struct AdminGrant {
    org: String,
    subject: String,
    app: String,
    namespace: String,
    resource_id: String,
}

#[derive(Debug, Clone, SerJson)]
struct AdminGrantsResponse {
    grants: Vec<AdminGrant>,
}

#[derive(Debug, Clone, SerJson)]
struct GrantResponse {
    records: usize,
    output: Option<String>,
}

#[derive(Debug, Clone, DeJson)]
struct GrantRequest {
    app: String,
    namespace: String,
    #[nserde(default)]
    subject: String,
}

pub fn page() -> Resp {
    let body = ADMIN_HTML.replace("__ADMIN_JS__", ADMIN_JS);
    Response::from_data(body.into_bytes())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
}

pub fn session() -> Resp {
    json_ok(&AdminSession {
        org: "local".to_string(),
        subject: LOCAL_OWNER_SUBJECT.to_string(),
        source: "local".to_string(),
        locked: false,
    })
}

pub fn apps(core: &terrane_host::HostCore) -> Resp {
    let mut apps = Vec::new();
    for app in core.state().app.apps.values() {
        let requested = terrane_host::permission::app_requested_resources(core, &app.id)
            .unwrap_or_else(|_| Vec::new());
        let missing = terrane_host::permission::permission_required_for_app(core, &app.id)
            .ok()
            .flatten()
            .map(|required| required.missing_resources)
            .unwrap_or_default();
        let resources = requested
            .into_iter()
            .map(|namespace| AdminAppResource {
                granted: !missing.iter().any(|item| item == &namespace),
                namespace,
            })
            .collect();
        apps.push(AdminApp {
            id: app.id.clone(),
            name: app.name.clone(),
            resources,
        });
    }
    apps.sort_by(|a, b| a.id.cmp(&b.id));
    json_ok(&AdminAppsResponse { apps })
}

pub fn grants(core: &terrane_host::HostCore) -> Resp {
    let mut grants = core
        .state()
        .auth
        .grants
        .values()
        .map(|grant| AdminGrant {
            org: grant.org.clone(),
            subject: grant.subject.clone(),
            app: grant.app.clone(),
            namespace: grant.namespace.clone(),
            resource_id: grant.resource_id.clone(),
        })
        .collect::<Vec<_>>();
    grants.sort_by(|a, b| {
        a.app
            .cmp(&b.app)
            .then_with(|| a.subject.cmp(&b.subject))
            .then_with(|| a.namespace.cmp(&b.namespace))
    });
    json_ok(&AdminGrantsResponse { grants })
}

pub fn grant(core: &mut terrane_host::HostCore, request: &mut Request) -> Resp {
    let mut body = String::new();
    if request.as_reader().read_to_string(&mut body).is_err() {
        return json_error(400, "cannot read request body");
    }
    let parsed = match GrantRequest::deserialize_json(&body) {
        Ok(parsed) => parsed,
        Err(e) => return json_error(400, &format!("bad grant body: {e}")),
    };
    let subject = if parsed.subject.trim().is_empty() {
        LOCAL_OWNER_SUBJECT.to_string()
    } else {
        parsed.subject
    };
    match terrane_host::dispatch_on_core(
        core,
        "auth.grant",
        &[subject, parsed.app, parsed.namespace],
    ) {
        Ok(outcome) => json_ok(&GrantResponse {
            records: outcome.records.len(),
            output: outcome.output,
        }),
        Err(e) => json_error(400, &e),
    }
}
