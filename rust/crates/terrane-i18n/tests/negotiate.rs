//! Integration tests for `Accept-Language` negotiation. These exercise the
//! public `from_accept_language` / `from_preferred_list` entry points against
//! the cases enumerated in the i18n plan §3.4.

use terrane_i18n::{from_accept_language, from_preferred_list, DEFAULT};

#[test]
fn empty_header_returns_default() {
    assert_eq!(from_accept_language(""), DEFAULT);
}

#[test]
fn whitespace_header_returns_default() {
    assert_eq!(from_accept_language("   "), DEFAULT);
}

#[test]
fn exact_match_with_uppercase_input() {
    assert_eq!(from_accept_language("ES"), "es");
}

#[test]
fn en_us_with_q_falls_to_en() {
    assert_eq!(from_accept_language("en-US,en;q=0.9"), "en");
}

#[test]
fn fr_ch_beats_en_at_lower_q() {
    assert_eq!(from_accept_language("fr-CH, fr;q=0.9, en;q=0.8"), "fr");
}

#[test]
fn de_at_falls_to_de() {
    assert_eq!(from_accept_language("de-AT"), "de");
}

#[test]
fn bare_pt_maps_to_pt_br() {
    assert_eq!(from_accept_language("pt"), "pt-BR");
}

#[test]
fn pt_pt_maps_to_pt_br() {
    assert_eq!(from_accept_language("pt-PT"), "pt-BR");
}

#[test]
fn pt_br_matches_directly() {
    assert_eq!(from_accept_language("pt-BR"), "pt-BR");
}

#[test]
fn bare_zh_maps_to_zh_hans() {
    assert_eq!(from_accept_language("zh"), "zh-Hans");
}

#[test]
fn zh_cn_maps_to_zh_hans() {
    assert_eq!(from_accept_language("zh-CN"), "zh-Hans");
}

#[test]
fn zh_hant_is_unsupported_and_falls_back() {
    assert_eq!(from_accept_language("zh-Hant"), DEFAULT);
}

#[test]
fn zh_tw_then_bare_zh_resolves_to_hans() {
    assert_eq!(from_accept_language("zh-TW,zh;q=0.9"), "zh-Hans");
}

#[test]
fn bare_th_maps_to_th_th() {
    assert_eq!(from_accept_language("th"), "th-TH");
}

#[test]
fn th_th_matches_directly() {
    assert_eq!(from_accept_language("th-TH"), "th-TH");
}

#[test]
fn wildcard_is_ignored_to_default() {
    assert_eq!(from_accept_language("*"), DEFAULT);
}

#[test]
fn q_zero_drops_en_letting_fr_win() {
    assert_eq!(from_accept_language("en;q=0, fr"), "fr");
}

#[test]
fn equal_q_preserves_header_order() {
    assert_eq!(from_accept_language("fr;q=0.8, de;q=0.8"), "fr");
}

#[test]
fn malformed_q_de_prioritizes_that_tag() {
    assert_eq!(from_accept_language("fr;q=abc, de"), "de");
}

#[test]
fn unknown_primary_falls_through_to_ja() {
    assert_eq!(from_accept_language("xx-YY, ja;q=0.5"), "ja");
}

#[test]
fn macos_preferred_list_fr_then_en() {
    assert_eq!(from_preferred_list(&["fr-CA", "en-US"]), "fr");
}

#[test]
fn macos_preferred_list_traditional_then_simplified() {
    assert_eq!(
        from_preferred_list(&["zh-Hant-TW", "zh-Hans"]),
        "zh-Hans"
    );
}

#[test]
fn empty_preferred_list_returns_default() {
    assert_eq!(from_preferred_list(&[]), DEFAULT);
}
