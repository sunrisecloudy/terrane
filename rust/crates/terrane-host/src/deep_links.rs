use std::path::Path;

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use serde_json::json;
use terrane_cap_interface::parse_item_uri;

use crate::{dispatch_on_core, HostCore};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenOutcome {
    Opened { app: String },
    Delivered { app: String, kind: String },
    ImportedFile { app: String, name: String },
}

impl OpenOutcome {
    pub fn message(&self) -> String {
        match self {
            OpenOutcome::Opened { app } => format!("opened {app}"),
            OpenOutcome::Delivered { app, kind } => {
                format!("delivered {kind} payload to {app}")
            }
            OpenOutcome::ImportedFile { app, name } => {
                format!("imported file {name} to {app}")
            }
        }
    }
}

pub fn open_target(target: &str) -> Result<OpenOutcome, String> {
    let mut core = crate::open()?;
    open_target_on_core(&mut core, target)
}

pub fn open_target_on_core(core: &mut HostCore, target: &str) -> Result<OpenOutcome, String> {
    if target.starts_with("terrane://") {
        return open_url_on_core(core, target);
    }
    open_file_on_core(core, Path::new(target))
}

fn open_url_on_core(core: &mut HostCore, url: &str) -> Result<OpenOutcome, String> {
    if let Ok(item) = parse_item_uri(url) {
        let payload = json!({ "item": item.item }).to_string();
        deliver(core, &item.app, "link", payload)?;
        return Ok(OpenOutcome::Delivered {
            app: item.app,
            kind: "link".to_string(),
        });
    }
    if let Some(rest) = url.strip_prefix("terrane://open/") {
        let app = route_app(rest)?;
        ensure_app(core, &app)?;
        return Ok(OpenOutcome::Opened { app });
    }
    if let Some(rest) = url.strip_prefix("terrane://send/") {
        let (route, query) = split_query(rest);
        let app = route_app(route)?;
        let query = parse_query(query)?;
        let kind = query_value(&query, "kind").unwrap_or("link");
        if kind != "link" {
            return Err(format!(
                "terrane://send only delivers common.receive(\"link\", ...), got kind={kind:?}"
            ));
        }
        let payload = query_value(&query, "payload").unwrap_or("{}").to_string();
        if payload.len() > terrane_cap_interop::MAX_ARGS_BYTES {
            return Err(format!(
                "scheme payload exceeds {} bytes",
                terrane_cap_interop::MAX_ARGS_BYTES
            ));
        }
        deliver(core, &app, "link", payload)?;
        return Ok(OpenOutcome::Delivered {
            app,
            kind: "link".to_string(),
        });
    }
    Err(format!("unsupported Terrane URL: {url}"))
}

fn open_file_on_core(core: &mut HostCore, path: &Path) -> Result<OpenOutcome, String> {
    let ext = path
        .extension()
        .and_then(|ext| ext.to_str())
        .ok_or_else(|| format!("file has no extension: {}", path.display()))?
        .to_ascii_lowercase();
    let claimants = filetype_claimants(core, &ext);
    let (app, mime) = match claimants.as_slice() {
        [] => return Err(format!("no app registered for .{ext}")),
        [(app, mime)] => (app.clone(), mime.clone()),
        _ => {
            let apps = claimants
                .iter()
                .map(|(app, _)| app.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            return Err(format!(
                "multiple apps registered for .{ext}; picker required: {apps}"
            ));
        }
    };
    let bytes = std::fs::read(path).map_err(|e| format!("read file {}: {e}", path.display()))?;
    if bytes.len() > terrane_cap_blob::MAX_BLOB_SIZE {
        return Err(format!(
            "file import exceeds blob cap of {} bytes",
            terrane_cap_blob::MAX_BLOB_SIZE
        ));
    }
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("file name is not valid UTF-8: {}", path.display()))?
        .to_string();
    let hash = terrane_cap_interop::sha256_hex(&bytes);
    let args = vec![
        app.clone(),
        name.clone(),
        mime.clone(),
        B64.encode(&bytes),
    ];
    dispatch_on_core(core, "blob.put", &args)?;
    let payload = json!({
        "name": name,
        "hash": hash,
        "size": bytes.len(),
        "mime": mime,
    })
    .to_string();
    deliver(core, &app, "blob", payload)?;
    Ok(OpenOutcome::ImportedFile { app, name })
}

fn deliver(core: &mut HostCore, app: &str, kind: &str, payload: String) -> Result<(), String> {
    let args = vec![app.to_string(), kind.to_string(), payload];
    dispatch_on_core(core, "app.link.deliver", &args)?;
    Ok(())
}

fn ensure_app(core: &HostCore, app: &str) -> Result<(), String> {
    if core.state().app.apps.contains_key(app) {
        Ok(())
    } else {
        Err(format!("app not found: {app}"))
    }
}

fn filetype_claimants(core: &HostCore, ext: &str) -> Vec<(String, String)> {
    let mut claimants = Vec::new();
    for (app, links) in &core.state().app.links {
        for link in links {
            if link.kind != "filetype" {
                continue;
            }
            let Some((registered_ext, mime)) = link.spec.split_once(':') else {
                continue;
            };
            if registered_ext.eq_ignore_ascii_case(ext) {
                claimants.push((app.clone(), mime.to_string()));
            }
        }
    }
    claimants
}

fn route_app(route: &str) -> Result<String, String> {
    let app = route
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .trim_matches('/');
    if app.is_empty() {
        return Err("Terrane URL missing app id".into());
    }
    if !app
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(format!("unsafe app id in Terrane URL: {app:?}"));
    }
    Ok(app.to_string())
}

fn split_query(rest: &str) -> (&str, &str) {
    rest.split_once('?').unwrap_or((rest, ""))
}

fn parse_query(query: &str) -> Result<Vec<(String, String)>, String> {
    let mut pairs = Vec::new();
    for raw_pair in query.split('&').filter(|pair| !pair.is_empty()) {
        let (key, value) = raw_pair.split_once('=').unwrap_or((raw_pair, ""));
        pairs.push((percent_decode(key)?, percent_decode(value)?));
    }
    Ok(pairs)
}

fn query_value<'a>(pairs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    pairs
        .iter()
        .find(|(candidate, _)| candidate == key)
        .map(|(_, value)| value.as_str())
}

fn percent_decode(value: &str) -> Result<String, String> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'%' => {
                let hi = *bytes
                    .get(i + 1)
                    .ok_or_else(|| format!("bad percent escape: {value}"))?;
                let lo = *bytes
                    .get(i + 2)
                    .ok_or_else(|| format!("bad percent escape: {value}"))?;
                out.push((from_hex(hi)? << 4) | from_hex(lo)?);
                i += 3;
            }
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|e| format!("percent-decoded value is not UTF-8: {e}"))
}

fn from_hex(b: u8) -> Result<u8, String> {
    match b {
        b'0'..=b'9' => Ok(b - b'0'),
        b'a'..=b'f' => Ok(10 + b - b'a'),
        b'A'..=b'F' => Ok(10 + b - b'A'),
        _ => Err(format!(
            "bad hex digit in percent escape: {}",
            char::from(b)
        )),
    }
}
