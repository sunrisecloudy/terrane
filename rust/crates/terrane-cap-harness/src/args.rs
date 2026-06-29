use terrane_cap_interface::{arg, Error, Result};

pub const DEFAULT_HARNESS: &str = "codex";

pub(crate) struct ParsedHarnessArgs {
    pub harness: String,
    pub required: Vec<String>,
    pub tail: String,
}

pub(crate) fn parse_harness_args(
    args: &[String],
    required_count: usize,
) -> Result<ParsedHarnessArgs> {
    let mut harness = DEFAULT_HARNESS.to_string();
    let mut rest = args;
    if matches!(args.first().map(String::as_str), Some("--harness")) {
        harness = supported_harness(arg(args, 1, "harness")?)?;
        rest = args.get(2..).unwrap_or_default();
    }
    if rest.len() < required_count {
        return Err(Error::InvalidInput(format!(
            "missing {}",
            match required_count {
                4 => "draft id, app id, app name, or prompt",
                3 => "run id, app id, or prompt",
                _ => "required argument",
            }
        )));
    }
    let required = rest[..required_count - 1].to_vec();
    let tail = rest[required_count - 1..].join(" ");
    Ok(ParsedHarnessArgs {
        harness,
        required,
        tail,
    })
}

fn supported_harness(raw: String) -> Result<String> {
    let harness = raw.trim();
    match harness {
        "codex" | "claude" | "claude-code" | "opencode" => Ok(harness.to_string()),
        "" => Err(Error::InvalidInput("harness must not be empty".into())),
        other => Err(Error::InvalidInput(format!(
            "unsupported harness: {other}; expected codex, claude-code, claude, or opencode"
        ))),
    }
}
