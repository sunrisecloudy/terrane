use super::*;

#[test]
fn grant_keys_escape_subjects_and_resource_ids() {
    let subject = "agent:user:local-owner:codex";
    let resource_id = "prefix:settings/theme";
    let key = grant_key("local", subject, "crm/app", resource_id);

    assert_eq!(
        key,
        "orgs/local/subjects/agent%3Auser%3Alocal-owner%3Acodex/apps/crm%2Fapp/resources/prefix%3Asettings%2Ftheme"
    );
    let segments: Vec<_> = key.split('/').collect();
    assert_eq!(segments.len(), 8);
    assert_eq!(decode_segment(segments[3]).unwrap(), subject);
    assert_eq!(decode_segment(segments[5]).unwrap(), "crm/app");
    assert_eq!(decode_segment(segments[7]).unwrap(), resource_id);
}

#[test]
fn namespace_v1_resource_id_is_just_namespace() {
    assert_eq!(namespace_resource_id("kv"), "kv");
    assert_eq!(
        grant_key(
            "local",
            "user:local-owner",
            "demo",
            &namespace_resource_id("kv")
        ),
        "orgs/local/subjects/user%3Alocal-owner/apps/demo/resources/kv"
    );
}

#[test]
fn selector_json_escapes_string_values() {
    assert_eq!(json_string(r#"kv"\demo"#), r#"kv\"\\demo"#);
}
