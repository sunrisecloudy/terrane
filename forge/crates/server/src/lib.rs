//! Minimal Forge server spine.
//!
//! This crate is the Rust replacement surface that legacy-removal Phase 1.3 can
//! grow into: a server owns a [`WorkspaceCore`] and exposes a narrow JSON HTTP
//! bridge. It is intentionally small for the first slice: no async runtime, no TLS,
//! no legacy package-control compatibility yet. Later slices can layer the v0.4
//! `/control` tool compatibility and WebSocket sync transport over the same core
//! command/event surface.

use forge_core::WorkspaceCore;
use forge_domain::{
    ActorContext, CoreCommand, CoreError, CoreEvent, CoreResponse, RequestId, Result, WorkspaceId,
};
use serde::Serialize;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::Mutex;

const AUTH_HEADER: &str = "x-forge-server-token";
const MAX_HTTP_BODY_BYTES: usize = 1024 * 1024;

/// Shared server state: one workspace core protected by a mutex so the std HTTP
/// listener can serve one request at a time without exposing raw SQLite access to
/// callers.
pub struct ForgeServer {
    core: Mutex<WorkspaceCore>,
    trusted_actor: ActorContext,
    workspace_id: WorkspaceId,
    auth_token: Option<String>,
}

impl ForgeServer {
    /// Create an in-memory server workspace for tests and ephemeral embedded use.
    pub fn in_memory(workspace_id: impl Into<String>) -> Result<Self> {
        let workspace_id = workspace_id.into();
        Ok(ForgeServer {
            core: Mutex::new(WorkspaceCore::in_memory(workspace_id.clone())?),
            trusted_actor: ActorContext::owner("forge-server"),
            workspace_id: WorkspaceId::new(workspace_id),
            auth_token: None,
        })
    }

    /// Open a file-backed server workspace.
    pub fn open(path: impl AsRef<Path>, workspace_id: impl Into<String>) -> Result<Self> {
        let workspace_id = workspace_id.into();
        Ok(ForgeServer {
            core: Mutex::new(WorkspaceCore::open(path, workspace_id.clone())?),
            trusted_actor: ActorContext::owner("forge-server"),
            workspace_id: WorkspaceId::new(workspace_id),
            auth_token: None,
        })
    }

    /// Require a bearer token before HTTP bridge/event requests can touch the core.
    pub fn require_auth_token(mut self, token: impl Into<String>) -> Result<Self> {
        let token = token.into();
        if token.trim().is_empty() {
            return Err(CoreError::ValidationError(
                "forge-server auth token must not be empty".into(),
            ));
        }
        self.auth_token = Some(token);
        Ok(self)
    }

    /// Handle one parsed HTTP request.
    pub fn handle_http(&self, method: &str, path: &str, body: &[u8]) -> HttpResponse {
        self.handle_http_with_headers(method, path, &BTreeMap::new(), body)
    }

    /// Handle one parsed HTTP request with headers supplied by the socket shim.
    pub fn handle_http_with_headers(
        &self,
        method: &str,
        path: &str,
        headers: &BTreeMap<String, String>,
        body: &[u8],
    ) -> HttpResponse {
        match (method, path) {
            ("GET", "/health") => json_response(
                200,
                &serde_json::json!({
                    "ok": true,
                    "service": "forge-server",
                    "status": "ok",
                }),
            ),
            ("POST", "/bridge") => match self.authorize(headers) {
                Ok(()) => self.handle_bridge(body),
                Err(response) => response,
            },
            ("POST", "/events/drain") => match self.authorize(headers) {
                Ok(()) => self.handle_event_drain(),
                Err(response) => response,
            },
            _ => json_error(
                404,
                CoreError::ValidationError(format!("unknown route {method} {path}")),
            ),
        }
    }

    fn handle_bridge(&self, body: &[u8]) -> HttpResponse {
        let mut command: CoreCommand = match serde_json::from_slice(body) {
            Ok(command) => command,
            Err(e) => {
                return json_response(
                    400,
                    &CoreResponse::err(
                        RequestId::new("server"),
                        CoreError::ValidationError(format!(
                            "/bridge body is not a valid CoreCommand: {e}"
                        )),
                    ),
                )
            }
        };
        command.actor = self.trusted_actor.clone();
        command.workspace_id = self.workspace_id.clone();
        let mut core = match self.lock_core() {
            Ok(core) => core,
            Err(response) => return response,
        };
        match catch_unwind(AssertUnwindSafe(|| core.handle(command))) {
            Ok(response) => json_response(200, &response),
            Err(_) => json_error(
                500,
                CoreError::RuntimeError("server bridge command panicked".into()),
            ),
        }
    }

    fn handle_event_drain(&self) -> HttpResponse {
        #[derive(Serialize)]
        struct EventDrain {
            ok: bool,
            events: Vec<CoreEvent>,
        }

        let mut core = match self.lock_core() {
            Ok(core) => core,
            Err(response) => return response,
        };
        json_response(
            200,
            &EventDrain {
                ok: true,
                events: core.events_mut().drain(),
            },
        )
    }

    fn authorize(
        &self,
        headers: &BTreeMap<String, String>,
    ) -> std::result::Result<(), HttpResponse> {
        let Some(expected) = self.auth_token.as_deref() else {
            return Ok(());
        };

        if supplied_auth_token(headers) == Some(expected) {
            return Ok(());
        }

        let mut response = json_error(
            401,
            CoreError::PermissionDenied("forge-server auth token required".into()),
        );
        response
            .headers
            .insert("www-authenticate".into(), "Bearer".into());
        Err(response)
    }

    fn lock_core(
        &self,
    ) -> std::result::Result<std::sync::MutexGuard<'_, WorkspaceCore>, HttpResponse> {
        self.core.lock().map_err(|_| {
            json_error(
                500,
                CoreError::RuntimeError("server workspace lock is poisoned".into()),
            )
        })
    }
}

/// HTTP response returned by the std server shim and by unit tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    pub status: u16,
    pub reason: &'static str,
    pub headers: BTreeMap<String, String>,
    pub body: Vec<u8>,
}

impl HttpResponse {
    fn new(status: u16, body: Vec<u8>) -> Self {
        let reason = match status {
            200 => "OK",
            400 => "Bad Request",
            401 => "Unauthorized",
            404 => "Not Found",
            413 => "Payload Too Large",
            500 => "Internal Server Error",
            _ => "OK",
        };
        let mut headers = BTreeMap::new();
        headers.insert("content-type".into(), "application/json".into());
        headers.insert("content-length".into(), body.len().to_string());
        headers.insert("connection".into(), "close".into());
        HttpResponse {
            status,
            reason,
            headers,
            body,
        }
    }

    fn write_to(&self, stream: &mut TcpStream) -> Result<()> {
        write!(stream, "HTTP/1.1 {} {}\r\n", self.status, self.reason).map_err(io_err)?;
        for (name, value) in &self.headers {
            write!(stream, "{name}: {value}\r\n").map_err(io_err)?;
        }
        stream.write_all(b"\r\n").map_err(io_err)?;
        stream.write_all(&self.body).map_err(io_err)?;
        Ok(())
    }

    pub fn json_value(&self) -> Result<serde_json::Value> {
        serde_json::from_slice(&self.body)
            .map_err(|e| CoreError::ValidationError(format!("response body is not JSON: {e}")))
    }
}

/// Run a blocking HTTP listener forever.
pub fn serve_blocking(bind: &str, server: &ForgeServer) -> Result<()> {
    let listener = TcpListener::bind(bind)
        .map_err(|e| CoreError::PlatformUnavailable(format!("bind {bind}: {e}")))?;
    for stream in listener.incoming() {
        match stream {
            Ok(mut stream) => {
                let response = match read_request(&mut stream) {
                    Ok(request) => server.handle_http_with_headers(
                        &request.method,
                        &request.path,
                        &request.headers,
                        &request.body,
                    ),
                    Err(error) => json_error(status_for_error(&error), error),
                };
                response.write_to(&mut stream)?;
            }
            Err(e) => return Err(CoreError::PlatformUnavailable(format!("accept: {e}"))),
        }
    }
    Ok(())
}

#[derive(Debug)]
struct HttpRequest {
    method: String,
    path: String,
    headers: BTreeMap<String, String>,
    body: Vec<u8>,
}

fn read_request(stream: &mut TcpStream) -> Result<HttpRequest> {
    read_request_from_reader(BufReader::new(stream))
}

fn read_request_from_reader<R: BufRead>(mut reader: R) -> Result<HttpRequest> {
    let mut request_line = String::new();
    reader.read_line(&mut request_line).map_err(io_err)?;
    let mut parts = request_line.split_whitespace();
    let method = parts
        .next()
        .ok_or_else(|| CoreError::ValidationError("HTTP request line missing method".into()))?
        .to_string();
    let path = parts
        .next()
        .ok_or_else(|| CoreError::ValidationError("HTTP request line missing path".into()))?
        .to_string();

    let mut content_length = 0usize;
    let mut headers = BTreeMap::new();
    loop {
        let mut line = String::new();
        reader.read_line(&mut line).map_err(io_err)?;
        if line == "\r\n" || line == "\n" || line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_string();
            if name.eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse::<usize>().map_err(|e| {
                    CoreError::ValidationError(format!("invalid content-length: {e}"))
                })?;
            }
            headers.insert(name, value);
        }
    }

    if content_length > MAX_HTTP_BODY_BYTES {
        return Err(CoreError::ResourceLimitExceeded(format!(
            "HTTP request body exceeds {MAX_HTTP_BODY_BYTES} bytes"
        )));
    }

    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).map_err(io_err)?;
    Ok(HttpRequest {
        method,
        path,
        headers,
        body,
    })
}

fn supplied_auth_token(headers: &BTreeMap<String, String>) -> Option<&str> {
    if let Some(value) = headers.get("authorization") {
        return value
            .strip_prefix("Bearer ")
            .or_else(|| value.strip_prefix("bearer "));
    }
    headers.get(AUTH_HEADER).map(String::as_str)
}

fn status_for_error(error: &CoreError) -> u16 {
    match error {
        CoreError::ResourceLimitExceeded(_) => 413,
        _ => 400,
    }
}

fn json_response<T: Serialize>(status: u16, value: &T) -> HttpResponse {
    match serde_json::to_vec(value) {
        Ok(body) => HttpResponse::new(status, body),
        Err(e) => {
            let fallback = serde_json::json!({
                "ok": false,
                "error": {
                    "kind": "RuntimeError",
                    "detail": format!("serialize response: {e}"),
                },
            });
            let body = serde_json::to_vec(&fallback).unwrap_or_else(|_| {
                br#"{"ok":false,"error":{"kind":"RuntimeError","detail":"serialize response"}}"#
                    .to_vec()
            });
            HttpResponse::new(500, body)
        }
    }
}

fn json_error(status: u16, error: CoreError) -> HttpResponse {
    #[derive(Serialize)]
    struct ErrorEnvelope {
        ok: bool,
        error: CoreError,
    }

    json_response(status, &ErrorEnvelope { ok: false, error })
}

fn io_err(e: std::io::Error) -> CoreError {
    CoreError::PlatformUnavailable(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use forge_domain::{ActorContext, AppletId, WorkspaceId};

    fn owner_command(name: &str, payload: serde_json::Value) -> CoreCommand {
        CoreCommand {
            request_id: RequestId::new("req"),
            actor: ActorContext::owner("dev"),
            workspace_id: WorkspaceId::new("ws"),
            applet_id: None::<AppletId>,
            name: name.into(),
            payload,
        }
    }

    fn headers(entries: &[(&str, &str)]) -> BTreeMap<String, String> {
        entries
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn health_reports_ready() {
        let server = ForgeServer::in_memory("ws").unwrap();
        let response = server.handle_http("GET", "/health", b"");
        assert_eq!(response.status, 200);
        let body = response.json_value().unwrap();
        assert_eq!(body["ok"], serde_json::json!(true));
        assert_eq!(body["service"], serde_json::json!("forge-server"));
    }

    #[test]
    fn bridge_accepts_core_command_and_returns_core_response() {
        let server = ForgeServer::in_memory("ws").unwrap();
        let body =
            serde_json::to_vec(&owner_command("workspace.open", serde_json::json!({}))).unwrap();
        let response = server.handle_http("POST", "/bridge", &body);
        assert_eq!(response.status, 200);
        let body: CoreResponse = serde_json::from_slice(&response.body).unwrap();
        assert!(body.ok, "{:?}", body.error);
        assert_eq!(body.payload["workspace_id"], serde_json::json!("ws"));
    }

    #[test]
    fn bridge_requires_configured_auth_token() {
        let server = ForgeServer::in_memory("ws")
            .unwrap()
            .require_auth_token("secret")
            .unwrap();
        let body =
            serde_json::to_vec(&owner_command("workspace.open", serde_json::json!({}))).unwrap();

        let missing = server.handle_http_with_headers("POST", "/bridge", &BTreeMap::new(), &body);
        assert_eq!(missing.status, 401);
        assert_eq!(missing.headers["www-authenticate"], "Bearer");

        let wrong = server.handle_http_with_headers(
            "POST",
            "/bridge",
            &headers(&[("authorization", "Bearer nope")]),
            &body,
        );
        assert_eq!(wrong.status, 401);

        let ok = server.handle_http_with_headers(
            "POST",
            "/bridge",
            &headers(&[("authorization", "Bearer secret")]),
            &body,
        );
        assert_eq!(ok.status, 200);
    }

    #[test]
    fn bridge_uses_server_identity_instead_of_request_actor() {
        let server = ForgeServer::in_memory("trusted-ws")
            .unwrap()
            .require_auth_token("secret")
            .unwrap();
        let mut command = owner_command(
            "quota.set",
            serde_json::json!({
                "policy": {
                    "workspace_limit": 123456
                }
            }),
        );
        command.actor.role = forge_domain::Role::Viewer;
        command.workspace_id = WorkspaceId::new("attacker-ws");
        let body = serde_json::to_vec(&command).unwrap();

        let response = server.handle_http_with_headers(
            "POST",
            "/bridge",
            &headers(&[(AUTH_HEADER, "secret")]),
            &body,
        );
        assert_eq!(response.status, 200);
        let body: CoreResponse = serde_json::from_slice(&response.body).unwrap();
        assert!(body.ok, "{:?}", body.error);
    }

    #[test]
    fn bridge_rejects_malformed_command_json() {
        let server = ForgeServer::in_memory("ws").unwrap();
        let response = server.handle_http("POST", "/bridge", b"{");
        assert_eq!(response.status, 400);
        let body: CoreResponse = serde_json::from_slice(&response.body).unwrap();
        assert!(!body.ok);
        assert_eq!(body.error.unwrap().code(), "ValidationError");
    }

    #[test]
    fn file_backed_server_opens_workspace_and_handles_bridge() {
        let path = std::env::temp_dir().join(format!(
            "forge-server-{}-{}.sqlite",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&path);

        {
            let server = ForgeServer::open(&path, "ws").unwrap();
            assert!(path.exists());
            let body = serde_json::to_vec(&owner_command("workspace.open", serde_json::json!({})))
                .unwrap();
            let response = server.handle_http("POST", "/bridge", &body);
            assert_eq!(response.status, 200);
            let body: CoreResponse = serde_json::from_slice(&response.body).unwrap();
            assert!(body.ok, "{:?}", body.error);
        }

        {
            let server = ForgeServer::open(&path, "ws").unwrap();
            let response = server.handle_http("GET", "/health", b"");
            assert_eq!(response.status, 200);
        }

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn read_request_parses_headers_and_rejects_oversize_bodies() {
        let request = read_request_from_reader(std::io::Cursor::new(
            b"POST /bridge HTTP/1.1\r\nHost: localhost\r\nAuthorization: Bearer secret\r\nContent-Length: 2\r\n\r\n{}",
        ))
        .unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(request.path, "/bridge");
        assert_eq!(request.headers["authorization"], "Bearer secret");
        assert_eq!(request.body, b"{}");

        let error = read_request_from_reader(std::io::Cursor::new(format!(
            "POST /bridge HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
            MAX_HTTP_BODY_BYTES + 1
        )))
        .unwrap_err();
        assert_eq!(error.code(), "ResourceLimitExceeded");
        assert_eq!(status_for_error(&error), 413);
    }
}
