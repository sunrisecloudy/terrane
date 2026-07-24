use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::Deserialize;

const DISCOVERY_FILE: &str = "mcp-gui.json";

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Discovery {
    version: u32,
    endpoint: String,
    health: String,
    token: String,
    #[allow(dead_code)]
    pid: i32,
}

pub struct GuiProxy {
    endpoint: String,
    token: String,
    agent: ureq::Agent,
}

impl GuiProxy {
    pub fn connect(home: &Path) -> Result<Self, String> {
        let path = discovery_path(home);
        let raw = fs::read_to_string(&path)
            .map_err(|e| format!("read GUI MCP discovery {}: {e}", path.display()))?;
        let discovery: Discovery = serde_json::from_str(&raw)
            .map_err(|e| format!("parse GUI MCP discovery {}: {e}", path.display()))?;
        if discovery.version != 1 {
            return Err(format!(
                "unsupported GUI MCP discovery version {}",
                discovery.version
            ));
        }
        if !is_loopback_http(&discovery.endpoint) || !is_loopback_http(&discovery.health) {
            return Err("GUI MCP discovery must use a loopback HTTP endpoint".into());
        }
        if discovery.token.is_empty() {
            return Err("GUI MCP discovery has an empty bearer token".into());
        }

        let agent = ureq::AgentBuilder::new()
            .redirects(0)
            .try_proxy_from_env(false)
            .timeout_connect(Duration::from_secs(2))
            // The GUI may present a trusted permission sheet and retry the MCP
            // request after the operator responds.
            .timeout_read(Duration::from_secs(125))
            .timeout_write(Duration::from_secs(5))
            .build();
        let health = agent
            .get(&discovery.health)
            .set("Authorization", &format!("Bearer {}", discovery.token))
            .call()
            .map_err(|e| format!("connect to Terrane GUI MCP endpoint: {e}"))?;
        if health.status() != 200 {
            return Err(format!(
                "Terrane GUI MCP health check returned HTTP {}",
                health.status()
            ));
        }

        Ok(Self {
            endpoint: discovery.endpoint,
            token: discovery.token,
            agent,
        })
    }

    pub fn forward(&self, raw: &str) -> Result<Option<String>, String> {
        let response = self
            .agent
            .post(&self.endpoint)
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_string(raw)
            .map_err(|e| format!("Terrane GUI MCP request failed: {e}"))?;
        let status = response.status();
        if status == 202 {
            return Ok(None);
        }
        if status != 200 {
            return Err(format!("Terrane GUI MCP returned HTTP {status}"));
        }
        response
            .into_string()
            .map(Some)
            .map_err(|e| format!("read Terrane GUI MCP response: {e}"))
    }
}

pub fn discovery_path(home: &Path) -> PathBuf {
    home.join(DISCOVERY_FILE)
}

fn is_loopback_http(value: &str) -> bool {
    let Some(rest) = value.strip_prefix("http://") else {
        return false;
    };
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.contains('@') {
        return false;
    }
    let (host, port) = if let Some(rest) = authority.strip_prefix("[::1]:") {
        ("::1", rest)
    } else if let Some((host, port)) = authority.rsplit_once(':') {
        (host, port)
    } else {
        return false;
    };
    matches!(host, "127.0.0.1" | "::1") && port.parse::<u16>().is_ok_and(|parsed| parsed != 0)
}

#[cfg(test)]
mod tests {
    use super::is_loopback_http;

    #[test]
    fn discovery_accepts_only_loopback_http() {
        assert!(is_loopback_http("http://127.0.0.1:1234/mcp"));
        assert!(is_loopback_http("http://[::1]:1234/mcp"));
        assert!(!is_loopback_http("http://localhost:1234/mcp"));
        assert!(!is_loopback_http("https://127.0.0.1:1234/mcp"));
        assert!(!is_loopback_http("http://example.com/mcp"));
        assert!(!is_loopback_http("http://localhost:1234@example.com/mcp"));
        assert!(!is_loopback_http("http://127.0.0.1:0/mcp"));
    }
}
