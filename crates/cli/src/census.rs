//! How many processes a test starts (sunfmin/control#1347).
//!
//! On macOS every start costs kernel time and wakes the system's daemons, so
//! the number of processes a test starts matters beside its CPU. The gate
//! counts them: it names a directory in `AFKCONVOY_PROCESS_CENSUS_DIR`, and
//! every command this crate and its tests start is made by [`command`], which
//! writes the program's name as one line to the file of the test it runs in —
//! `<binary id>@<test name>`, as nextest says them — and passes that file on
//! as `AFKCONVOY_PROCESS_CENSUS`, so an afkconvoy the test starts counts what
//! it starts in the same file. A line is written whole in one append: starts
//! of one test, on any thread or in any process, never tear each other.
//!
//! Without the directory nothing is written. The count is of starts made by
//! this crate's code, not the kernel's: a program a program started —
//! a script's `cat` — is not on it, which is why a test's fakes are programs
//! and not scripts. `./gate.sh` reads the files (docs/gate-baseline.md).

use std::ffi::OsStr;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::OnceLock;

/// The directory the gate has the files of one run's counts written in.
pub const DIR_ENV: &str = "AFKCONVOY_PROCESS_CENSUS_DIR";

/// The file the starts of the test running — or of the test a parent was
/// running — are written to.
pub const FILE_ENV: &str = "AFKCONVOY_PROCESS_CENSUS";

/// The file a test's name is counted in, in the directory the gate named.
fn file_of(dir: &str, binary: &str, test: &str) -> PathBuf {
    let name: String = format!("{binary}@{test}")
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | ':' | '@') {
                c
            } else {
                '+'
            }
        })
        .collect();
    PathBuf::from(dir).join(name)
}

/// Where this process counts its starts, if anywhere.
fn file() -> Option<&'static PathBuf> {
    static FILE: OnceLock<Option<PathBuf>> = OnceLock::new();
    FILE.get_or_init(|| {
        if let Some(file) = std::env::var_os(FILE_ENV) {
            return Some(PathBuf::from(file));
        }
        let dir = std::env::var(DIR_ENV).ok()?;
        let binary = std::env::var("NEXTEST_BINARY_ID").ok()?;
        let test = std::env::var("NEXTEST_TEST_NAME").ok()?;
        Some(file_of(&dir, &binary, &test))
    })
    .as_ref()
}

/// A command that is counted when the gate counts: [`Command::new`], and the
/// one place this crate makes a command.
pub fn command(program: impl AsRef<OsStr>) -> Command {
    let mut command = Command::new(&program);
    if let Some(file) = file() {
        let line = format!(
            "{}\n",
            std::path::Path::new(program.as_ref()).file_name().map_or_else(
                || program.as_ref().to_string_lossy().into_owned(),
                |name| name.to_string_lossy().into_owned(),
            )
        );
        if let Ok(mut open) = std::fs::OpenOptions::new().create(true).append(true).open(file) {
            let _ = open.write_all(line.as_bytes());
        }
        command.env(FILE_ENV, file);
    }
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_test_is_counted_in_a_file_named_by_its_binary_and_name() {
        assert_eq!(
            file_of("/c", "afkconvoy::cli", "gate::a test/x"),
            PathBuf::from("/c/afkconvoy::cli@gate::a+test+x")
        );
    }
}
