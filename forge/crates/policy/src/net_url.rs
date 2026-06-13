//! A minimal, dependency-free URL splitter + private-IP literal classifier.
//!
//! Deliberately *not* the `url` crate: forge-policy stays wasm-clean with no
//! extra deps, and `NetPolicy` only needs `scheme` / `host` / `path`. This is a
//! conservative parser — anything it can't confidently split is an error and the
//! caller fails the request closed.
//!
//! The private-IP classifier is a **literal-text** check (SC-5
//! `denyPrivateNetwork`). It recognizes:
//!   - the `localhost` hostname and any `*.localhost`;
//!   - IPv4 loopback `127.0.0.0/8`, RFC1918 (`10/8`, `172.16/12`, `192.168/16`),
//!     link-local `169.254/16` (incl. the `169.254.169.254` cloud-metadata
//!     address), carrier-grade NAT `100.64/10`, "this host" `0.0.0.0/8`;
//!   - IPv6 loopback `::1`, unspecified `::`, unique-local `fc00::/7`,
//!     link-local `fe80::/10`, and IPv4-mapped forms of the above.
//!
//! True DNS resolution (hostname → private answer) is a runtime concern; this
//! catches only what is decidable from the literal host text.

use std::net::{Ipv4Addr, Ipv6Addr};

/// A coarsely-split URL: just the parts `NetPolicy` matches on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedUrl {
    /// Lowercased scheme, e.g. `https`.
    pub scheme: String,
    /// Host with brackets stripped from IPv6 and the port removed, lowercased,
    /// e.g. `api.example.com`, `127.0.0.1`, `::1`.
    pub host: String,
    /// Path beginning with `/` (query/fragment stripped). `/` if absent.
    pub path: String,
}

impl ParsedUrl {
    /// Split `scheme://host[:port]/path?query#frag` into the parts we match on.
    /// Returns a short error string (not a `CoreError`) the caller maps.
    pub fn parse(url: &str) -> Result<ParsedUrl, String> {
        let (scheme, rest) = url
            .split_once("://")
            .ok_or_else(|| "missing scheme://".to_string())?;
        if scheme.is_empty() {
            return Err("empty scheme".into());
        }
        // The authority runs up to the first '/', '?' or '#'.
        let authority_end = rest
            .find(['/', '?', '#'])
            .unwrap_or(rest.len());
        let authority = &rest[..authority_end];
        let after_authority = &rest[authority_end..];
        if authority.is_empty() {
            return Err("missing host".into());
        }
        // Strip userinfo if present (`user:pass@host`) — defensively rejected for
        // network grants, but split here so host parsing is correct either way.
        let authority = authority.rsplit_once('@').map(|(_, h)| h).unwrap_or(authority);

        let host = parse_host(authority)?;
        if host.is_empty() {
            return Err("empty host".into());
        }

        // Path = everything from the first '/' up to '?' or '#'.
        let path_part = after_authority
            .split(['?', '#'])
            .next()
            .unwrap_or("");
        let path = if path_part.is_empty() {
            "/".to_string()
        } else {
            path_part.to_string()
        };

        Ok(ParsedUrl { scheme: scheme.to_ascii_lowercase(), host, path })
    }
}

/// Extract the bare host from an authority (`host`, `host:port`, `[v6]`,
/// `[v6]:port`), lowercased and with IPv6 brackets removed.
fn parse_host(authority: &str) -> Result<String, String> {
    if let Some(rest) = authority.strip_prefix('[') {
        // Bracketed IPv6: `[::1]` or `[::1]:8080`.
        let close = rest.find(']').ok_or_else(|| "unterminated [ipv6]".to_string())?;
        let host = &rest[..close];
        if host.is_empty() {
            return Err("empty [ipv6] host".into());
        }
        return Ok(host.to_ascii_lowercase());
    }
    // Non-bracketed: strip a trailing `:port` (only one ':' for IPv4/hostnames;
    // an unbracketed multi-colon authority is a malformed IPv6 we reject).
    match authority.split_once(':') {
        Some((host, port)) => {
            if host.contains(':') || port.contains(':') {
                return Err("ambiguous unbracketed colons in authority".into());
            }
            Ok(host.to_ascii_lowercase())
        }
        None => Ok(authority.to_ascii_lowercase()),
    }
}

/// Whether a literal host string is a private/loopback/link-local/metadata
/// target that SC-5 denies by default. Operates on the literal text only.
pub fn host_is_private_literal(host: &str) -> bool {
    let h = host.trim().to_ascii_lowercase();
    let h = h.trim_start_matches('[').trim_end_matches(']');

    // The `localhost` hostname (and any subdomain of it) is always private.
    if h == "localhost" || h.ends_with(".localhost") {
        return true;
    }

    // IPv4 literal?
    if let Ok(v4) = h.parse::<Ipv4Addr>() {
        return ipv4_is_private(v4);
    }
    // IPv6 literal (brackets already stripped)?
    if let Ok(v6) = h.parse::<Ipv6Addr>() {
        return ipv6_is_private(v6);
    }
    false
}

/// SC-5 private IPv4 ranges, including link-local (and the cloud-metadata
/// address inside it), CGNAT, and "this host".
fn ipv4_is_private(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    ip.is_loopback()        // 127.0.0.0/8
        || ip.is_private()  // 10/8, 172.16/12, 192.168/16
        || ip.is_link_local() // 169.254.0.0/16 (incl. 169.254.169.254 metadata)
        || ip.is_broadcast()  // 255.255.255.255
        || ip.is_unspecified() // 0.0.0.0
        || o[0] == 0          // 0.0.0.0/8 "this host"
        || (o[0] == 100 && (64..=127).contains(&o[1])) // 100.64.0.0/10 CGNAT
}

/// SC-5 private IPv6 ranges: loopback, unspecified, unique-local (`fc00::/7`),
/// link-local (`fe80::/10`), plus IPv4-mapped forms of private IPv4.
fn ipv6_is_private(ip: Ipv6Addr) -> bool {
    if ip.is_loopback() || ip.is_unspecified() {
        return true;
    }
    let seg0 = ip.segments()[0];
    // fc00::/7 unique-local.
    if (seg0 & 0xfe00) == 0xfc00 {
        return true;
    }
    // fe80::/10 link-local.
    if (seg0 & 0xffc0) == 0xfe80 {
        return true;
    }
    // ::ffff:a.b.c.d IPv4-mapped — classify by the embedded IPv4.
    if let Some(v4) = ip.to_ipv4_mapped() {
        return ipv4_is_private(v4);
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_https_host_path() {
        let u = ParsedUrl::parse("https://api.example.com/public/weather").unwrap();
        assert_eq!(u.scheme, "https");
        assert_eq!(u.host, "api.example.com");
        assert_eq!(u.path, "/public/weather");
    }

    #[test]
    fn parses_host_with_port() {
        let u = ParsedUrl::parse("http://127.0.0.1:8080/status").unwrap();
        assert_eq!(u.host, "127.0.0.1");
        assert_eq!(u.path, "/status");
    }

    #[test]
    fn parses_bracketed_ipv6() {
        let u = ParsedUrl::parse("http://[::1]/status").unwrap();
        assert_eq!(u.host, "::1");
        assert_eq!(u.path, "/status");
    }

    #[test]
    fn strips_query_and_fragment_from_path() {
        let u = ParsedUrl::parse("https://h.example.com/p?x=1#frag").unwrap();
        assert_eq!(u.path, "/p");
    }

    #[test]
    fn missing_scheme_errors() {
        assert!(ParsedUrl::parse("api.example.com/x").is_err());
    }

    #[test]
    fn empty_path_defaults_to_slash() {
        let u = ParsedUrl::parse("https://h.example.com").unwrap();
        assert_eq!(u.path, "/");
    }

    #[test]
    fn localhost_and_loopback_are_private() {
        assert!(host_is_private_literal("localhost"));
        assert!(host_is_private_literal("api.localhost"));
        assert!(host_is_private_literal("127.0.0.1"));
        assert!(host_is_private_literal("::1"));
    }

    #[test]
    fn rfc1918_link_local_cgnat_and_metadata_are_private() {
        assert!(host_is_private_literal("10.1.2.3"));
        assert!(host_is_private_literal("172.16.0.1"));
        assert!(host_is_private_literal("192.168.1.1"));
        assert!(host_is_private_literal("169.254.169.254")); // cloud metadata
        assert!(host_is_private_literal("100.64.0.1")); // CGNAT
        assert!(host_is_private_literal("0.0.0.0"));
    }

    #[test]
    fn ipv6_ula_and_link_local_are_private() {
        assert!(host_is_private_literal("fc00::1"));
        assert!(host_is_private_literal("fe80::1"));
        assert!(host_is_private_literal("::ffff:10.0.0.1")); // v4-mapped private
    }

    #[test]
    fn public_hosts_are_not_private() {
        assert!(!host_is_private_literal("api.example.com"));
        assert!(!host_is_private_literal("203.0.113.20"));
        assert!(!host_is_private_literal("2001:db8::1"));
    }
}
