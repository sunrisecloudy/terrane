//! Pure `Accept-Language` / preference-list negotiation.
//!
//! Given a header or ordered list, pick the best code from
//! [`crate::SUPPORTED`], falling back to [`crate::DEFAULT`] when nothing
//! resolves. No I/O, no allocations beyond the candidate scratch list.

use crate::{canonical, DEFAULT, SUPPORTED};

/// Best supported code for an RFC 7231 `Accept-Language` header.
///
/// Returns [`DEFAULT`] (`"en"`) for an empty header or one where no candidate
/// resolves. q=0 entries drop their tag; malformed q values are treated as
/// q=0. Among equal q values the header order is the tiebreak (stable).
pub fn from_accept_language(header: &str) -> &'static str {
    // (original_index, lowercased_tag, q) — the index makes the sort fully
    // deterministic independent of sort stability.
    let mut candidates: Vec<(usize, String, f32)> = Vec::new();
    for (idx, raw) in header.split(',').enumerate() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        let (tag, q) = parse_element(trimmed);
        if q <= 0.0 {
            continue;
        }
        candidates.push((idx, tag.to_ascii_lowercase(), q));
    }
    // Stable sort by q descending; original order breaks ties. Rust's
    // slice sort is stable, but the index tiebreak keeps it explicit.
    candidates.sort_by(|a, b| {
        b.2.partial_cmp(&a.2)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.0.cmp(&b.0))
    });
    for (_, tag, _) in candidates {
        if let Some(code) = resolve(&tag) {
            return code;
        }
    }
    DEFAULT
}

/// Best supported code for an ordered preference list (e.g. macOS
/// `preferredLanguages`). Earlier entries win; the first that resolves is
/// returned. Falls back to [`DEFAULT`] when none resolve.
pub fn from_preferred_list(prefs: &[&str]) -> &'static str {
    for raw in prefs {
        let tag = raw.trim().to_ascii_lowercase();
        if tag.is_empty() {
            continue;
        }
        if let Some(code) = resolve(&tag) {
            return code;
        }
    }
    DEFAULT
}

/// Split one comma-element `"<tag>;q=<float>;…"` into its tag and q value.
///
/// Missing q defaults to 1.0. Non-numeric or out-of-range q is treated as 0.0
/// (de-prioritizing junk). Numeric q is clamped to `[0.0, 1.0]`.
fn parse_element(s: &str) -> (&str, f32) {
    let mut parts = s.split(';');
    let tag = parts.next().unwrap_or("").trim();
    let mut q = 1.0_f32;
    for param in parts {
        let param = param.trim();
        let lower = param.to_ascii_lowercase();
        if let Some(val) = lower.strip_prefix("q=") {
            match val.trim().parse::<f32>() {
                Ok(v) => q = v.clamp(0.0, 1.0),
                Err(_) => q = 0.0,
            }
        }
    }
    (tag, q)
}

/// Resolve one lowercased tag to a supported code using the ladder:
///   1. exact case-insensitive match against SUPPORTED;
///   2. regional/script primary defaults (`zh`, `pt`, `th`);
///   3. generic primary-subtag match (a region/script-less supported code whose
///      single subtag equals the primary).
///
/// Returns `None` for tags that are deliberately unsupported, e.g. Traditional
/// Chinese (`zh-Hant`, `zh-TW`, `zh-HK`) is NOT folded to `zh-Hans`.
fn resolve(lower_tag: &str) -> Option<&'static str> {
    if lower_tag.is_empty() {
        return None;
    }
    // 1. exact
    if let Some(c) = canonical(lower_tag) {
        return Some(c);
    }
    let primary = lower_tag.split('-').next().unwrap_or(lower_tag);
    // 2. regional/script defaults that need explicit, non-generic handling.
    match primary {
        "zh" => {
            // Simplified Chinese family we support. We deliberately do NOT map
            // Traditional (`zh-Hant`, `zh-TW`, `zh-HK`, `zh-Hant-*`) down to
            // Simplified: they are different scripts and stay unsupported.
            if lower_tag == "zh"
                || lower_tag == "zh-cn"
                || lower_tag == "zh-sg"
                || lower_tag == "zh-hans"
                || lower_tag.starts_with("zh-hans-")
            {
                return Some("zh-Hans");
            }
            return None;
        }
        // Brazilian Portuguese is the only variant shipped; map every `pt-*`.
        "pt" => return Some("pt-BR"),
        // Thai is shipped only as `th-TH`; map every `th-*`.
        "th" => return Some("th-TH"),
        _ => {}
    }
    // 3. generic primary-subtag match: a single-subtag supported code.
    if let Some(c) = canonical(primary) {
        debug_assert!(
            !c.contains('-'),
            "generic primary match must be a region/script-less code"
        );
        return Some(c);
    }
    let _ = SUPPORTED; // anchor the const so future editors see it here too.
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_whitespace_fall_back_to_default() {
        assert_eq!(from_accept_language(""), DEFAULT);
        assert_eq!(from_accept_language("   "), DEFAULT);
        assert_eq!(from_accept_language(",,,,"), DEFAULT);
    }

    #[test]
    fn exact_match_is_case_insensitive() {
        assert_eq!(from_accept_language("ES"), "es");
        assert_eq!(from_accept_language("es"), "es");
        assert_eq!(from_accept_language("PT-BR"), "pt-BR");
        assert_eq!(from_accept_language("zh-hans"), "zh-Hans");
    }

    #[test]
    fn primary_subtag_match_strips_region() {
        assert_eq!(from_accept_language("en-US,en;q=0.9"), "en");
        assert_eq!(from_accept_language("de-AT"), "de");
        assert_eq!(from_accept_language("es-419"), "es");
        assert_eq!(from_accept_language("ja-JP"), "ja");
        assert_eq!(from_accept_language("fr-CH, fr;q=0.9, en;q=0.8"), "fr");
    }

    #[test]
    fn regional_defaults_map_variants() {
        assert_eq!(from_accept_language("pt"), "pt-BR");
        assert_eq!(from_accept_language("pt-PT"), "pt-BR");
        assert_eq!(from_accept_language("pt-BR"), "pt-BR");
        assert_eq!(from_accept_language("th"), "th-TH");
        assert_eq!(from_accept_language("th-TH"), "th-TH");
    }

    #[test]
    fn simplified_chinese_family_maps_to_hans() {
        assert_eq!(from_accept_language("zh"), "zh-Hans");
        assert_eq!(from_accept_language("zh-CN"), "zh-Hans");
        assert_eq!(from_accept_language("zh-SG"), "zh-Hans");
        assert_eq!(from_accept_language("zh-Hans"), "zh-Hans");
    }

    #[test]
    fn traditional_chinese_is_not_folded_to_simplified() {
        assert_eq!(from_accept_language("zh-Hant"), DEFAULT);
        assert_eq!(from_accept_language("zh-TW"), DEFAULT);
        assert_eq!(from_accept_language("zh-HK"), DEFAULT);
        assert_eq!(from_accept_language("zh-Hant-TW"), DEFAULT);
    }

    #[test]
    fn bare_zh_wins_after_traditional_misses() {
        assert_eq!(from_accept_language("zh-TW,zh;q=0.9"), "zh-Hans");
    }

    #[test]
    fn q_zero_drops_tag() {
        assert_eq!(from_accept_language("en;q=0, fr"), "fr");
    }

    #[test]
    fn malformed_q_is_de_prioritized() {
        assert_eq!(from_accept_language("fr;q=abc, de"), "de");
    }

    #[test]
    fn wildcard_is_ignored() {
        assert_eq!(from_accept_language("*"), DEFAULT);
    }

    #[test]
    fn equal_q_uses_header_order_tiebreak() {
        assert_eq!(from_accept_language("fr;q=0.8, de;q=0.8"), "fr");
        assert_eq!(from_accept_language("de;q=0.8, fr;q=0.8"), "de");
    }

    #[test]
    fn unknown_primary_falls_through_to_next_candidate() {
        assert_eq!(from_accept_language("xx-YY, ja;q=0.5"), "ja");
    }

    #[test]
    fn preferred_list_resolves_in_order() {
        assert_eq!(from_preferred_list(&["fr-CA", "en-US"]), "fr");
        assert_eq!(
            from_preferred_list(&["zh-Hant-TW", "zh-Hans"]),
            "zh-Hans"
        );
        assert_eq!(from_preferred_list(&[]), DEFAULT);
        assert_eq!(from_preferred_list(&[""]), DEFAULT);
    }
}
