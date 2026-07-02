//! The shared landing page: one template, host-specific wiring via
//! [`HomePageOptions`]. Web-shaped options fetch the catalog client-side;
//! native-shaped options inline the catalog. Either way the config block must
//! stay script-safe for user-controlled app names.

use terrane_host::{home_page, HomePageOptions};

fn web_options() -> HomePageOptions<'static> {
    HomePageOptions {
        app_href_template: "/apps/{id}/",
        catalog_url: Some("/apps"),
        catalog_json: None,
        admin_href: Some("/__terrane/admin"),
        catalog_poll_ms: None,
    }
}

#[test]
fn web_shaped_options_render_fetch_config_and_admin_link() {
    let html = home_page(&web_options());

    assert!(html.contains("<h1>Terrane</h1>"), "brand missing: {html}");
    assert!(
        html.contains(r#"id="home-app-list""#),
        "app list mount missing: {html}"
    );
    assert!(
        html.contains(r#"<script type="application/json" id="home-config">"#),
        "config block missing: {html}"
    );
    assert!(
        html.contains(r#""appHref":"/apps/{id}/""#),
        "app href template missing: {html}"
    );
    assert!(
        html.contains(r#""catalogUrl":"/apps""#),
        "catalog url missing: {html}"
    );
    assert!(
        html.contains(r#""adminHref":"/__terrane/admin""#),
        "admin href missing: {html}"
    );
    assert!(
        !html.contains(r#""catalog":"#),
        "web options must not inline a catalog: {html}"
    );
    // The client script is inlined and wires the config to the page.
    assert!(
        html.contains("getElementById(\"home-config\")"),
        "config reader missing: {html}"
    );
    assert!(
        html.contains("window.terraneAppIcon"),
        "shared app icons missing: {html}"
    );
    assert!(
        html.contains("fetch(String(config.catalogUrl)"),
        "catalog fetch missing: {html}"
    );
    assert!(
        html.contains("id=\"home-admin-link\""),
        "admin link element missing: {html}"
    );
}

#[test]
fn catalog_poll_config_is_optional_and_numeric() {
    let mut options = web_options();
    assert!(
        !home_page(&options).contains(r#""catalogPollMs":"#),
        "poll config should be absent by default"
    );
    options.catalog_poll_ms = Some(3000);
    let html = home_page(&options);
    assert!(
        html.contains(r#""catalogPollMs":3000"#),
        "poll config missing: {html}"
    );
    assert!(
        html.contains("setInterval(loadCatalog"),
        "catalog polling loop missing: {html}"
    );
}

#[test]
fn native_shaped_options_inline_catalog_without_admin_link() {
    let catalog = r#"{"apps":[{"id":"todo","name":"Todo","has_ui":true}]}"#;
    let html = home_page(&HomePageOptions {
        app_href_template: "terrane-app://{id}/frame/",
        catalog_url: None,
        catalog_json: Some(catalog),
        admin_href: None,
        catalog_poll_ms: None,
    });

    assert!(
        html.contains(r#""appHref":"terrane-app://{id}/frame/""#),
        "app href template missing: {html}"
    );
    assert!(
        html.contains(r#""catalog":"{\"apps\":[{\"id\":\"todo\",\"name\":\"Todo\",\"has_ui\":true}]}""#),
        "inline catalog missing: {html}"
    );
    assert!(
        !html.contains(r#""catalogUrl":"#) && !html.contains(r#""adminHref":"#),
        "native options must not add web wiring: {html}"
    );
}

#[test]
fn config_escapes_script_closers_in_user_controlled_names() {
    let catalog = r#"{"apps":[{"id":"x","name":"</script><script>alert(1)</script>","has_ui":true}]}"#;
    let html = home_page(&HomePageOptions {
        app_href_template: "terrane-app://{id}/frame/",
        catalog_url: None,
        catalog_json: Some(catalog),
        admin_href: None,
        catalog_poll_ms: None,
    });

    let config_block = html
        .split(r#"<script type="application/json" id="home-config">"#)
        .nth(1)
        .and_then(|rest| rest.split("</script>").next())
        .expect("config block present");
    assert!(
        !config_block.contains('<'),
        "config must escape every '<': {config_block}"
    );
    assert!(
        config_block.contains(r"\u003c/script>"),
        "hostile name should be unicode-escaped: {config_block}"
    );
}
