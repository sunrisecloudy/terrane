use terrane_cap_interface::{Error, Result};

const MINUTE: u64 = 60;

pub(crate) fn validate_cron(expr: &str) -> Result<()> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(Error::InvalidInput(
            "cron must have five fields: minute hour day-of-month month day-of-week".into(),
        ));
    }
    validate_field(fields[0], 0, 59, "minute")?;
    validate_field(fields[1], 0, 23, "hour")?;
    validate_field(fields[2], 1, 31, "day-of-month")?;
    validate_field(fields[3], 1, 12, "month")?;
    validate_field(fields[4], 0, 7, "day-of-week")?;
    Ok(())
}

pub(crate) fn validate_timezone(timezone: &str) -> Result<()> {
    if timezone == "UTC" || timezone == "Etc/UTC" {
        return Ok(());
    }
    if timezone.len() > 64
        || !timezone.contains('/')
        || timezone.starts_with('/')
        || timezone.ends_with('/')
        || !timezone
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+'))
    {
        return Err(Error::InvalidInput(format!(
            "timezone must be UTC or an IANA-style zone name, got {timezone:?}"
        )));
    }
    Ok(())
}

pub fn next_due_after(cron: &str, after_epoch_secs: u64) -> Result<u64> {
    validate_cron(cron)?;
    let fields: Vec<&str> = cron.split_whitespace().collect();
    if fields[1..].iter().all(|field| *field == "*") {
        let minute = fields[0];
        if minute == "*" {
            return Ok(next_minute(after_epoch_secs));
        }
        if let Some(step) = minute.strip_prefix("*/") {
            let step = parse_number(step, 1, 59, "minute step")? as u64;
            let base = next_minute(after_epoch_secs);
            let minute_index = base / MINUTE;
            let remainder = minute_index % step;
            return Ok(if remainder == 0 {
                base
            } else {
                (minute_index + (step - remainder)) * MINUTE
            });
        }
        if let Ok(fixed) = minute.parse::<u8>() {
            if fixed <= 59 {
                let base = next_minute(after_epoch_secs);
                let hour_start = (base / 3600) * 3600;
                let candidate = hour_start + u64::from(fixed) * MINUTE;
                return Ok(if candidate >= base {
                    candidate
                } else {
                    candidate + 3600
                });
            }
        }
    }
    Err(Error::InvalidInput(
        "scheduler host due calculation supports minute-only cron forms: '* * * * *', '*/n * * * *', or 'm * * * *'"
            .into(),
    ))
}

fn next_minute(after_epoch_secs: u64) -> u64 {
    ((after_epoch_secs / MINUTE) + 1) * MINUTE
}

fn validate_field(field: &str, min: u8, max: u8, label: &str) -> Result<()> {
    if field.is_empty() {
        return Err(Error::InvalidInput(format!("cron {label} field is empty")));
    }
    for part in field.split(',') {
        validate_part(part, min, max, label)?;
    }
    Ok(())
}

fn validate_part(part: &str, min: u8, max: u8, label: &str) -> Result<()> {
    let (base, step) = match part.split_once('/') {
        Some((base, step)) => (base, Some(step)),
        None => (part, None),
    };
    if let Some(step) = step {
        parse_number(step, 1, max, &format!("{label} step"))?;
    }
    if base == "*" {
        return Ok(());
    }
    if let Some((start, end)) = base.split_once('-') {
        let start = parse_number(start, min, max, label)?;
        let end = parse_number(end, min, max, label)?;
        if start > end {
            return Err(Error::InvalidInput(format!(
                "cron {label} range start must be <= end"
            )));
        }
        return Ok(());
    }
    parse_number(base, min, max, label)?;
    Ok(())
}

fn parse_number(raw: &str, min: u8, max: u8, label: &str) -> Result<u8> {
    let value = raw
        .parse::<u8>()
        .map_err(|_| Error::InvalidInput(format!("cron {label} must be numeric, got {raw:?}")))?;
    if value < min || value > max {
        return Err(Error::InvalidInput(format!(
            "cron {label} must be between {min} and {max}, got {value}"
        )));
    }
    Ok(value)
}
