use super::{json_string_value, top_level_fields};

#[test]
fn top_level_parser_ignores_nested_ids() {
    let raw = r#"{"jsonrpc":"2.0","method":"ping","params":{"item":{"id":555}},"id":8}"#;
    let fields = top_level_fields(raw);
    let field = |name: &str| fields.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);

    assert_eq!(field("id"), Some("8"));
    assert_eq!(field("method").and_then(json_string_value), Some("ping"));
    assert_eq!(field("params"), Some(r#"{"item":{"id":555}}"#));
}
