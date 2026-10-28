//! What every harness's session shares: a clock stamp, a worktree path too
//! long for one directory component, and a line of JSON. Which lines start
//! or end a turn stays in that harness's module.

use serde_json::Value;

use crate::turn::{Note, Turn, latest};

/// Epoch seconds of an ISO-8601 stamp in UTC, or `None` when it is not one.
/// A fractional second is dropped. The offset is `Z` or zero.
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
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    Some(days * 86_400 + u64::from(hour) * 3600 + u64::from(minute) * 60 + u64::from(second))
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
pub(super) fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    if month <= 2 {
        year += 1;
    }
    (year as i32, month as u32, day as u32)
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> Option<u64> {
    if !(1..=12).contains(&month) || day < 1 {
        return None;
    }
    year -= i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u64;
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * u64::from(month_index) + 2) / 5 + u64::from(day) - 1;
    let day_of_era = yoe * 365 + yoe / 4 - yoe / 100 + day_of_year;
    let shifted = i64::from(era) * 146_097 + day_of_era as i64 - 719_468;
    u64::try_from(shifted).ok()
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
    }
}
