use std::env;
use std::fs;
use std::path::PathBuf;

use ed25519_dalek::SigningKey;
use terrane_cap_worker::package::{hex, package_all};

fn main() {
    if let Err(error) = run() {
        eprintln!("terrane capability packager failed: {error}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut worker = None;
    let mut output = None;
    let mut platform = Some(env::consts::OS.to_string());
    let mut architecture = Some(env::consts::ARCH.to_string());
    let mut args = env::args().skip(1);
    while let Some(arg) = args.next() {
        let value = args
            .next()
            .ok_or_else(|| format!("missing value for {arg}"))?;
        match arg.as_str() {
            "--worker" => worker = Some(PathBuf::from(value)),
            "--output" => output = Some(PathBuf::from(value)),
            "--platform" => platform = Some(value),
            "--architecture" => architecture = Some(value),
            other => return Err(format!("unknown packager argument: {other}")),
        }
    }
    let signing_key = signing_key()?;
    let packaged = package_all(
        worker
            .as_ref()
            .ok_or_else(|| "--worker is required".to_string())?,
        output
            .as_ref()
            .ok_or_else(|| "--output is required".to_string())?,
        &signing_key,
        &platform.unwrap(),
        &architecture.unwrap(),
    )?;
    let verifying_key = hex(signing_key.verifying_key().as_bytes());
    fs::write(
        output
            .as_ref()
            .ok_or_else(|| "--output is required".to_string())?
            .join("verifying-key.hex"),
        format!("{verifying_key}\n"),
    )
    .map_err(|error| error.to_string())?;
    println!("packaged {} native capability bundles", packaged.len());
    println!("verifying key: {verifying_key}");
    Ok(())
}

fn signing_key() -> Result<SigningKey, String> {
    let value = env::var("TERRANE_CAP_SIGNING_KEY_HEX")
        .map_err(|_| "TERRANE_CAP_SIGNING_KEY_HEX is required".to_string())?;
    let bytes = decode_hex(&value)?;
    let seed: [u8; 32] = bytes
        .try_into()
        .map_err(|_| "TERRANE_CAP_SIGNING_KEY_HEX must encode exactly 32 bytes".to_string())?;
    Ok(SigningKey::from_bytes(&seed))
}

fn decode_hex(value: &str) -> Result<Vec<u8>, String> {
    if !value.len().is_multiple_of(2) {
        return Err("signing key hex has odd length".into());
    }
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let text = std::str::from_utf8(pair).map_err(|error| error.to_string())?;
            u8::from_str_radix(text, 16).map_err(|error| error.to_string())
        })
        .collect()
}
