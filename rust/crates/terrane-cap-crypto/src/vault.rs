//! The vault meta record and the master-password unlock check. The meta holds
//! only public material — KDF parameters, the salt, and a one-way verifier — so
//! it is safe to store as an ordinary (plaintext) kv value alongside the
//! ciphertext items.

use nanoserde::{DeJson, SerJson};

use crate::primitives::{
    b64, derive_key, new_salt, unb64, verifier, verify_key, CryptoError, KdfParams, VaultKey,
};

/// Public, non-secret vault descriptor persisted by the app in kv.
#[derive(Debug, Clone, PartialEq, Eq, SerJson, DeJson)]
pub struct VaultMeta {
    /// Meta format version.
    pub v: u32,
    /// KDF identifier (always `argon2id` in v1).
    pub kdf: String,
    /// Argon2id memory cost (KiB).
    pub m: u32,
    /// Argon2id time cost (iterations).
    pub t: u32,
    /// Argon2id parallelism.
    pub p: u32,
    /// Base64 salt.
    pub salt: String,
    /// Base64 one-way key verifier.
    pub verifier: String,
}

impl VaultMeta {
    fn params(&self) -> KdfParams {
        KdfParams {
            m_cost: self.m,
            t_cost: self.t,
            p_cost: self.p,
        }
    }
}

/// Create a brand-new vault: fresh salt + recommended KDF params, derive the key,
/// and return the persistable meta together with the live key. Also used to
/// re-key a vault when changing the master password (the caller re-seals items
/// under the returned key).
pub fn new_vault(master: &str) -> Result<(VaultMeta, VaultKey), CryptoError> {
    let salt = new_salt()?;
    let params = KdfParams::RECOMMENDED;
    let key = derive_key(master, &salt, params)?;
    let ver = verifier(&key);
    let meta = VaultMeta {
        v: 1,
        kdf: "argon2id".to_string(),
        m: params.m_cost,
        t: params.t_cost,
        p: params.p_cost,
        salt: b64(&salt),
        verifier: b64(&ver),
    };
    Ok((meta, key))
}

/// Re-derive the key from a master password and confirm it against the stored
/// verifier. `Ok(None)` means the password was wrong; `Err` means the meta was
/// malformed.
pub fn unlock(master: &str, meta: &VaultMeta) -> Result<Option<VaultKey>, CryptoError> {
    let salt = unb64(&meta.salt)?;
    let expected = unb64(&meta.verifier)?;
    let key = derive_key(master, &salt, meta.params())?;
    if verify_key(&key, &expected) {
        Ok(Some(key))
    } else {
        Ok(None)
    }
}
