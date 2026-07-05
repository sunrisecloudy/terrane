use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use sha2::{Digest as _, Sha256};
use terrane_cap_interface::{Error, Result};

pub(crate) fn decode_base64(value: &str) -> Result<Vec<u8>> {
    B64.decode(value.trim())
        .map_err(|_| Error::InvalidInput("blob bytes_base64 must be valid standard base64".into()))
}

pub(crate) fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;

    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
