use std::path::Path;
use std::process::Command;

use terrane_cap_tts::{rendered_event, sha256_hex, RenderRecord, TTS_MIME_WAV};
use terrane_core::{Error, EventRecord, Result};

pub fn speak(text: &str, voice: Option<&str>, rate_milli: u32) -> Result<()> {
    ensure_macos("tts.speak")?;
    let mut command = say_command(text, voice, rate_milli);
    let status = command
        .status()
        .map_err(|e| Error::Storage(format!("run /usr/bin/say: {e}")))?;
    if status.success() {
        Ok(())
    } else {
        Err(Error::Runtime(format!(
            "/usr/bin/say exited with status {status}"
        )))
    }
}

pub fn render(
    home: &Path,
    app: &str,
    text: &str,
    text_hash: &str,
    voice: Option<&str>,
    rate_milli: u32,
) -> Result<Vec<EventRecord>> {
    ensure_macos("tts.render")?;
    let dir = tempfile::tempdir()
        .map_err(|e| Error::Storage(format!("create tts temp dir: {e}")))?;
    let out_path = dir.path().join("tts.wav");
    let mut command = say_command(text, voice, rate_milli);
    command
        .arg("-o")
        .arg(&out_path)
        .arg("--data-format=LEI16");
    let status = command
        .status()
        .map_err(|e| Error::Storage(format!("run /usr/bin/say: {e}")))?;
    if !status.success() {
        return Err(Error::Runtime(format!(
            "/usr/bin/say exited with status {status}"
        )));
    }

    let bytes = std::fs::read(&out_path)
        .map_err(|e| Error::Storage(format!("read rendered speech {}: {e}", out_path.display())))?;
    let blob_hash = sha256_hex(&bytes);
    crate::blob_store::insert_if_absent(home, &blob_hash, &bytes)?;
    let size = u64::try_from(bytes.len())
        .map_err(|_| Error::Storage("tts render byte length overflow".into()))?;
    let duration_ms = wav_duration_ms(&bytes).unwrap_or(0);

    Ok(vec![
        terrane_cap_blob::stored_event(
            app,
            format!("__tts__/{text_hash}"),
            &blob_hash,
            size,
            TTS_MIME_WAV,
        )?,
        rendered_event(&RenderRecord {
            app: app.to_string(),
            text_hash: text_hash.to_string(),
            voice: voice.map(str::to_string),
            rate_milli,
            blob_hash,
            size,
            mime: TTS_MIME_WAV.to_string(),
            duration_ms,
        })?,
    ])
}

pub fn voices_json() -> Result<String> {
    ensure_macos("tts.voices")?;
    let output = Command::new("/usr/bin/say")
        .arg("-v")
        .arg("?")
        .output()
        .map_err(|e| Error::Storage(format!("run /usr/bin/say -v ?: {e}")))?;
    if !output.status.success() {
        return Err(Error::Runtime(format!(
            "/usr/bin/say -v ? exited with status {}",
            output.status
        )));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let voices = stdout
        .lines()
        .filter_map(parse_voice_line)
        .collect::<Vec<_>>();
    serde_json::to_string(&voices)
        .map_err(|e| Error::Runtime(format!("encode tts voices: {e}")))
}

fn say_command(text: &str, voice: Option<&str>, rate_milli: u32) -> Command {
    let mut command = Command::new("/usr/bin/say");
    if let Some(voice) = voice {
        command.arg("-v").arg(voice);
    }
    command.arg("-r").arg(words_per_minute(rate_milli).to_string());
    command.arg(text);
    command
}

fn words_per_minute(rate_milli: u32) -> u32 {
    let base = 175u32;
    ((base * rate_milli) / 1000).max(1)
}

fn ensure_macos(verb: &str) -> Result<()> {
    if cfg!(target_os = "macos") {
        Ok(())
    } else {
        Err(Error::Runtime(format!(
            "{verb} unsupported on this CLI host; supported hosts: macOS CLI, mac app"
        )))
    }
}

fn parse_voice_line(line: &str) -> Option<serde_json::Value> {
    let mut parts = line.split_whitespace();
    let id = parts.next()?;
    let lang = parts.next().unwrap_or("");
    Some(serde_json::json!({
        "id": id,
        "name": id,
        "lang": lang,
        "kind": "system",
    }))
}

fn wav_duration_ms(bytes: &[u8]) -> Option<u64> {
    let cursor = std::io::Cursor::new(bytes);
    let reader = hound::WavReader::new(cursor).ok()?;
    let spec = reader.spec();
    if spec.sample_rate == 0 || spec.channels == 0 {
        return None;
    }
    let frames = u64::from(reader.duration()) / u64::from(spec.channels);
    Some((frames * 1000) / u64::from(spec.sample_rate))
}
