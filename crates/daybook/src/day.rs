//! A day as the daybook titles it: `YYYY-MM-DD`, in the host's local time.

/// Today on this host, `YYYY-MM-DD` in its local time, by its clock. A
/// program with a clock of its own says its time to `day_at`.
pub fn today() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    day_at(now)
}

/// The day the epoch second `at` falls on, `YYYY-MM-DD` in this host's
/// local time.
pub fn day_at(at: u64) -> String {
    let at = at as libc::time_t;
    let mut local = std::mem::MaybeUninit::<libc::tm>::zeroed();
    // SAFETY: `localtime_r` writes only the `tm` it is handed.
    let local = unsafe {
        libc::localtime_r(&at, local.as_mut_ptr());
        local.assume_init()
    };
    format!("{:04}-{:02}-{:02}", local.tm_year + 1900, local.tm_mon + 1, local.tm_mday)
}

/// The day after `day`, a `YYYY-MM-DD` said as `day_named` takes it.
pub fn next_day(day: &str) -> String {
    let part = |range: std::ops::Range<usize>| day.get(range).and_then(|p| p.parse::<i32>().ok());
    let (Some(year), Some(month), Some(date)) = (part(0..4), part(5..7), part(8..10)) else {
        return day.to_string();
    };
    // SAFETY: an all-zero `tm` is a valid one.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    (tm.tm_year, tm.tm_mon, tm.tm_mday) = (year - 1900, month - 1, date + 1);
    (tm.tm_hour, tm.tm_isdst) = (12, -1);
    // SAFETY: `mktime` reads and normalizes only the `tm` it is handed.
    let at = unsafe { libc::mktime(&mut tm) };
    day_at(at.max(0) as u64)
}

/// A day, `YYYY-MM-DD`, as a writer says it.
pub fn day_named(said: &str) -> Result<String, String> {
    let bytes = said.as_bytes();
    let digits =
        [0, 1, 2, 3, 5, 6, 8, 9].iter().all(|at| bytes.get(*at).is_some_and(u8::is_ascii_digit));
    match bytes.len() == 10 && bytes[4] == b'-' && bytes[7] == b'-' && digits {
        true => Ok(said.to_string()),
        false => Err(format!("`{said}` is no day: say it `YYYY-MM-DD`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_day_is_said_as_a_date() {
        assert!(day_named("2026-11-03").is_ok());
        for wrong in ["2026-1-03", "20261103", "2026/11/03", "yesterday"] {
            assert!(day_named(wrong).is_err(), "{wrong}");
        }
    }

    #[test]
    fn today_is_a_date_and_the_next_day_follows_it() {
        let day = today();
        assert!(day_named(&day).is_ok(), "{day}");
        assert_eq!(next_day("2026-12-31"), "2027-01-01");
        assert_eq!(next_day("2028-02-28"), "2028-02-29");
    }
}
