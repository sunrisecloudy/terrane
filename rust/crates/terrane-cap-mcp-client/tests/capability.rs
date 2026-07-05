use terrane_cap_mcp_client::{
    call_key_for, prepare_call, prepare_transport, MAX_ARGS_BYTES, REDACTED,
};

#[test]
fn transport_validation_preserves_secret_markers_and_redacts_plain_sensitive_values() {
    let transport = prepare_transport(
        r#"{
            "http":{
                "url":"https://mcp.example.test/mcp",
                "headers":{
                    "authorization":{"$secret":"linear.header"},
                    "x-api-key":"literal-secret",
                    "x-trace":"ok"
                }
            }
        }"#,
    )
    .unwrap();

    assert!(transport.contains(r#""$secret":"linear.header""#), "{transport}");
    assert!(!transport.contains("literal-secret"), "{transport}");
    assert!(transport.contains(REDACTED), "{transport}");
    assert!(transport.contains("x-trace"), "{transport}");
}

#[test]
fn call_preparation_canonicalizes_keys_redacts_sensitive_args_and_hashes_unredacted_args() {
    let prepared = prepare_call(
        "linear",
        "issue_search",
        r#"{"b":2,"token":"secret","a":1,"sensitiveArgs":["/token"],"timeoutMs":1200}"#,
    )
    .unwrap();
    let same = prepare_call(
        "linear",
        "issue_search",
        r#"{"timeoutMs":1200,"sensitiveArgs":["/token"],"a":1,"token":"secret","b":2}"#,
    )
    .unwrap();

    assert_eq!(prepared.args_json, r#"{"a":1,"b":2,"token":"secret"}"#);
    assert_eq!(
        prepared.args_json_redacted,
        format!(r#"{{"a":1,"b":2,"token":"{REDACTED}"}}"#)
    );
    assert_eq!(prepared.call_key, same.call_key);
    assert_eq!(
        prepared.call_key,
        call_key_for("linear", "issue_search", &prepared.args_json).unwrap()
    );
    assert_eq!(prepared.timeout_ms, 1200);
}

#[test]
fn call_preparation_rejects_bad_shapes_and_limits() {
    assert!(prepare_call("bad name", "tool", "{}").is_err());
    assert!(prepare_call("linear", "", "{}").is_err());
    assert!(prepare_call("linear", "tool", "[]").is_err());
    assert!(prepare_call("linear", "tool", r#"{"sensitiveArgs":["token"]}"#).is_err());
    assert!(prepare_call("linear", "tool", r#"{"timeoutMs":300001}"#).is_err());
    let too_large = format!(r#"{{"x":"{}"}}"#, "x".repeat(MAX_ARGS_BYTES));
    assert!(prepare_call("linear", "tool", &too_large).is_err());
}
