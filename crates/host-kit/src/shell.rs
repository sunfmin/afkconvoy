//! The login shell of the host's user, in which the host kit asks what a
//! machine has and runs what installs it: where a harness's program is, what
//! `npx skills` has installed, where tmux is, and the lines that install them.
//!
//! The program embedding the crate says how a line is run there (`Shell`):
//! afkconvoy runs it in the login shell a worker gets. A program with no shell
//! of its own takes `LoginShell`, the user's `$SHELL` as a login shell.

use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::{Result, bail};

/// How a line is run in the login shell of the host's user.
pub trait Shell {
    /// Runs `line` there → whether it exited zero and what it printed;
    /// `None` where it did not end within `timeout`, and was ended. An error
    /// says why it could not be started.
    fn run(&self, line: &str, timeout: Duration) -> Result<Option<(bool, String)>>;
}

/// Runs `line` in `shell` → what it printed, where it exited zero; an error
/// says why it did not, with the last lines it printed.
pub fn ask(shell: &dyn Shell, line: &str, timeout: Duration) -> Result<String> {
    match shell.run(line, timeout)? {
        Some((true, printed)) => Ok(printed),
        Some((false, printed)) => {
            let tail: Vec<&str> = printed.lines().rev().take(3).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            bail!("`{line}` failed: {}", tail.join(" / "))
        }
        None => bail!("`{line}` did not end within {}s", timeout.as_secs()),
    }
}

/// `text` as one word of a POSIX shell.
pub fn word(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

/// The user's `$SHELL` (`/bin/sh` where none is said), run as an interactive
/// login shell (`-lic`) in the home, so a line finds the programs a terminal
/// of the user's finds. What it printed on both streams is one text, the
/// standard error after the standard output. Past its timeout its process
/// group is ended.
#[derive(Debug, Clone, Copy, Default)]
pub struct LoginShell;

impl Shell for LoginShell {
    fn run(&self, line: &str, timeout: Duration) -> Result<Option<(bool, String)>> {
        let program = std::env::var("SHELL").ok().filter(|shell| !shell.is_empty());
        let program = program.unwrap_or_else(|| "/bin/sh".to_string());
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into());
        let mut child = match Command::new(&program)
            .args(["-lic", line])
            .current_dir(home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .spawn()
        {
            Ok(child) => child,
            Err(error) => bail!("the login shell `{program}` could not be started: {error}"),
        };
        let reader = |stream: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut bytes = vec![];
                if let Some(mut stream) = stream {
                    let _ = stream.read_to_end(&mut bytes);
                }
                bytes
            })
        };
        let out = reader(child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
        let err = reader(child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>));
        let began = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break Some(status);
            }
            if began.elapsed() >= timeout {
                break None;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        let Some(status) = status else {
            // The whole group: a line's own children hold its streams open.
            let group = format!("-{}", child.id());
            let _ = Command::new("kill").args(["-KILL", "--", &group]).status();
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        };
        let mut printed = out.join().unwrap_or_default();
        printed.extend(err.join().unwrap_or_default());
        Ok(Some((status.success(), String::from_utf8_lossy(&printed).into_owned())))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_is_quoted_whatever_it_holds() {
        assert_eq!(word("plain"), "'plain'");
        assert_eq!(word("it's"), r"'it'\''s'");
    }
}
