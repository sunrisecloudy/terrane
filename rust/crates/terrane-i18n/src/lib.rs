//! The shared localization (i18n) leaf crate: the single source of truth for the
//! supported-language list and `Accept-Language` negotiation.
//!
//! This crate is deliberately dependency-free so both the deterministic core
//! (via the kv capability's public bucket) and every host transport (CLI, FFI,
//! web) share one implementation. It owns no state and performs no I/O — the
//! negotiation functions are pure and fully unit-testable.

pub mod negotiate;

pub use negotiate::{from_accept_language, from_preferred_list};

/// Every locale code Terrane ships translations for, in canonical spelling.
///
/// The first entry equals [`DEFAULT`]. New entries MUST be added with a
/// matching seed catalog and (if regional) an alias decision in
/// [`negotiate`]. Codes are a BCP-47 subset.
pub const SUPPORTED: &[&str] = &[
    "en",
    "es",
    "zh-Hans",
    "ar",
    "pt-BR",
    "fr",
    "de",
    "ja",
    "id",
    "th-TH",
    "ko",
    "vi",
];

/// The fallback code used when nothing in the request resolves. Always the
/// first entry of [`SUPPORTED`].
pub const DEFAULT: &str = "en";

/// True if `code` matches a supported code, compared case-insensitively.
pub fn is_supported(code: &str) -> bool {
    SUPPORTED.iter().any(|c| c.eq_ignore_ascii_case(code))
}

/// The canonical spelling of `code` if it is supported (case-insensitive), else
/// `None`. Callers should always store/emit the canonical form so casing never
/// diverges across hosts.
pub fn canonical(code: &str) -> Option<&'static str> {
    SUPPORTED
        .iter()
        .copied()
        .find(|c| c.eq_ignore_ascii_case(code))
}
