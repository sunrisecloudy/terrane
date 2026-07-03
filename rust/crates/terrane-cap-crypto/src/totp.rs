//! RFC 6238 TOTP (and RFC 4226 HOTP) for the two-factor codes a vault stores
//! alongside a login. Pure computation over a supplied clock; no state.

use hmac::{Hmac, Mac};
use sha1::Sha1;
use sha2::{Sha256, Sha512};

use crate::primitives::CryptoError;

/// Hash used inside the HMAC. SHA1 is the RFC default and what most issuers emit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Algorithm {
    Sha1,
    Sha256,
    Sha512,
}

impl Algorithm {
    pub fn parse(name: &str) -> Algorithm {
        match name.trim().to_ascii_uppercase().as_str() {
            "SHA256" | "SHA-256" => Algorithm::Sha256,
            "SHA512" | "SHA-512" => Algorithm::Sha512,
            _ => Algorithm::Sha1,
        }
    }
}

/// Decode an RFC 4648 base32 secret (case-insensitive, padding and spaces
/// tolerated) into raw key bytes.
pub fn base32_decode(input: &str) -> Result<Vec<u8>, CryptoError> {
    const ALPHABET: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
    let mut buffer: u32 = 0;
    let mut bits: u32 = 0;
    let mut out = Vec::new();
    for ch in input.chars() {
        if ch == '=' || ch.is_whitespace() || ch == '-' {
            continue;
        }
        let up = ch.to_ascii_uppercase() as u8;
        let val = ALPHABET
            .iter()
            .position(|&c| c == up)
            .ok_or(CryptoError::BadInput("base32"))? as u32;
        buffer = (buffer << 5) | val;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buffer >> bits) as u8);
        }
    }
    if out.is_empty() {
        return Err(CryptoError::BadInput("base32"));
    }
    Ok(out)
}

fn hmac_digest(algo: Algorithm, key: &[u8], msg: &[u8]) -> Vec<u8> {
    match algo {
        Algorithm::Sha1 => {
            let mut mac = <Hmac<Sha1> as Mac>::new_from_slice(key).expect("any key length");
            mac.update(msg);
            mac.finalize().into_bytes().to_vec()
        }
        Algorithm::Sha256 => {
            let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key).expect("any key length");
            mac.update(msg);
            mac.finalize().into_bytes().to_vec()
        }
        Algorithm::Sha512 => {
            let mut mac = <Hmac<Sha512> as Mac>::new_from_slice(key).expect("any key length");
            mac.update(msg);
            mac.finalize().into_bytes().to_vec()
        }
    }
}

/// RFC 4226 HOTP: an `digits`-length decimal code for a given counter.
pub fn hotp(secret: &[u8], counter: u64, digits: u32, algo: Algorithm) -> String {
    // Cap at 9: 10^10 overflows u32 (panic under overflow-checks, silent wrap in
    // release). 9 digits is already beyond any real TOTP; RFC 6238 uses 6 or 8.
    let digits = digits.clamp(1, 9);
    let digest = hmac_digest(algo, secret, &counter.to_be_bytes());
    let offset = (digest[digest.len() - 1] & 0x0f) as usize;
    let bin = ((u32::from(digest[offset]) & 0x7f) << 24)
        | (u32::from(digest[offset + 1]) << 16)
        | (u32::from(digest[offset + 2]) << 8)
        | u32::from(digest[offset + 3]);
    let modulo = 10u32.pow(digits);
    let code = bin % modulo;
    format!("{code:0width$}", width = digits as usize)
}

/// A computed TOTP code plus how many seconds remain in its current window.
pub struct TotpCode {
    pub code: String,
    pub period: u64,
    pub remaining: u64,
}

/// RFC 6238 TOTP at wall-clock time `unix_seconds`.
pub fn totp(
    secret_base32: &str,
    unix_seconds: u64,
    period: u64,
    digits: u32,
    algo: Algorithm,
) -> Result<TotpCode, CryptoError> {
    let period = period.max(1);
    let digits = digits.clamp(4, 9);
    let secret = base32_decode(secret_base32)?;
    let counter = unix_seconds / period;
    let code = hotp(&secret, counter, digits, algo);
    let remaining = period - (unix_seconds % period);
    Ok(TotpCode {
        code,
        period,
        remaining,
    })
}
