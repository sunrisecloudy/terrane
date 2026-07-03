//! Crate-surface tests for the vault primitives: KDF/verifier, AEAD round-trip,
//! generators, and RFC 6238 TOTP vectors.

use terrane_cap_crypto::{
    base32_decode, derive_key, new_vault, open, passphrase, password, seal, strength, totp, unlock,
    verifier, verify_key, Algorithm, KdfParams, PassphraseOptions, PasswordOptions, VaultMeta,
};

/// Cheap KDF params so tests that derive a key stay fast.
const FAST: KdfParams = KdfParams {
    m_cost: 1024,
    t_cost: 1,
    p_cost: 1,
};

#[test]
fn seal_open_round_trips_and_rejects_wrong_key() {
    let key = derive_key("correct horse", b"saltsaltsalt1234", FAST).unwrap();
    let blob = seal(&key, b"battery staple").unwrap();
    // Ciphertext must not contain the plaintext.
    assert!(!blob.windows(7).any(|w| w == b"battery"));
    let opened = open(&key, &blob).unwrap();
    assert_eq!(opened, b"battery staple");

    let other = derive_key("wrong horse", b"saltsaltsalt1234", FAST).unwrap();
    assert!(open(&other, &blob).is_err());
}

#[test]
fn seal_is_nondeterministic() {
    let key = derive_key("pw", b"saltsaltsalt1234", FAST).unwrap();
    let a = seal(&key, b"same").unwrap();
    let b = seal(&key, b"same").unwrap();
    assert_ne!(a, b, "fresh nonce per seal must differ");
    assert_eq!(open(&key, &a).unwrap(), open(&key, &b).unwrap());
}

#[test]
fn verifier_matches_only_the_right_key() {
    let key = derive_key("master", b"saltsaltsalt1234", FAST).unwrap();
    let tag = verifier(&key);
    assert!(verify_key(&key, &tag));
    let other = derive_key("master2", b"saltsaltsalt1234", FAST).unwrap();
    assert!(!verify_key(&other, &tag));
}

#[test]
fn new_vault_unlocks_with_correct_password_only() {
    use nanoserde::{DeJson, SerJson};
    let (meta, key) = new_vault("hunter2").unwrap();
    // Meta round-trips as JSON (it is stored as a plaintext kv value).
    let round = VaultMeta::deserialize_json(&meta.serialize_json()).unwrap();
    assert_eq!(round, meta);

    let blob = seal(&key, b"secret note").unwrap();

    let opened = unlock("hunter2", &meta).unwrap().expect("right password");
    assert_eq!(open(&opened, &blob).unwrap(), b"secret note");

    assert!(unlock("wrong", &meta).unwrap().is_none());
}

#[test]
fn password_respects_length_and_classes() {
    let opts = PasswordOptions {
        length: 32,
        lowercase: true,
        uppercase: true,
        digits: true,
        symbols: false,
        avoid_ambiguous: false,
    };
    let pw = password(&opts).unwrap();
    assert_eq!(pw.chars().count(), 32);
    assert!(pw.chars().any(|c| c.is_ascii_lowercase()));
    assert!(pw.chars().any(|c| c.is_ascii_uppercase()));
    assert!(pw.chars().any(|c| c.is_ascii_digit()));
    assert!(pw.chars().all(|c| c.is_ascii_alphanumeric()));
}

#[test]
fn password_avoids_ambiguous_characters() {
    let opts = PasswordOptions {
        length: 200,
        avoid_ambiguous: true,
        ..PasswordOptions::default()
    };
    let pw = password(&opts).unwrap();
    for bad in "O0oIl1|S5B8".chars() {
        assert!(!pw.contains(bad), "ambiguous {bad} leaked");
    }
}

#[test]
fn passphrase_has_requested_shape() {
    let opts = PassphraseOptions {
        words: 6,
        separator: ".".to_string(),
        capitalize: true,
        include_number: false,
    };
    let phrase = passphrase(&opts).unwrap();
    let parts: Vec<&str> = phrase.split('.').collect();
    assert_eq!(parts.len(), 6);
    for word in parts {
        assert!(word.chars().next().unwrap().is_ascii_uppercase());
    }
}

#[test]
fn strength_increases_with_complexity() {
    let (weak, _) = strength("abc");
    let (strong, _) = strength("A9#kQ2!vBz7$Lm4@Rp1&");
    assert!(weak < strong);
    assert_eq!(strength("").0, 0);
}

#[test]
fn totp_matches_rfc6238_vectors() {
    // RFC 6238 Appendix B: ASCII secret "12345678901234567890", SHA1, 8 digits.
    let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
    assert_eq!(base32_decode(secret).unwrap(), b"12345678901234567890");

    let at = |t: u64| totp(secret, t, 30, 8, Algorithm::Sha1).unwrap().code;
    assert_eq!(at(59), "94287082");
    assert_eq!(at(1111111109), "07081804");
    assert_eq!(at(1234567890), "89005924");
}

#[test]
fn totp_reports_remaining_window() {
    let secret = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
    let code = totp(secret, 65, 30, 6, Algorithm::Sha1).unwrap();
    assert_eq!(code.period, 30);
    // 65 mod 30 = 5 → 25 seconds left in the window.
    assert_eq!(code.remaining, 25);
}
