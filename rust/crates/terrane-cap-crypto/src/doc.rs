use terrane_cap_interface::{
    param, resource_method, CapabilityDoc, CapabilityManifestDoc, CommandDoc, EventDoc, ExampleDoc,
    InternalNote, LimitDoc, ParamDoc, QueryDoc, ResourceDoc, ResourceMethodDoc, SchemaDoc,
};

/// Like [`resource_method`], but records the return shape the docs require.
fn method(
    name: &str,
    params: &[ParamDoc],
    returns: &str,
    summary: &str,
) -> ResourceMethodDoc {
    let mut doc = resource_method(name, "read", params, summary);
    doc.returns = returns.to_string();
    doc
}

pub fn crypto_doc(include_internal: bool) -> CapabilityDoc {
    CapabilityDoc {
        namespace: "crypto".to_string(),
        title: "Vault Crypto".to_string(),
        summary: "Client-side vault encryption for app backends: an Argon2id master-password KDF, \
                  XChaCha20-Poly1305 seal/open, password/passphrase generators, and TOTP. Keys live \
                  only in a process-local session keyring; nothing here is ever recorded or replayed."
            .to_string(),
        status: "experimental".to_string(),
        version: "0.1.0".to_string(),
        audience: vec![
            "app-author".to_string(),
            "agent".to_string(),
            "host-implementer".to_string(),
        ],
        manifest: CapabilityManifestDoc {
            commands: Vec::new(),
            queries: Vec::new(),
            events: Vec::new(),
            subscriptions: Vec::new(),
            resource_methods: resource_method_docs(),
        },
        commands: Vec::<CommandDoc>::new(),
        queries: Vec::<QueryDoc>::new(),
        events: Vec::<EventDoc>::new(),
        resources: vec![ResourceDoc {
            namespace: "crypto".to_string(),
            summary: "Encrypt and decrypt vault items against a master-password-derived key held \
                      in a session keyring, plus password tooling."
                .to_string(),
            methods: resource_method_docs(),
        }],
        schemas: Vec::<SchemaDoc>::new(),
        examples: vec![ExampleDoc {
            title: "Create a vault and seal a secret".to_string(),
            summary: "Derive a key from the master password, keep only ciphertext, store it in kv."
                .to_string(),
            language: "javascript".to_string(),
            code: "var v = JSON.parse(ctx.resource.crypto.newVault(master));\n\
                   ctx.resource.kv.set('meta', v.meta);\n\
                   var s = JSON.parse(ctx.resource.crypto.seal(v.session, JSON.stringify(item)));\n\
                   ctx.resource.kv.set('item:' + id, s.blob);"
                .to_string(),
            expected: "kv holds only the base64 ciphertext blob; the plaintext never reaches an event."
                .to_string(),
        }],
        constraints: vec![
            "Every method is a read: it records no events and is never replayed, so plaintext and \
             derived keys never enter the event log."
                .to_string(),
            "Derived keys live only in a process-local, in-RAM session keyring; a session is bound \
             to the app that unlocked it and idles out after 15 minutes."
                .to_string(),
            "newVault and unlock run Argon2id, which is deliberately slow; call them once per unlock, \
             not per item."
                .to_string(),
            "Expected outcomes (bad_password, locked, decrypt_failed) return { ok: false, reason } \
             rather than throwing, because a throwing read aborts the backend run."
                .to_string(),
            "The CLI runs one process per command, so the keyring starts empty each command; a CLI \
             flow must unlock and use the vault within a single backend run."
                .to_string(),
        ],
        limits: vec![
            LimitDoc {
                name: "kdf".to_string(),
                value: "argon2id m=19456,t=2,p=1".to_string(),
                reason: "OWASP-recommended interactive floor for master-password derivation."
                    .to_string(),
            },
            LimitDoc {
                name: "sessionTtl".to_string(),
                value: "15 minutes idle".to_string(),
                reason: "Unlocked keys auto-lock so a forgotten session does not stay open forever."
                    .to_string(),
            },
        ],
        compatibility: vec![
            "Sealed blobs carry a version byte, so the AEAD format can change without misreading old \
             ciphertext."
                .to_string(),
        ],
        internal: if include_internal {
            vec![InternalNote {
                title: "No replay surface".to_string(),
                body: "The capability owns no commands or events. Its only state is a static session \
                       keyring outside State, so replay of the kv ciphertext events reproduces the \
                       vault without any key material."
                    .to_string(),
            }]
        } else {
            Vec::new()
        },
    }
}

fn resource_method_docs() -> Vec<ResourceMethodDoc> {
    vec![
        method(
            "newVault",
            &[param(
                "masterPassword",
                "Master password to derive the new vault key from.",
                "string",
            )],
            "{ ok:true, meta, session } JSON",
            "Create a vault: return persistable meta and an unlocked session id.",
        ),
        method(
            "unlock",
            &[
                param("masterPassword", "Master password to verify.", "string"),
                param("meta", "Vault meta previously returned by newVault.", "string"),
            ],
            "{ ok:true, session } or { ok:false, reason:bad_password } JSON",
            "Verify the master password and open a session.",
        ),
        method(
            "lock",
            &[param("session", "Session id to forget.", "string")],
            "{ ok:true } JSON",
            "Lock a session, wiping its key from the keyring.",
        ),
        method(
            "status",
            &[param("session", "Session id to check.", "string")],
            "{ ok:true, unlocked } JSON",
            "Report whether a session is currently unlocked.",
        ),
        method(
            "seal",
            &[
                param("session", "Unlocked session id.", "string"),
                param("plaintext", "Item plaintext to encrypt.", "string"),
            ],
            "{ ok:true, blob } or { ok:false, reason:locked } JSON",
            "Encrypt plaintext under the session key into a base64 ciphertext blob.",
        ),
        method(
            "open",
            &[
                param("session", "Unlocked session id.", "string"),
                param("blob", "Base64 ciphertext blob from seal.", "string"),
            ],
            "{ ok:true, plaintext } or { ok:false, reason } JSON",
            "Decrypt a blob under the session key back to plaintext.",
        ),
        method(
            "generatePassword",
            &[param(
                "optionsJson",
                "JSON: length, lowercase, uppercase, digits, symbols, avoid_ambiguous.",
                "string",
            )],
            "{ ok:true, password } JSON",
            "Generate a random password from the selected character classes.",
        ),
        method(
            "generatePassphrase",
            &[param(
                "optionsJson",
                "JSON: words, separator, capitalize, include_number.",
                "string",
            )],
            "{ ok:true, passphrase } JSON",
            "Generate a diceware passphrase from the EFF large wordlist.",
        ),
        method(
            "strength",
            &[param("password", "Password to score.", "string")],
            "{ ok:true, score, guessesLog10 } JSON",
            "Return a coarse 0-4 strength score and an approximate guesses log10.",
        ),
        method(
            "totp",
            &[param(
                "paramsJson",
                "JSON: secret (base32), digits, period, algorithm, timestamp.",
                "string",
            )],
            "{ ok:true, code, period, remaining } or { ok:false, reason } JSON",
            "Compute the current TOTP code for a stored 2FA secret.",
        ),
    ]
}
