use terrane_cap_interface::{Error, Result};

const MINUTE_MS: u64 = 60_000;
const SEARCH_LIMIT_MINUTES: u64 = 366 * 24 * 60 * 5;

#[derive(Debug, Clone, PartialEq, Eq)]
struct CronSpec {
    minute: Field,
    hour: Field,
    day_of_month: Field,
    month: Field,
    day_of_week: Field,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Field {
    values: Vec<u8>,
}

pub(crate) fn canonical_cron(expr: &str) -> Result<String> {
    let normalized = expr.split_whitespace().collect::<Vec<_>>().join(" ");
    parse_cron(&normalized)?;
    Ok(normalized)
}

pub fn next_after(expr: &str, after_epoch_ms: u64) -> Result<u64> {
    let spec = parse_cron(expr)?;
    let start = ((after_epoch_ms / MINUTE_MS) + 1) * MINUTE_MS;
    find_matching_minute(&spec, start, true)
}

pub(crate) fn latest_at_or_before(expr: &str, now_epoch_ms: u64) -> Result<Option<u64>> {
    let spec = parse_cron(expr)?;
    let start = (now_epoch_ms / MINUTE_MS) * MINUTE_MS;
    find_matching_minute(&spec, start, false).map(Some)
}

pub(crate) fn missed_since(expr: &str, last_scheduled_for: u64, now_epoch_ms: u64) -> Result<Vec<u64>> {
    let mut out = Vec::new();
    let mut cursor = last_scheduled_for;
    loop {
        let next = next_after(expr, cursor)?;
        if next > now_epoch_ms {
            return Ok(out);
        }
        out.push(next);
        cursor = next;
    }
}

fn find_matching_minute(spec: &CronSpec, start_epoch_ms: u64, forward: bool) -> Result<u64> {
    let mut cursor = start_epoch_ms;
    for _ in 0..SEARCH_LIMIT_MINUTES {
        if matches_epoch_minute(spec, cursor)? {
            return Ok(cursor);
        }
        cursor = if forward {
            cursor.checked_add(MINUTE_MS)
        } else {
            cursor.checked_sub(MINUTE_MS)
        }
        .ok_or_else(|| Error::InvalidInput("cron search overflowed epoch range".into()))?;
    }
    Err(Error::InvalidInput(
        "cron has no matching UTC minute within five years".into(),
    ))
}

fn matches_epoch_minute(spec: &CronSpec, epoch_ms: u64) -> Result<bool> {
    let seconds = epoch_ms / 1000;
    let minute = ((seconds / 60) % 60) as u8;
    let hour = ((seconds / 3600) % 24) as u8;
    let days = (seconds / 86_400) as i64;
    let (year, month, day) = civil_from_days(days)?;
    let dow = day_of_week(days);
    Ok(spec.minute.contains(minute)
        && spec.hour.contains(hour)
        && spec.month.contains(month)
        && dom_dow_match(spec, day, dow)
        && year >= 1970)
}

fn dom_dow_match(spec: &CronSpec, day: u8, dow: u8) -> bool {
    let dom_any = spec.day_of_month.is_full_range(1, 31);
    let dow_any = spec.day_of_week.is_full_range(0, 7);
    let dom_match = spec.day_of_month.contains(day);
    let dow_match = spec.day_of_week.contains(dow) || (dow == 0 && spec.day_of_week.contains(7));
    if !dom_any && !dow_any {
        dom_match || dow_match
    } else {
        dom_match && dow_match
    }
}

fn parse_cron(expr: &str) -> Result<CronSpec> {
    let fields: Vec<&str> = expr.split_whitespace().collect();
    if fields.len() != 5 {
        return Err(Error::InvalidInput(
            "cron must have five fields: minute hour day-of-month month day-of-week".into(),
        ));
    }
    Ok(CronSpec {
        minute: parse_field(fields[0], 0, 59, "minute")?,
        hour: parse_field(fields[1], 0, 23, "hour")?,
        day_of_month: parse_field(fields[2], 1, 31, "day-of-month")?,
        month: parse_field(fields[3], 1, 12, "month")?,
        day_of_week: parse_field(fields[4], 0, 7, "day-of-week")?,
    })
}

fn parse_field(field: &str, min: u8, max: u8, label: &str) -> Result<Field> {
    if field.is_empty() {
        return Err(Error::InvalidInput(format!("cron {label} field is empty")));
    }
    let mut present = vec![false; usize::from(max) + 1];
    for part in field.split(',') {
        add_part(&mut present, part, min, max, label)?;
    }
    let values = (min..=max)
        .filter(|value| present[usize::from(*value)])
        .collect::<Vec<_>>();
    if values.is_empty() {
        return Err(Error::InvalidInput(format!("cron {label} field is empty")));
    }
    Ok(Field { values })
}

fn add_part(present: &mut [bool], part: &str, min: u8, max: u8, label: &str) -> Result<()> {
    let (base, step) = match part.split_once('/') {
        Some((base, step)) => (base, Some(parse_number(step, 1, max, &format!("{label} step"))?)),
        None => (part, None),
    };
    let (start, end) = if base == "*" {
        (min, max)
    } else if let Some((start, end)) = base.split_once('-') {
        let start = parse_number(start, min, max, label)?;
        let end = parse_number(end, min, max, label)?;
        if start > end {
            return Err(Error::InvalidInput(format!(
                "cron {label} range start must be <= end"
            )));
        }
        (start, end)
    } else {
        let value = parse_number(base, min, max, label)?;
        (value, value)
    };
    let step = step.unwrap_or(1);
    let mut value = start;
    while value <= end {
        present[usize::from(value)] = true;
        let Some(next) = value.checked_add(step) else {
            break;
        };
        if next == value {
            break;
        }
        value = next;
    }
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

impl Field {
    fn contains(&self, value: u8) -> bool {
        self.values.binary_search(&value).is_ok()
    }

    fn is_full_range(&self, min: u8, max: u8) -> bool {
        self.values.len() == usize::from(max - min + 1)
            && self.values.first() == Some(&min)
            && self.values.last() == Some(&max)
    }
}

fn civil_from_days(days_since_epoch: i64) -> Result<(i32, u8, u8)> {
    let z = days_since_epoch
        .checked_add(719_468)
        .ok_or_else(|| Error::InvalidInput("epoch day overflow".into()))?;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };
    Ok((year as i32, m as u8, d as u8))
}

fn day_of_week(days_since_epoch: i64) -> u8 {
    ((days_since_epoch + 4).rem_euclid(7)) as u8
}
