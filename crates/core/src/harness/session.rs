//! What every harness's session shares: a clock stamp, a worktree path too
//! long for one directory component, and a line of JSON. Which lines start
//! or end a turn stays in that harness's module.

use serde_json::Value;

use crate::turn::{Note, Turn, latest};

/// Epoch seconds of an ISO-8601 stamp in UTC, or `None` when it is not one.
/// A fractional second is dropped. The offset is `Z` or zero.
// Compared, not `RangeInclusive::contains`: Flux cannot see that it never panics.
#[allow(clippy::manual_range_contains)]
pub(super) fn parse_iso(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year: i32 = text[0..4].parse().ok()?;
    let month: u32 = text[5..7].parse().ok()?;
    let day: u32 = text[8..10].parse().ok()?;
    let hour: u32 = text[11..13].parse().ok()?;
    let minute: u32 = text[14..16].parse().ok()?;
    let second: u32 = text[17..19].parse().ok()?;
    let mut rest = &text[19..];
    if let Some(stripped) = rest.strip_prefix('.') {
        let digits = stripped.find(|ch: char| !ch.is_ascii_digit()).unwrap_or(stripped.len());
        rest = &stripped[digits..];
    }
    if rest != "Z" && rest != "+00:00" && rest != "-00:00" {
        return None;
    }
    if month < 1 || month > 12 || day < 1 || day > 31 || hour > 23 || minute > 59 || second > 60 {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    days.checked_mul(86_400)?
        .checked_add(u64::from(hour).checked_mul(3_600)?)?
        .checked_add(u64::from(minute).checked_mul(60)?)?
        .checked_add(u64::from(second))
}

/// `at` as an ISO-8601 stamp in UTC, whole seconds.
pub(super) fn format_iso(at: u64) -> String {
    let (year, month, day) = civil_from_days((at / 86_400) as i64);
    let rest = at % 86_400;
    let hour = rest / 3600;
    let minute = (rest % 3600) / 60;
    let second = rest % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

/// Epoch seconds. A value past year 2286 is milliseconds.
pub(super) fn epoch_number(value: u64) -> u64 {
    if value > 10_000_000_000 { value / 1000 } else { value }
}

/// `seconds` when the line carries them, otherwise an ISO `timestamp`.
pub(super) fn number_at(seconds: Option<u64>, timestamp: Option<&str>) -> Option<u64> {
    seconds.map(epoch_number).or_else(|| timestamp.and_then(parse_iso))
}

/// The year, month and day of an epoch day count, for a session laid out by date.
/// A count whose arithmetic does not fit is the Unix epoch.
pub(super) fn civil_from_days(days: i64) -> (i32, u32, u32) {
    epoch_day(days).unwrap_or((1970, 1, 1))
}

/// Howard Hinnant's civil-from-days. Every product is checked: Flux rejects the
/// bare arithmetic, and a day count that does not fit is not a session day.
fn epoch_day(days: i64) -> Option<(i32, u32, u32)> {
    let shifted = days.checked_add(719_468)?;
    let adjusted = if shifted >= 0 { shifted } else { shifted.checked_sub(146_096)? };
    let era = adjusted / 146_097;
    let day_of_era = u64::try_from(shifted.checked_sub(era.checked_mul(146_097)?)?).ok()?;
    let year_of_era = day_of_era
        .checked_sub(day_of_era / 1_460)?
        .checked_add(day_of_era / 36_524)?
        .checked_sub(day_of_era / 146_096)?
        / 365;
    let mut year = i64::try_from(year_of_era).ok()?.checked_add(era.checked_mul(400)?)?;
    let day_of_year = day_of_era.checked_sub(
        year_of_era
            .checked_mul(365)?
            .checked_add(year_of_era / 4)?
            .checked_sub(year_of_era / 100)?,
    )?;
    let month_index = day_of_year.checked_mul(5)?.checked_add(2)? / 153;
    let day = day_of_year
        .checked_sub(month_index.checked_mul(153)?.checked_add(2)? / 5)?
        .checked_add(1)?;
    let month =
        if month_index < 10 { month_index.checked_add(3)? } else { month_index.checked_sub(9)? };
    if month <= 2 {
        year = year.checked_add(1)?;
    }
    Some((i32::try_from(year).ok()?, u32::try_from(month).ok()?, u32::try_from(day).ok()?))
}

// Compared, not `RangeInclusive::contains`: Flux cannot see that it never panics.
#[allow(clippy::manual_range_contains)]
fn days_from_civil(mut year: i32, month: u32, day: u32) -> Option<u64> {
    if month < 1 || month > 12 || day < 1 {
        return None;
    }
    year = year.checked_sub(i32::from(month <= 2))?;
    let era = if year >= 0 { year } else { year.checked_sub(399)? } / 400;
    let yoe = u64::try_from(year.checked_sub(era.checked_mul(400)?)?).ok()?;
    let month_index = if month > 2 { month.checked_sub(3)? } else { month.checked_add(9)? };
    let day_of_year = (u64::from(month_index).checked_mul(153)?.checked_add(2)? / 5)
        .checked_add(u64::from(day))?
        .checked_sub(1)?;
    let day_of_era = yoe
        .checked_mul(365)?
        .checked_add(yoe / 4)?
        .checked_sub(yoe / 100)?
        .checked_add(day_of_year)?;
    let shifted = i64::from(era)
        .checked_mul(146_097)?
        .checked_add(i64::try_from(day_of_era).ok()?)?
        .checked_sub(719_468)?;
    u64::try_from(shifted).ok()
}

/// One JSON object, field by field. `json!` unwraps a `Result` Flux cannot prove.
pub(super) fn object(fields: Vec<(&str, Value)>) -> Value {
    Value::Object(fields.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
}

/// The same worktree, ignoring a trailing slash.
pub(super) fn same_cwd(recorded: &str, cwd: &str) -> bool {
    recorded.trim_end_matches('/') == cwd.trim_end_matches('/')
}

/// One path component holds at most this many bytes. A worktree path longer
/// than that cannot be the harness's own directory name, so the record and
/// the reader share a short name for it. A path that fits keeps the name the
/// harness writes.
pub(super) fn stored_name(natural: String, cwd: &str) -> String {
    if natural.len() <= 255 { natural } else { format!("afkconvoy-{}", mark(cwd)) }
}

/// A short name for `cwd`, shared by the record and the reader.
pub(super) fn mark(cwd: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

/// The directory name pi and DeepSeek Harness write for `cwd`: the path with
/// a trailing slash, every character that is not a letter or a digit turned
/// into `-`, and a `-` at each end.
pub(super) fn wrapped_dir(cwd: &str) -> String {
    let mut with_slash = cwd.to_string();
    if !with_slash.ends_with('/') {
        with_slash.push('/');
    }
    let mut body = String::new();
    for ch in with_slash.chars() {
        if ch.is_ascii_alphanumeric() {
            body.push(ch);
        } else {
            body.push('-');
        }
    }
    stored_name(format!("-{body}-"), cwd)
}

/// Each JSON object `text` holds, one a line. A blank line and a line that
/// is not JSON are not events.
pub(super) fn json_lines(text: &str) -> impl Iterator<Item = Value> + '_ {
    text.lines().filter_map(|line| {
        let line = line.trim();
        if line.is_empty() { None } else { serde_json::from_str(line).ok() }
    })
}

/// The turn `notes` leave, in the order the session wrote them.
pub(super) fn turn_of(notes: &[Note], not_before: Option<u64>) -> Option<Turn> {
    latest(notes, not_before)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: u64 = 1_793_106_373;

    #[test]
    fn an_iso_stamp_is_epoch_seconds() {
        assert_eq!(parse_iso("2026-10-27T13:06:13Z"), Some(AT));
        assert_eq!(parse_iso("2026-10-27T13:06:13.022Z"), Some(AT));
        assert_eq!(format_iso(AT), "2026-10-27T13:06:13Z");
        assert_eq!(parse_iso(&format_iso(AT)), Some(AT));
        assert_eq!(format_iso(0), "1970-01-01T00:00:00Z");
        assert_eq!(parse_iso("1970-01-01T00:00:00Z"), Some(0));
    }
}
