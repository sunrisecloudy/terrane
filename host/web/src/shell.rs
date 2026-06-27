use terrane_api::AppsResponse;
use tiny_http::Response;

use crate::http::{header, json_error, Resp};

pub fn response(core: &terrane_host::HostCore, current_id: &str) -> Resp {
    let apps = terrane_host::list_apps(core);
    if !apps.apps.iter().any(|app| app.id == current_id) {
        return json_error(404, &format!("no such app: {current_id}"));
    }
    let body = render_shell(&apps, current_id);
    Response::from_data(body.into_bytes())
        .with_header(header("Content-Type", "text/html; charset=utf-8"))
}

fn render_shell(apps: &AppsResponse, current_id: &str) -> String {
    let current_name = apps
        .apps
        .iter()
        .find(|app| app.id == current_id)
        .map(|app| app.name.as_str())
        .unwrap_or(current_id);

    let mut items = String::new();
    for app in &apps.apps {
        if app.has_ui {
            let selected = if app.id == current_id {
                " aria-current=\"page\""
            } else {
                ""
            };
            let class = if app.id == current_id {
                "app-link selected"
            } else {
                "app-link"
            };
            items.push_str(&format!(
                "<a class=\"{class}\" href=\"/apps/{id}/\"{selected}>\
                 <span>{name}</span><small>{id}</small></a>",
                class = class,
                id = html_escape(&app.id),
                selected = selected,
                name = html_escape(&app.name)
            ));
        } else {
            items.push_str(&format!(
                "<div class=\"app-link disabled\"><span>{name}</span><small>{id} - no UI</small></div>",
                name = html_escape(&app.name),
                id = html_escape(&app.id)
            ));
        }
    }

    format!(
        "<!doctype html>
<html lang=\"en\">
<head>
<meta charset=\"utf-8\">
<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">
<title>{title} - Terrane</title>
<style>
:root {{ color-scheme: light dark; }}
* {{ box-sizing: border-box; }}
body {{
  margin: 0;
  min-height: 100vh;
  font: 13px -apple-system, BlinkMacSystemFont, \"Segoe UI\", sans-serif;
  background: Canvas;
  color: CanvasText;
}}
.shell {{
  display: grid;
  grid-template-columns: 240px minmax(0, 1fr);
  min-height: 100vh;
}}
.sidebar {{
  border-right: 1px solid color-mix(in srgb, CanvasText 14%, transparent);
  background: color-mix(in srgb, Canvas 94%, CanvasText 6%);
  padding: 14px;
  min-width: 0;
}}
.brand {{
  margin: 2px 0 14px;
  font-size: 14px;
  font-weight: 700;
}}
.app-list {{
  display: flex;
  flex-direction: column;
  gap: 6px;
}}
.app-link {{
  display: block;
  padding: 9px 10px;
  border: 1px solid transparent;
  border-radius: 8px;
  color: inherit;
  text-decoration: none;
}}
.app-link span {{
  display: block;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}}
.app-link small {{
  display: block;
  margin-top: 2px;
  color: color-mix(in srgb, CanvasText 55%, transparent);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}}
.app-link:hover {{
  background: color-mix(in srgb, CanvasText 7%, transparent);
}}
.app-link.selected {{
  border-color: #0071e3;
  background: color-mix(in srgb, #0071e3 12%, transparent);
}}
.app-link.disabled {{
  cursor: default;
  opacity: 0.55;
}}
.stage {{
  min-width: 0;
  min-height: 100vh;
  display: grid;
  grid-template-rows: 0 minmax(0, 1fr);
}}
.title {{
  width: 1px;
  height: 1px;
  margin: -1px;
  overflow: hidden;
  clip: rect(0 0 0 0);
}}
iframe {{
  width: 100%;
  height: 100%;
  border: 0;
  background: Canvas;
}}
@media (max-width: 760px) {{
  .shell {{ grid-template-columns: 1fr; grid-template-rows: auto minmax(0, 1fr); }}
  .sidebar {{ border-right: 0; border-bottom: 1px solid color-mix(in srgb, CanvasText 14%, transparent); }}
  .app-list {{ flex-direction: row; overflow-x: auto; padding-bottom: 2px; }}
  .app-link {{ min-width: 150px; }}
  .stage {{ min-height: calc(100vh - 112px); }}
}}
</style>
</head>
<body>
<div class=\"shell\">
  <nav class=\"sidebar\" aria-label=\"Apps\">
    <div class=\"brand\">Terrane</div>
    <div class=\"app-list\">{items}</div>
  </nav>
  <main class=\"stage\">
    <h1 class=\"title\">{title}</h1>
    <iframe title=\"{title}\" src=\"/apps/{id}/__terrane/frame/\"></iframe>
  </main>
</div>
</body>
</html>",
        title = html_escape(current_name),
        items = items,
        id = html_escape(current_id)
    )
}

fn html_escape(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(ch),
        }
    }
    out
}
