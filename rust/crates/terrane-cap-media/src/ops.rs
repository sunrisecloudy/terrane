use serde_json::Value;
use terrane_cap_interface::{Error, Result};

pub const MAX_OPS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaKind {
    Image,
    Audio,
    Video,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MediaOp {
    Resize { max_width: u32, max_height: u32 },
    Crop { x: u32, y: u32, width: u32, height: u32 },
    Rotate { degrees: u16 },
    Thumbnail { size: u32 },
    Encode { format: String, quality: u8 },
    TranscodeAudio { format: String },
}

pub fn classify_mime(mime: &str) -> Result<MediaKind> {
    if mime.starts_with("image/") {
        Ok(MediaKind::Image)
    } else if mime.starts_with("audio/") {
        Ok(MediaKind::Audio)
    } else if mime.starts_with("video/") {
        Ok(MediaKind::Video)
    } else {
        Err(Error::InvalidInput(format!(
            "unsupported media mime: {mime}"
        )))
    }
}

pub fn parse_ops(ops_json: &str) -> Result<Vec<MediaOp>> {
    let value: Value = serde_json::from_str(ops_json)
        .map_err(|e| Error::InvalidInput(format!("invalid media ops JSON: {e}")))?;
    let ops = value
        .as_array()
        .ok_or_else(|| Error::InvalidInput("media ops must be an array".into()))?;
    if ops.is_empty() {
        return Err(Error::InvalidInput("media ops must not be empty".into()));
    }
    if ops.len() > MAX_OPS {
        return Err(Error::InvalidInput(format!(
            "media ops exceed per-transform cap of {MAX_OPS}"
        )));
    }
    ops.iter().map(parse_op).collect()
}

pub fn validate_ops_for_mime(mime: &str, ops_json: &str) -> Result<Vec<MediaOp>> {
    let kind = classify_mime(mime)?;
    let ops = parse_ops(ops_json)?;
    match kind {
        MediaKind::Image => {
            if ops.iter().any(|op| matches!(op, MediaOp::TranscodeAudio { .. })) {
                return Err(Error::InvalidInput(
                    "transcodeAudio is only valid for audio sources".into(),
                ));
            }
        }
        MediaKind::Audio => {
            if ops.len() != 1 || !matches!(ops[0], MediaOp::TranscodeAudio { .. }) {
                return Err(Error::InvalidInput(
                    "audio v1 supports exactly one transcodeAudio op".into(),
                ));
            }
        }
        MediaKind::Video => {
            return Err(Error::InvalidInput(
                "media.transform does not support video sources in v1".into(),
            ));
        }
    }
    Ok(ops)
}

pub fn op_names(ops_json: &str) -> Vec<String> {
    parse_ops(ops_json)
        .map(|ops| {
            ops.into_iter()
                .map(|op| match op {
                    MediaOp::Resize { .. } => "resize",
                    MediaOp::Crop { .. } => "crop",
                    MediaOp::Rotate { .. } => "rotate",
                    MediaOp::Thumbnail { .. } => "thumbnail",
                    MediaOp::Encode { .. } => "encode",
                    MediaOp::TranscodeAudio { .. } => "transcodeAudio",
                }
                .to_string())
                .collect()
        })
        .unwrap_or_else(|_| Vec::new())
}

fn parse_op(value: &Value) -> Result<MediaOp> {
    let obj = value
        .as_object()
        .ok_or_else(|| Error::InvalidInput("media op must be an object".into()))?;
    let op = string_field(obj, "op")?;
    match op {
        "resize" => Ok(MediaOp::Resize {
            max_width: positive_u32(obj, "maxWidth")?,
            max_height: positive_u32(obj, "maxHeight")?,
        }),
        "crop" => Ok(MediaOp::Crop {
            x: u32_field(obj, "x")?,
            y: u32_field(obj, "y")?,
            width: positive_u32(obj, "width")?,
            height: positive_u32(obj, "height")?,
        }),
        "rotate" => {
            let degrees = positive_u32(obj, "degrees")?;
            match degrees {
                90 | 180 | 270 => Ok(MediaOp::Rotate {
                    degrees: degrees as u16,
                }),
                _ => Err(Error::InvalidInput(
                    "rotate degrees must be 90, 180, or 270".into(),
                )),
            }
        }
        "thumbnail" => Ok(MediaOp::Thumbnail {
            size: positive_u32(obj, "size")?,
        }),
        "encode" => {
            let format = string_field(obj, "format")?.to_string();
            if !matches!(format.as_str(), "jpeg" | "png" | "webp") {
                return Err(Error::InvalidInput(
                    "encode format must be jpeg, png, or webp".into(),
                ));
            }
            let quality = positive_u32(obj, "quality")?;
            if quality > 100 {
                return Err(Error::InvalidInput(
                    "encode quality must be between 1 and 100".into(),
                ));
            }
            Ok(MediaOp::Encode {
                format,
                quality: quality as u8,
            })
        }
        "transcodeAudio" => {
            let format = string_field(obj, "format")?.to_string();
            if format != "wav" {
                return Err(Error::InvalidInput(
                    "audio v1 only supports transcodeAudio format wav".into(),
                ));
            }
            Ok(MediaOp::TranscodeAudio { format })
        }
        other => Err(Error::InvalidInput(format!("unknown media op: {other}"))),
    }
}

fn string_field<'a>(
    obj: &'a serde_json::Map<String, Value>,
    key: &str,
) -> Result<&'a str> {
    obj.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| Error::InvalidInput(format!("media op missing string field {key}")))
}

fn positive_u32(obj: &serde_json::Map<String, Value>, key: &str) -> Result<u32> {
    let value = u32_field(obj, key)?;
    if value == 0 {
        return Err(Error::InvalidInput(format!(
            "media op field {key} must be positive"
        )));
    }
    Ok(value)
}

fn u32_field(obj: &serde_json::Map<String, Value>, key: &str) -> Result<u32> {
    let raw = obj
        .get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| Error::InvalidInput(format!("media op missing integer field {key}")))?;
    u32::try_from(raw)
        .map_err(|_| Error::InvalidInput(format!("media op field {key} exceeds u32")))
}
