use std::env;
use std::fs;
use std::path::PathBuf;

const REACT_JS: &str = include_str!("../vendor/react/react.production.min.js");
const REACT_DOM_JS: &str = include_str!("../vendor/react/react-dom.production.min.js");
const REACT_LICENSE: &str = include_str!("../vendor/react/LICENSE.react.txt");
const REACT_DOM_LICENSE: &str = include_str!("../vendor/react/LICENSE.react-dom.txt");

fn main() {
    if let Err(e) = run() {
        eprintln!("terrane-app-build: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let app_dir = env::args()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("usage: terrane-app-build <app-dir>")?;
    let manifest_path = app_dir.join("manifest.json");
    let manifest = fs::read_to_string(&manifest_path)
        .map_err(|e| format!("read {}: {e}", manifest_path.display()))?;
    let title = json_string_field(&manifest, "name").unwrap_or_else(|| "Terrane App".to_string());
    let entry = json_string_field(&manifest, "entry").unwrap_or_else(|| "src/main.jsx".to_string());
    let styles = json_string_array_field(&manifest, "styles");

    let entry_path = app_dir.join(&entry);
    let source = fs::read_to_string(&entry_path)
        .map_err(|e| format!("read frontend entry {}: {e}", entry_path.display()))?;
    let transformed = transform_jsx(&source)?;

    let dist = app_dir.join("dist");
    if dist.exists() {
        fs::remove_dir_all(&dist).map_err(|e| format!("clear {}: {e}", dist.display()))?;
    }
    let assets = dist.join("assets");
    fs::create_dir_all(&assets).map_err(|e| format!("create {}: {e}", assets.display()))?;

    fs::write(assets.join("react.production.min.js"), REACT_JS)
        .map_err(|e| format!("write react asset: {e}"))?;
    fs::write(assets.join("react-dom.production.min.js"), REACT_DOM_JS)
        .map_err(|e| format!("write react-dom asset: {e}"))?;
    fs::write(
        assets.join("react-licenses.txt"),
        format!("React:\n{REACT_LICENSE}\n\nReact DOM:\n{REACT_DOM_LICENSE}"),
    )
    .map_err(|e| format!("write react license asset: {e}"))?;
    fs::write(assets.join("app.js"), transformed).map_err(|e| format!("write app.js: {e}"))?;

    let mut stylesheet_links = String::new();
    for (index, style) in styles.iter().enumerate() {
        let source_path = app_dir.join(style);
        let file_name = if styles.len() == 1 {
            "app.css".to_string()
        } else {
            format!("app-{index}.css")
        };
        fs::copy(&source_path, assets.join(&file_name))
            .map_err(|e| format!("copy stylesheet {}: {e}", source_path.display()))?;
        stylesheet_links.push_str(&format!(
            "<link rel=\"stylesheet\" href=\"assets/{}\">\n",
            html_attr(&file_name)
        ));
    }

    let html = format!(
        "<!doctype html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1\">\n<title>{}</title>\n{}</head>\n<body>\n<div id=\"root\"></div>\n<script src=\"assets/react.production.min.js\"></script>\n<script src=\"assets/react-dom.production.min.js\"></script>\n<script src=\"assets/app.js\"></script>\n</body>\n</html>\n",
        html_text(&title),
        stylesheet_links
    );
    fs::write(dist.join("index.html"), html).map_err(|e| format!("write index.html: {e}"))?;
    println!("built {}", dist.display());
    Ok(())
}

fn json_string_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let after = json.split(&needle).nth(1)?;
    let after_colon = after.split_once(':')?.1.trim_start();
    parse_json_string(after_colon).map(|(s, _)| s)
}

fn json_string_array_field(json: &str, key: &str) -> Vec<String> {
    let needle = format!("\"{key}\"");
    let Some(after) = json.split(&needle).nth(1) else {
        return Vec::new();
    };
    let Some(after_colon) = after.split_once(':').map(|(_, value)| value.trim_start()) else {
        return Vec::new();
    };
    let Some(mut rest) = after_colon.strip_prefix('[') else {
        return Vec::new();
    };
    let mut out = Vec::new();
    loop {
        rest = rest.trim_start();
        if rest.starts_with(']') {
            break;
        }
        let Some((value, next)) = parse_json_string(rest) else {
            break;
        };
        out.push(value);
        rest = next.trim_start();
        if let Some(next) = rest.strip_prefix(',') {
            rest = next;
        }
    }
    out
}

fn parse_json_string(input: &str) -> Option<(String, &str)> {
    let mut chars = input.char_indices();
    if chars.next()?.1 != '"' {
        return None;
    }
    let mut out = String::new();
    let mut escaped = false;
    for (index, ch) in chars {
        if escaped {
            out.push(match ch {
                '"' => '"',
                '\\' => '\\',
                '/' => '/',
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if ch == '\\' {
            escaped = true;
        } else if ch == '"' {
            return Some((out, &input[index + ch.len_utf8()..]));
        } else {
            out.push(ch);
        }
    }
    None
}

fn transform_jsx(source: &str) -> Result<String, String> {
    let mut parser = JsxParser::new(source);
    parser.parse_js()
}

struct JsxParser<'a> {
    input: &'a str,
    pos: usize,
}

struct Element {
    tag: String,
    attrs: Vec<(String, AttrValue)>,
    children: Vec<String>,
}

enum AttrValue {
    String(String),
    Expr(String),
    Bool,
}

impl<'a> JsxParser<'a> {
    fn new(input: &'a str) -> Self {
        Self { input, pos: 0 }
    }

    fn parse_js(&mut self) -> Result<String, String> {
        let mut out = String::new();
        while self.pos < self.input.len() {
            let ch = self.peek_char().unwrap();
            if ch == '<' && self.peek_next_is_tag_start() {
                out.push_str(&self.parse_element_expr()?);
            } else if ch == '"' || ch == '\'' || ch == '`' {
                out.push_str(&self.take_string(ch)?);
            } else if self.starts_with("//") {
                out.push_str(&self.take_line_comment());
            } else if self.starts_with("/*") {
                out.push_str(&self.take_block_comment()?);
            } else {
                out.push(ch);
                self.pos += ch.len_utf8();
            }
        }
        Ok(out)
    }

    fn parse_element_expr(&mut self) -> Result<String, String> {
        let element = self.parse_element()?;
        Ok(render_element(element))
    }

    fn parse_element(&mut self) -> Result<Element, String> {
        self.expect('<')?;
        let tag = self.take_name()?;
        let mut attrs = Vec::new();
        loop {
            self.skip_ws();
            if self.starts_with("/>") {
                self.pos += 2;
                return Ok(Element {
                    tag,
                    attrs,
                    children: Vec::new(),
                });
            }
            if self.peek_char() == Some('>') {
                self.pos += 1;
                break;
            }
            attrs.push(self.parse_attr()?);
        }

        let mut children = Vec::new();
        loop {
            if self.starts_with("</") {
                self.pos += 2;
                let close = self.take_name()?;
                if close != tag {
                    return Err(format!("expected closing </{tag}>, got </{close}>"));
                }
                self.skip_ws();
                self.expect('>')?;
                break;
            }
            if self.pos >= self.input.len() {
                return Err(format!("unclosed JSX element <{tag}>"));
            }
            if self.peek_char() == Some('<') && self.peek_next_is_tag_start() {
                children.push(self.parse_element_expr()?);
            } else if self.peek_char() == Some('{') {
                let expr = self.take_braced_expr()?;
                if !expr.trim().is_empty() {
                    children.push(expr.trim().to_string());
                }
            } else {
                let text = self.take_text_node();
                let collapsed = collapse_ws(&text);
                if !collapsed.is_empty() {
                    children.push(format!("{collapsed:?}"));
                }
            }
        }

        Ok(Element {
            tag,
            attrs,
            children,
        })
    }

    fn parse_attr(&mut self) -> Result<(String, AttrValue), String> {
        let name = self.take_attr_name()?;
        self.skip_ws();
        if self.peek_char() != Some('=') {
            return Ok((name, AttrValue::Bool));
        }
        self.pos += 1;
        self.skip_ws();
        match self.peek_char() {
            Some('"') | Some('\'') => {
                let quote = self.peek_char().unwrap();
                let raw = self.take_string(quote)?;
                Ok((name, AttrValue::String(unquote_js_string(&raw))))
            }
            Some('{') => Ok((name, AttrValue::Expr(self.take_braced_expr()?))),
            other => Err(format!(
                "bad JSX attribute value at {:?}: {other:?}",
                self.pos
            )),
        }
    }

    fn take_name(&mut self) -> Result<String, String> {
        let start = self.pos;
        while let Some(ch) = self.peek_char() {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.' {
                self.pos += ch.len_utf8();
            } else {
                break;
            }
        }
        if self.pos == start {
            Err(format!("expected JSX tag name at {}", self.pos))
        } else {
            Ok(self.input[start..self.pos].to_string())
        }
    }

    fn take_attr_name(&mut self) -> Result<String, String> {
        self.take_name()
    }

    fn take_braced_expr(&mut self) -> Result<String, String> {
        self.expect('{')?;
        let start = self.pos;
        let mut depth = 1usize;
        while self.pos < self.input.len() {
            let ch = self.peek_char().unwrap();
            if ch == '"' || ch == '\'' || ch == '`' {
                self.take_string(ch)?;
                continue;
            }
            if self.starts_with("//") {
                self.take_line_comment();
                continue;
            }
            if self.starts_with("/*") {
                self.take_block_comment()?;
                continue;
            }
            if ch == '{' {
                depth += 1;
            } else if ch == '}' {
                depth -= 1;
                if depth == 0 {
                    let expr = self.input[start..self.pos].to_string();
                    self.pos += 1;
                    return Ok(expr);
                }
            }
            self.pos += ch.len_utf8();
        }
        Err("unclosed JSX expression".into())
    }

    fn take_text_node(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.input.len() {
            if self.starts_with("</")
                || self.peek_char() == Some('{')
                || (self.peek_char() == Some('<') && self.peek_next_is_tag_start())
            {
                break;
            }
            let ch = self.peek_char().unwrap();
            self.pos += ch.len_utf8();
        }
        html_unescape(&self.input[start..self.pos])
    }

    fn take_string(&mut self, quote: char) -> Result<String, String> {
        let start = self.pos;
        self.pos += quote.len_utf8();
        let mut escaped = false;
        while self.pos < self.input.len() {
            let ch = self.peek_char().unwrap();
            self.pos += ch.len_utf8();
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == quote {
                return Ok(self.input[start..self.pos].to_string());
            }
        }
        Err("unclosed JS string".into())
    }

    fn take_line_comment(&mut self) -> String {
        let start = self.pos;
        while self.pos < self.input.len() {
            let ch = self.peek_char().unwrap();
            self.pos += ch.len_utf8();
            if ch == '\n' {
                break;
            }
        }
        self.input[start..self.pos].to_string()
    }

    fn take_block_comment(&mut self) -> Result<String, String> {
        let start = self.pos;
        let Some(end) = self.input[self.pos + 2..].find("*/") else {
            return Err("unclosed block comment".into());
        };
        self.pos += end + 4;
        Ok(self.input[start..self.pos].to_string())
    }

    fn skip_ws(&mut self) {
        while self.peek_char().is_some_and(char::is_whitespace) {
            self.pos += self.peek_char().unwrap().len_utf8();
        }
    }

    fn expect(&mut self, expected: char) -> Result<(), String> {
        if self.peek_char() == Some(expected) {
            self.pos += expected.len_utf8();
            Ok(())
        } else {
            Err(format!("expected {expected:?} at {}", self.pos))
        }
    }

    fn starts_with(&self, s: &str) -> bool {
        self.input[self.pos..].starts_with(s)
    }

    fn peek_char(&self) -> Option<char> {
        self.input[self.pos..].chars().next()
    }

    fn peek_next_is_tag_start(&self) -> bool {
        self.input[self.pos + 1..]
            .chars()
            .next()
            .is_some_and(|ch| ch.is_ascii_alphabetic())
    }
}

fn render_element(element: Element) -> String {
    let tag = if element
        .tag
        .chars()
        .next()
        .is_some_and(|ch| ch.is_ascii_uppercase())
    {
        element.tag
    } else {
        format!("{:?}", element.tag)
    };
    let props = render_props(&element.attrs);
    let mut args = vec![tag, props];
    args.extend(element.children);
    format!("React.createElement({})", args.join(", "))
}

fn render_props(attrs: &[(String, AttrValue)]) -> String {
    if attrs.is_empty() {
        return "null".into();
    }
    let items = attrs
        .iter()
        .map(|(name, value)| {
            let key = if is_identifier(name) {
                name.clone()
            } else {
                format!("{name:?}")
            };
            let value = match value {
                AttrValue::String(s) => format!("{s:?}"),
                AttrValue::Expr(expr) => expr.trim().to_string(),
                AttrValue::Bool => "true".to_string(),
            };
            format!("{key}: {value}")
        })
        .collect::<Vec<_>>();
    format!("{{ {} }}", items.join(", "))
}

fn is_identifier(input: &str) -> bool {
    let mut chars = input.chars();
    chars
        .next()
        .is_some_and(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch == '$' || ch.is_ascii_alphanumeric())
}

fn unquote_js_string(input: &str) -> String {
    input[1..input.len() - 1].to_string()
}

fn collapse_ws(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn html_unescape(input: &str) -> String {
    input
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
}

fn html_text(input: &str) -> String {
    input
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

fn html_attr(input: &str) -> String {
    html_text(input).replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_frontend_build_fields() {
        let manifest = r#"{
          "name": "Demo",
          "frontend": {
            "tool": "terrane-app-build",
            "entry": "src/main.jsx",
            "styles": ["src/app.css"]
          }
        }"#;

        assert_eq!(json_string_field(manifest, "name").as_deref(), Some("Demo"));
        assert_eq!(
            json_string_field(manifest, "entry").as_deref(),
            Some("src/main.jsx")
        );
        assert_eq!(
            json_string_array_field(manifest, "styles"),
            vec!["src/app.css".to_string()]
        );
    }

    #[test]
    fn transforms_nested_jsx_to_react_calls() {
        let source = r#"function App() {
          return <main className="card"><h1>{title}</h1><input disabled /></main>;
        }"#;

        let out = transform_jsx(source).unwrap();

        assert!(out.contains("React.createElement(\"main\""));
        assert!(out.contains("className: \"card\""));
        assert!(out.contains("React.createElement(\"h1\", null, title)"));
        assert!(out.contains("React.createElement(\"input\", { disabled: true })"));
    }
}
