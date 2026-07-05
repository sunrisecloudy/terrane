use std::io::Cursor;
use std::process::Command;

use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView, ImageEncoder, ImageFormat};
use serde_json::json;
use sha2::{Digest as _, Sha256};
use symphonia::core::audio::{AudioBufferRef, SampleBuffer};
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;
use terrane_core::{Error, EventRecord, Result};

pub fn info(bytes: &[u8], mime: &str) -> Result<String> {
    if mime.starts_with("image/") {
        return image_info(bytes);
    }
    if mime.starts_with("audio/") {
        return audio_info(bytes);
    }
    if mime.starts_with("video/") {
        return video_info(bytes);
    }
    Err(Error::InvalidInput(format!(
        "unrecognized media mime: {mime}"
    )))
}

pub fn transform_with_home(
    home: &std::path::Path,
    app: &str,
    source_hash: &str,
    source_mime: &str,
    ops_json: &str,
    dest_name: &str,
) -> Result<Vec<EventRecord>> {
    let source_bytes = crate::blob_store::read_verified(home, source_hash)?;
    let ops = terrane_cap_media::ops::validate_ops_for_mime(source_mime, ops_json)?;
    let (bytes, mime) = if source_mime.starts_with("image/") {
        transform_image(&source_bytes, &ops)?
    } else if source_mime.starts_with("audio/") {
        transform_audio(&source_bytes, &ops)?
    } else {
        return Err(Error::InvalidInput(format!(
            "unsupported media transform source mime: {source_mime}"
        )));
    };
    if bytes.len() > terrane_cap_blob::MAX_BLOB_SIZE {
        return Err(Error::InvalidInput(format!(
            "media transform output exceeds {} bytes",
            terrane_cap_blob::MAX_BLOB_SIZE
        )));
    }
    let dest_hash = sha256_hex(&bytes);
    crate::blob_store::insert_if_absent(home, &dest_hash, &bytes)?;
    Ok(vec![
        terrane_cap_media::transformed_event(
            app,
            source_hash,
            ops_json,
            dest_name,
            &dest_hash,
            u64::try_from(bytes.len())
                .map_err(|_| Error::Storage("media output length overflow".into()))?,
            &mime,
        )?,
        terrane_cap_blob::stored_event(
            app,
            dest_name,
            &dest_hash,
            u64::try_from(bytes.len())
                .map_err(|_| Error::Storage("media output length overflow".into()))?,
            &mime,
        )?,
    ])
}

fn image_info(bytes: &[u8]) -> Result<String> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::InvalidInput(format!("unrecognized image: {e}")))?;
    let format = reader
        .format()
        .map(|f| format!("{f:?}").to_lowercase())
        .unwrap_or_else(|| "unknown".to_string());
    let image = reader
        .decode()
        .map_err(|e| Error::InvalidInput(format!("unrecognized image: {e}")))?;
    let (width, height) = image.dimensions();
    Ok(json!({
        "kind": "image",
        "width": width,
        "height": height,
        "format": format,
    })
    .to_string())
}

fn audio_info(bytes: &[u8]) -> Result<String> {
    let probed = probe_audio(bytes)?;
    let track = probed
        .format
        .default_track()
        .ok_or_else(|| Error::InvalidInput("unrecognized audio: no default track".into()))?;
    let params = &track.codec_params;
    let sample_rate = params.sample_rate.unwrap_or(0);
    let channels = params.channels.map(|c| c.count()).unwrap_or(0);
    let duration_ms = match (params.n_frames, params.sample_rate) {
        (Some(frames), Some(rate)) if rate > 0 => frames.saturating_mul(1000) / u64::from(rate),
        _ => 0,
    };
    Ok(json!({
        "kind": "audio",
        "durationMs": duration_ms,
        "sampleRateHz": sample_rate,
        "channels": channels,
        "codec": format!("{:?}", params.codec).to_lowercase(),
    })
    .to_string())
}

fn video_info(bytes: &[u8]) -> Result<String> {
    let ffprobe = Command::new("ffprobe").arg("-version").output();
    if ffprobe.is_err() {
        return Ok(json!({ "kind": "video", "probe": "unavailable" }).to_string());
    }
    let dir = tempfile::Builder::new()
        .prefix("terrane-media-probe")
        .tempdir()
        .map_err(|e| Error::Storage(format!("create ffprobe tempdir: {e}")))?;
    let path = dir.path().join("input.video");
    std::fs::write(&path, bytes)
        .map_err(|e| Error::Storage(format!("write ffprobe input: {e}")))?;
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height,codec_name,duration",
            "-of",
            "json",
        ])
        .arg(&path)
        .output()
        .map_err(|e| Error::Storage(format!("run ffprobe: {e}")))?;
    if !output.status.success() {
        return Ok(json!({ "kind": "video", "probe": "unavailable" }).to_string());
    }
    let value: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|e| Error::Runtime(format!("parse ffprobe output: {e}")))?;
    let stream = value
        .get("streams")
        .and_then(|v| v.as_array())
        .and_then(|a| a.first())
        .cloned()
        .unwrap_or_else(|| json!({}));
    let duration_ms = stream
        .get("duration")
        .and_then(|v| v.as_str())
        .and_then(|s| s.parse::<f64>().ok())
        .map(|seconds| (seconds * 1000.0) as u64)
        .unwrap_or(0);
    Ok(json!({
        "kind": "video",
        "width": stream.get("width").and_then(|v| v.as_u64()).unwrap_or(0),
        "height": stream.get("height").and_then(|v| v.as_u64()).unwrap_or(0),
        "durationMs": duration_ms,
        "codec": stream.get("codec_name").and_then(|v| v.as_str()).unwrap_or("unknown"),
    })
    .to_string())
}

fn transform_image(
    bytes: &[u8],
    ops: &[terrane_cap_media::ops::MediaOp],
) -> Result<(Vec<u8>, String)> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| Error::InvalidInput(format!("unrecognized image: {e}")))?;
    let (width, height) = reader
        .into_dimensions()
        .map_err(|e| Error::InvalidInput(format!("unrecognized image: {e}")))?;
    if u64::from(width) * u64::from(height) > terrane_cap_media::MAX_PIXEL_BUDGET {
        return Err(Error::InvalidInput(format!(
            "decoded image exceeds {} pixels",
            terrane_cap_media::MAX_PIXEL_BUDGET
        )));
    }
    let mut image = image::load_from_memory(bytes)
        .map_err(|e| Error::InvalidInput(format!("unrecognized image: {e}")))?;
    let mut format = "png".to_string();
    let mut quality = 80u8;
    for op in ops {
        match op {
            terrane_cap_media::ops::MediaOp::Resize {
                max_width,
                max_height,
            } => {
                image = image.resize(*max_width, *max_height, image::imageops::FilterType::Lanczos3);
            }
            terrane_cap_media::ops::MediaOp::Crop {
                x,
                y,
                width,
                height,
            } => {
                if x.saturating_add(*width) > image.width()
                    || y.saturating_add(*height) > image.height()
                {
                    return Err(Error::InvalidInput("crop rectangle exceeds image bounds".into()));
                }
                image = image.crop_imm(*x, *y, *width, *height);
            }
            terrane_cap_media::ops::MediaOp::Rotate { degrees } => {
                image = match degrees {
                    90 => image.rotate90(),
                    180 => image.rotate180(),
                    270 => image.rotate270(),
                    _ => return Err(Error::InvalidInput("bad rotation degrees".into())),
                };
            }
            terrane_cap_media::ops::MediaOp::Thumbnail { size } => {
                image = image.thumbnail(*size, *size);
                format = "jpeg".to_string();
                quality = 80;
            }
            terrane_cap_media::ops::MediaOp::Encode {
                format: next,
                quality: next_quality,
            } => {
                format = next.clone();
                quality = *next_quality;
            }
            terrane_cap_media::ops::MediaOp::TranscodeAudio { .. } => {
                return Err(Error::InvalidInput(
                    "transcodeAudio is not valid for image sources".into(),
                ));
            }
        }
    }
    encode_image(&image, &format, quality)
}

fn encode_image(image: &DynamicImage, format: &str, quality: u8) -> Result<(Vec<u8>, String)> {
    let mut out = Vec::new();
    match format {
        "jpeg" => {
            let rgb = image.to_rgb8();
            JpegEncoder::new_with_quality(&mut out, quality)
                .write_image(
                    rgb.as_raw(),
                    rgb.width(),
                    rgb.height(),
                    image::ExtendedColorType::Rgb8,
                )
                .map_err(|e| Error::Storage(format!("encode jpeg: {e}")))?;
            Ok((out, "image/jpeg".to_string()))
        }
        "png" => {
            image
                .write_to(&mut Cursor::new(&mut out), ImageFormat::Png)
                .map_err(|e| Error::Storage(format!("encode png: {e}")))?;
            Ok((out, "image/png".to_string()))
        }
        "webp" => {
            image
                .write_to(&mut Cursor::new(&mut out), ImageFormat::WebP)
                .map_err(|e| Error::Storage(format!("encode webp: {e}")))?;
            Ok((out, "image/webp".to_string()))
        }
        other => Err(Error::InvalidInput(format!(
            "unsupported image encode format: {other}"
        ))),
    }
}

fn transform_audio(
    bytes: &[u8],
    ops: &[terrane_cap_media::ops::MediaOp],
) -> Result<(Vec<u8>, String)> {
    match ops {
        [terrane_cap_media::ops::MediaOp::TranscodeAudio { format }] if format == "wav" => {
            decode_audio_to_wav(bytes).map(|bytes| (bytes, "audio/wav".to_string()))
        }
        _ => Err(Error::InvalidInput(
            "audio v1 supports exactly one transcodeAudio wav op".into(),
        )),
    }
}

fn decode_audio_to_wav(bytes: &[u8]) -> Result<Vec<u8>> {
    let mut probed = probe_audio(bytes)?;
    let track = probed
        .format
        .default_track()
        .ok_or_else(|| Error::InvalidInput("unrecognized audio: no default track".into()))?;
    let track_id = track.id;
    let params = track.codec_params.clone();
    let sample_rate = params
        .sample_rate
        .ok_or_else(|| Error::InvalidInput("audio sample rate unavailable".into()))?;
    let channels = params
        .channels
        .ok_or_else(|| Error::InvalidInput("audio channels unavailable".into()))?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&params, &DecoderOptions::default())
        .map_err(|e| Error::InvalidInput(format!("open audio decoder: {e}")))?;
    let mut out = Cursor::new(Vec::new());
    {
        let spec = hound::WavSpec {
            channels: u16::try_from(channels.count())
                .map_err(|_| Error::InvalidInput("audio channel count overflow".into()))?,
            sample_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut writer = hound::WavWriter::new(&mut out, spec)
            .map_err(|e| Error::Storage(format!("create wav writer: {e}")))?;
        loop {
            let packet = match probed.format.next_packet() {
                Ok(packet) => packet,
                Err(symphonia::core::errors::Error::IoError(_)) => break,
                Err(e) => return Err(Error::InvalidInput(format!("read audio packet: {e}"))),
            };
            if packet.track_id() != track_id {
                continue;
            }
            let decoded = decoder
                .decode(&packet)
                .map_err(|e| Error::InvalidInput(format!("decode audio packet: {e}")))?;
            write_audio_buffer(&mut writer, decoded)?;
        }
        writer
            .finalize()
            .map_err(|e| Error::Storage(format!("finalize wav: {e}")))?;
    }
    Ok(out.into_inner())
}

fn write_audio_buffer<W: std::io::Write + std::io::Seek>(
    writer: &mut hound::WavWriter<W>,
    decoded: AudioBufferRef<'_>,
) -> Result<()> {
    let spec = *decoded.spec();
    let duration = decoded.capacity() as u64;
    let mut samples = SampleBuffer::<i16>::new(duration, spec);
    samples.copy_interleaved_ref(decoded);
    for sample in samples.samples() {
        writer
            .write_sample(*sample)
            .map_err(|e| Error::Storage(format!("write wav sample: {e}")))?;
    }
    Ok(())
}

struct ProbedAudio {
    format: Box<dyn symphonia::core::formats::FormatReader>,
}

fn probe_audio(bytes: &[u8]) -> Result<ProbedAudio> {
    let cursor = Cursor::new(bytes.to_vec());
    let source = MediaSourceStream::new(Box::new(cursor), Default::default());
    let probed = symphonia::default::get_probe()
        .format(
            &Hint::new(),
            source,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| Error::InvalidInput(format!("unrecognized audio: {e}")))?;
    Ok(ProbedAudio {
        format: probed.format,
    })
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        let _ = write!(out, "{byte:02x}");
    }
    out
}
