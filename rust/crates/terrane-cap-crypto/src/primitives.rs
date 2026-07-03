//! Low-level vault primitives: a master-password KDF, an AEAD seal/open, and a
//! password verifier. Nothing here touches State or the event log — these run on
//! the read path only, so their (randomised) outputs never enter replay.

use argon2::{Algorithm, Argon2, Params, Version};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use chacha20poly1305::aead::Aead;
use chacha20poly1305::{KeyInit, XChaCha20Poly1305, XNonce};
use hmac::{Hmac, Mac};
use sha1::{Digest as _, Sha1};
use sha2::Sha256;
use zeroize::Zeroizing;

pub const KEY_LEN: usize = 32;
pub const SALT_LEN: usize = 16;
pub const XNONCE_LEN: usize = 24;
/// Sealed-blob layout version, stored as the first byte so the format can change
/// without silently misreading old ciphertext.
pub const BLOB_VERSION: u8 = 1;

const VERIFIER_DOMAIN: &[u8] = b"terrane.vault.verifier.v1";

/// A derived vault key. `Zeroizing` wipes it from memory on drop.
pub type VaultKey = Zeroizing<[u8; KEY_LEN]>;

/// Argon2id cost parameters. Stored in the vault meta so a vault stays openable
/// even if the defaults later change.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KdfParams {
    /// Memory cost in KiB.
    pub m_cost: u32,
    /// Iterations (time cost).
    pub t_cost: u32,
    /// Degree of parallelism.
    pub p_cost: u32,
}

impl KdfParams {
    /// OWASP-recommended Argon2id floor (19 MiB, 2 passes, 1 lane): resists
    /// offline cracking while keeping interactive unlock under ~100 ms.
    pub const RECOMMENDED: KdfParams = KdfParams {
        m_cost: 19_456,
        t_cost: 2,
        p_cost: 1,
    };
}

/// Anything that can go wrong on the crypto read path. Kept coarse on purpose so
/// error strings never echo secret material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CryptoError {
    Kdf,
    Encrypt,
    Decrypt,
    BadInput(&'static str),
    Random,
}

impl core::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            CryptoError::Kdf => write!(f, "key derivation failed"),
            CryptoError::Encrypt => write!(f, "encryption failed"),
            CryptoError::Decrypt => write!(f, "decryption failed"),
            CryptoError::BadInput(what) => write!(f, "invalid {what}"),
            CryptoError::Random => write!(f, "randomness unavailable"),
        }
    }
}

/// Fill a buffer with cryptographically secure random bytes.
pub fn random_bytes(out: &mut [u8]) -> Result<(), CryptoError> {
    getrandom::fill(out).map_err(|_| CryptoError::Random)
}

/// Generate a fresh random salt for a new vault.
pub fn new_salt() -> Result<[u8; SALT_LEN], CryptoError> {
    let mut salt = [0u8; SALT_LEN];
    random_bytes(&mut salt)?;
    Ok(salt)
}

/// Derive the 32-byte vault key from the master password and salt with Argon2id.
pub fn derive_key(master: &str, salt: &[u8], params: KdfParams) -> Result<VaultKey, CryptoError> {
    let argon_params = Params::new(
        params.m_cost,
        params.t_cost,
        params.p_cost,
        Some(KEY_LEN),
    )
    .map_err(|_| CryptoError::Kdf)?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, argon_params);
    let mut key: VaultKey = Zeroizing::new([0u8; KEY_LEN]);
    argon
        .hash_password_into(master.as_bytes(), salt, key.as_mut_slice())
        .map_err(|_| CryptoError::Kdf)?;
    Ok(key)
}

/// A one-way tag over the key. Stored in the vault meta so `unlock` can confirm a
/// master password without keeping the key or any plaintext around. HMAC keeps it
/// non-invertible, so the stored tag reveals nothing about the key.
pub fn verifier(key: &[u8; KEY_LEN]) -> [u8; 32] {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
        .expect("HMAC accepts any key length");
    mac.update(VERIFIER_DOMAIN);
    mac.finalize().into_bytes().into()
}

/// Constant-time check that `key` reproduces the stored verifier tag.
pub fn verify_key(key: &[u8; KEY_LEN], expected: &[u8]) -> bool {
    let mut mac = <Hmac<Sha256> as Mac>::new_from_slice(key)
        .expect("HMAC accepts any key length");
    mac.update(VERIFIER_DOMAIN);
    mac.verify_slice(expected).is_ok()
}

/// Encrypt plaintext under the vault key. The returned blob is
/// `version || nonce(24) || ciphertext+tag`; a fresh random nonce per call means
/// identical plaintexts never produce identical ciphertext.
pub fn seal(key: &[u8; KEY_LEN], plaintext: &[u8]) -> Result<Vec<u8>, CryptoError> {
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| CryptoError::Encrypt)?;
    let mut nonce = [0u8; XNONCE_LEN];
    random_bytes(&mut nonce)?;
    let ct = cipher
        .encrypt(XNonce::from_slice(&nonce), plaintext)
        .map_err(|_| CryptoError::Encrypt)?;
    let mut blob = Vec::with_capacity(1 + XNONCE_LEN + ct.len());
    blob.push(BLOB_VERSION);
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ct);
    Ok(blob)
}

/// Decrypt a blob produced by [`seal`] under the same key. Fails (never panics)
/// on a wrong key, a truncated blob, or an unknown version.
pub fn open(key: &[u8; KEY_LEN], blob: &[u8]) -> Result<Vec<u8>, CryptoError> {
    if blob.len() < 1 + XNONCE_LEN || blob[0] != BLOB_VERSION {
        return Err(CryptoError::Decrypt);
    }
    let cipher = XChaCha20Poly1305::new_from_slice(key).map_err(|_| CryptoError::Decrypt)?;
    let nonce = &blob[1..1 + XNONCE_LEN];
    let ct = &blob[1 + XNONCE_LEN..];
    cipher
        .decrypt(XNonce::from_slice(nonce), ct)
        .map_err(|_| CryptoError::Decrypt)
}

/// Uppercase hex SHA-1 of `text`. Used for the HIBP "Pwned Passwords"
/// k-anonymity check: the caller sends only the first 5 hex chars (the prefix)
/// to the range API and matches the 35-char suffix locally, so the password
/// itself never leaves the device.
pub fn sha1_hex(text: &str) -> String {
    use std::fmt::Write as _;
    let digest = Sha1::digest(text.as_bytes());
    let mut out = String::with_capacity(40);
    for byte in digest {
        let _ = write!(out, "{byte:02X}");
    }
    out
}

/// Base64 (standard alphabet) encode.
pub fn b64(bytes: &[u8]) -> String {
    B64.encode(bytes)
}

/// Base64 (standard alphabet) decode.
pub fn unb64(text: &str) -> Result<Vec<u8>, CryptoError> {
    B64.decode(text.trim()).map_err(|_| CryptoError::BadInput("base64"))
}
