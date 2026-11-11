//! The shell line a worker's terminal runs. A shell line itself, and how text
//! reaches it, is the harness crate's (`afkconvoy_harness::line`): every
//! harness's start is one.

pub use afkconvoy_harness::line::{Line, quoted};

/// How much lower a worker runs than what started it: enough that it gives
/// way to the human and to the fleet's own gate on a busy host, and nothing
/// on one with nothing else to do.
pub const NICENESS: u64 = 10;

/// The line a worker's terminal runs: `line`, which starts the worker launch
/// command — in its role, or as it was given — run under `nice` with `tmp`
/// for its `TMPDIR`. The shell lowers itself, so whatever it reads the
/// command as — an alias, a function — and all it starts run lower too.
pub fn niced(tmp: &str, line: &str) -> String {
    Line::new()
        .fixed("export TMPDIR=")
        .word(tmp)
        .fixed(" && renice -n ")
        .number(NICENESS)
        .fixed(" -p $$ >/dev/null && ")
        .launch(line)
        .done()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_niced_line_runs_lower_with_its_own_tmpdir() {
        let tmp = "/it's a tmp";
        let line = niced(tmp, "printf '%s\\n' \"$TMPDIR\" && ps -o nice= -p $$");
        let read = std::process::Command::new("/bin/sh").arg("-c").arg(&line).output().unwrap();
        assert!(read.status.success(), "{line:?}: {read:?}");
        let read = String::from_utf8(read.stdout).unwrap();
        let (given, nice) = read.split_once('\n').unwrap();
        assert_eq!(given, tmp, "{line}");
        assert!(nice.trim().parse::<i64>().unwrap() >= NICENESS as i64, "{read:?}: {line}");
    }
}
