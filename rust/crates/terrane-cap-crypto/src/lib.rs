//! The `crypto` capability — client-side vault encryption for app backends.
//!
//! It exists because Terrane's event log and kv projection are plaintext: a
//! secret written through `kv.set` lands verbatim on disk. This capability lets
//! an app keep only ciphertext in kv. It derives a key from a master password
//! (Argon2id), seals/opens items (XChaCha20-Poly1305), and holds the derived key
//! in a process-local session keyring that is never part of State.
//!
//! Every method is a resource **read**: it records no events and is never
//! replayed. That is the whole trick — plaintext and keys pass through the read
//! path, while only the resulting ciphertext ever becomes a (kv) event. Replaying
//! the log rebuilds the vault from ciphertext without any key material.

// nanoserde's DeJson derive on Option fields emits code clippy flags with
// question_mark; it is a false positive on generated code, not our source.
#![allow(clippy::question_mark)]

mod doc;
mod generate;
mod keyring;
mod primitives;
mod resources;
mod totp;
mod vault;

pub use generate::{password, passphrase, strength, PassphraseOptions, PasswordOptions};
pub use primitives::{
    b64, derive_key, new_salt, open, random_id, seal, sha1_hex, unb64, verifier, verify_key,
    CryptoError, KdfParams, VaultKey, BLOB_VERSION, KEY_LEN, SALT_LEN, XNONCE_LEN,
};
pub use totp::{base32_decode, hotp, totp, Algorithm, TotpCode};
pub use vault::{new_vault, unlock, VaultMeta};

use terrane_cap_interface::{
    CapManifest, Capability, CommandCtx, Decision, Error, EventRecord, GrantResourceSpec, ReadValue,
    ResourceReadCtx, Result, StateStore,
};

/// The vault-crypto capability. Stateless struct: its only "state" is the
/// process-global session keyring in [`keyring`].
pub struct CryptoCapability;

impl Capability for CryptoCapability {
    fn namespace(&self) -> &'static str {
        "crypto"
    }

    fn manifest(&self) -> CapManifest {
        CapManifest {
            commands: Vec::new(),
            events: Vec::new(),
            queries: Vec::new(),
            resources: resources::resource_methods(),
            grant_resources: vec![GrantResourceSpec::namespace_v1(
                "crypto",
                &["read"],
                "Client-side vault encryption: master-password KDF, AEAD seal/open, generators, TOTP.",
            )],
            subscriptions: Vec::new(),
        }
    }

    fn doc(&self, include_internal: bool) -> terrane_cap_interface::CapabilityDoc {
        doc::crypto_doc(include_internal)
    }

    fn decide(&self, _ctx: CommandCtx<'_>, name: &str, _args: &[String]) -> Result<Decision> {
        Err(Error::InvalidInput(format!(
            "crypto exposes no commands (got {name}); use ctx.resource.crypto.*"
        )))
    }

    fn fold(&self, _state: &mut dyn StateStore, _record: &EventRecord) -> Result<()> {
        // Owns no events and subscribes to none.
        Ok(())
    }

    fn read_resource(
        &self,
        ctx: ResourceReadCtx<'_>,
        name: &str,
        args: &[String],
    ) -> Result<ReadValue> {
        resources::read(ctx, name, args)
    }
}
