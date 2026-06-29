use super::{handle_json_rpc, json_string_value, top_level_fields};

#[test]
fn top_level_parser_ignores_nested_ids() {
    let raw = r#"{"jsonrpc":"2.0","method":"ping","params":{"item":{"id":555}},"id":8}"#;
    let fields = top_level_fields(raw);
    let field = |name: &str| fields.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);

    assert_eq!(field("id"), Some("8"));
    assert_eq!(field("method").and_then(json_string_value), Some("ping"));
    assert_eq!(field("params"), Some(r#"{"item":{"id":555}}"#));
}

#[test]
fn capability_doc_tools_return_public_and_internal_views() {
    let dir = tempfile::tempdir().unwrap();
    let mut core = crate::open_at_log_path(dir.path().join("log.bin")).unwrap();

    let list = handle_json_rpc(
        &mut core,
        r#"{"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"capabilities_list","arguments":{}}}"#,
    )
    .unwrap();
    assert!(list.contains("relational_db"), "capabilities_list: {list}");
    assert!(list.contains("document"), "capabilities_list: {list}");

    let public = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"#,
            r#""name":"capability_info","arguments":{"namespace":"relational_db","#,
            r#""format":"json"}}}"#
        ),
    )
    .unwrap();
    assert!(
        public.contains("table_spec.schema.json"),
        "public: {public}"
    );
    assert!(!public.contains("Reserved kv layout"), "public: {public}");

    let internal = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"#,
            r#""name":"capability_info","arguments":{"namespace":"relational_db","#,
            r#""format":"json","includeInternal":true}}}"#
        ),
    )
    .unwrap();
    assert!(
        internal.contains("Reserved kv layout"),
        "internal: {internal}"
    );

    let document = handle_json_rpc(
        &mut core,
        concat!(
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"#,
            r#""name":"capability_info","arguments":{"namespace":"document","#,
            r#""format":"json"}}}"#
        ),
    )
    .unwrap();
    assert!(
        document.contains("document.schema.json"),
        "document: {document}"
    );
}
