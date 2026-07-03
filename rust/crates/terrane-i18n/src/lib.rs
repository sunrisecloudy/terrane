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

/// Supported codes that render right-to-left. Only Arabic in the initial set;
/// callers use [`dir_for`]/[`is_rtl`] rather than hard-coding `"ar"`.
pub const RTL: &[&str] = &["ar"];

/// True if `code` is a right-to-left language (case-insensitive).
pub fn is_rtl(code: &str) -> bool {
    RTL.iter().any(|c| c.eq_ignore_ascii_case(code))
}

/// The writing direction for `code`: `"rtl"` for right-to-left languages, else
/// `"ltr"`. Hosts set this on the document / push it to the app frame so a
/// single source of truth drives layout mirroring.
pub fn dir_for(code: &str) -> &'static str {
    if is_rtl(code) {
        "rtl"
    } else {
        "ltr"
    }
}

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
