//! Password and passphrase generators. All randomness is unbiased (rejection
//! sampling over a CSPRNG); nothing here records state.

use std::sync::LazyLock;

use crate::primitives::{random_bytes, CryptoError};

/// The EFF "large" diceware wordlist (7776 words, CC BY 3.0,
/// <https://www.eff.org/dice>). 12.9 bits of entropy per word.
static WORDS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    include_str!("wordlist.txt")
        .lines()
        .map(str::trim)
        .filter(|w| !w.is_empty())
        .collect()
});

const LOWER: &[u8] = b"abcdefghijklmnopqrstuvwxyz";
const UPPER: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ";
const DIGITS: &[u8] = b"0123456789";
const SYMBOLS: &[u8] = b"!@#$%^&*()-_=+[]{};:,.?";
/// Characters that are easy to confuse in most fonts, removed when the caller
/// asks to avoid ambiguity.
const AMBIGUOUS: &[u8] = b"O0oIl1|S5B8";

/// Draw a uniform integer in `[0, n)` from the CSPRNG without modulo bias.
fn uniform(n: u32) -> Result<u32, CryptoError> {
    if n <= 1 {
        return Ok(0);
    }
    let limit = u32::MAX - (u32::MAX % n);
    loop {
        let mut buf = [0u8; 4];
        random_bytes(&mut buf)?;
        let r = u32::from_le_bytes(buf);
        if r < limit {
            return Ok(r % n);
        }
    }
}

fn pick(pool: &[u8]) -> Result<u8, CryptoError> {
    let idx = uniform(pool.len() as u32)? as usize;
    Ok(pool[idx])
}

/// A Fisher–Yates shuffle so class-guaranteed characters aren't stuck up front.
fn shuffle(items: &mut [u8]) -> Result<(), CryptoError> {
    for i in (1..items.len()).rev() {
        let j = uniform((i + 1) as u32)? as usize;
        items.swap(i, j);
    }
    Ok(())
}

/// Options for [`password`]. Missing fields fall back to strong defaults.
pub struct PasswordOptions {
    pub length: usize,
    pub lowercase: bool,
    pub uppercase: bool,
    pub digits: bool,
    pub symbols: bool,
    pub avoid_ambiguous: bool,
}

impl Default for PasswordOptions {
    fn default() -> Self {
        Self {
            length: 20,
            lowercase: true,
            uppercase: true,
            digits: true,
            symbols: true,
            avoid_ambiguous: false,
        }
    }
}

/// Generate a random password. Guarantees at least one character from every
/// selected class (when the length allows), then fills the rest from the union.
pub fn password(opts: &PasswordOptions) -> Result<String, CryptoError> {
    let length = opts.length.clamp(1, 256);
    let filter = |set: &[u8]| -> Vec<u8> {
        set.iter()
            .copied()
            .filter(|c| !opts.avoid_ambiguous || !AMBIGUOUS.contains(c))
            .collect()
    };

    let mut classes: Vec<Vec<u8>> = Vec::new();
    if opts.lowercase {
        classes.push(filter(LOWER));
    }
    if opts.uppercase {
        classes.push(filter(UPPER));
    }
    if opts.digits {
        classes.push(filter(DIGITS));
    }
    if opts.symbols {
        classes.push(filter(SYMBOLS));
    }
    classes.retain(|c| !c.is_empty());
    if classes.is_empty() {
        // No class selected (or all emptied by the ambiguity filter): fall back to
        // lowercase so we never return an empty/all-nothing password.
        classes.push(filter(LOWER));
        if classes[0].is_empty() {
            classes[0] = LOWER.to_vec();
        }
    }

    let union: Vec<u8> = classes.iter().flatten().copied().collect();
    let mut out: Vec<u8> = Vec::with_capacity(length);

    // One guaranteed character from each class, capped at the requested length.
    for class in classes.iter().take(length) {
        out.push(pick(class)?);
    }
    while out.len() < length {
        out.push(pick(&union)?);
    }
    shuffle(&mut out)?;

    // Every byte comes from ASCII class tables, so this is always valid UTF-8.
    Ok(String::from_utf8(out).unwrap_or_default())
}

/// Options for [`passphrase`].
pub struct PassphraseOptions {
    pub words: usize,
    pub separator: String,
    pub capitalize: bool,
    pub include_number: bool,
}

impl Default for PassphraseOptions {
    fn default() -> Self {
        Self {
            words: 5,
            separator: "-".to_string(),
            capitalize: false,
            include_number: false,
        }
    }
}

/// Generate a diceware passphrase from the EFF large wordlist.
pub fn passphrase(opts: &PassphraseOptions) -> Result<String, CryptoError> {
    let count = opts.words.clamp(1, 64);
    let words = &*WORDS;
    if words.is_empty() {
        return Err(CryptoError::BadInput("wordlist"));
    }

    let mut chosen: Vec<String> = Vec::with_capacity(count);
    for _ in 0..count {
        let idx = uniform(words.len() as u32)? as usize;
        let mut w = words[idx].to_string();
        if opts.capitalize {
            let mut chars = w.chars();
            if let Some(first) = chars.next() {
                w = first.to_uppercase().collect::<String>() + chars.as_str();
            }
        }
        chosen.push(w);
    }

    if opts.include_number {
        // Append a random digit to one randomly chosen word.
        let target = uniform(chosen.len() as u32)? as usize;
        let digit = uniform(10)?;
        chosen[target].push(char::from(b'0' + digit as u8));
    }

    Ok(chosen.join(&opts.separator))
}

/// A coarse strength estimate (0–4) plus an approximate log10 of the guess count.
/// This is a lightweight heuristic, not a full zxcvbn analysis.
pub fn strength(password: &str) -> (u8, f64) {
    let len = password.chars().count();
    if len == 0 {
        return (0, 0.0);
    }
    let mut pool = 0u32;
    if password.chars().any(|c| c.is_ascii_lowercase()) {
        pool += 26;
    }
    if password.chars().any(|c| c.is_ascii_uppercase()) {
        pool += 26;
    }
    if password.chars().any(|c| c.is_ascii_digit()) {
        pool += 10;
    }
    if password.chars().any(|c| !c.is_ascii_alphanumeric()) {
        pool += 32;
    }
    let pool = pool.max(1);
    // Entropy in bits ≈ length · log2(pool); guesses ≈ 2^entropy.
    let entropy_bits = (len as f64) * (pool as f64).log2();
    let guesses_log10 = entropy_bits * 2f64.log10();
    let score = match entropy_bits as u32 {
        0..=27 => 0,
        28..=45 => 1,
        46..=59 => 2,
        60..=127 => 3,
        _ => 4,
    };
    (score, guesses_log10)
}
