//! Whether a worker's harness holds its terminal (sunfmin/control#1150).
//!
//! Orca runs a worker's harness as the foreground job of its terminal's
//! login shell. Stopped and continued, the harness is resumed in the
//! background: the shell holds the terminal again, the harness reads none of
//! it — or stops at its first read — and a line typed there is the shell's
//! to run. A line typed at a terminal and not confirmed delivered can also
//! wait, unread, in the terminal's input queue, until the harness exits and
//! the shell reads it as a command.
//!
//! What a process is to its terminal is the kernel's: `ps` says each one's
//! process group, its terminal's foreground process group and whether it is
//! stopped. The harness is the process of the worker (`hot::tree`) its shell
//! started: the one whose parent is no process of the worker.

use std::path::Path;
use std::time::{Duration, Instant};

use afkconvoy_core::harness::Keystroke;
use anyhow::{Context, Result};

use crate::hot::Proc;

/// The note beside a worker's brief naming the terminal device its harness
/// was last seen on, such as `ttys012`: where a bare shell is looked for once
/// the harness is gone (`bare_shell`), and whose unread input is dropped.
pub(crate) const TTY_FILE: &str = "convoy-worker-tty";

/// What brings a job of a shell back to its terminal's foreground, typed at
/// the shell.
pub(crate) const FG: Keystroke = Keystroke { text: Some("fg"), enter: true };

/// How long a harness brought back is given to hold its terminal again.
const RECOVER_WAIT: Duration = Duration::from_secs(3);

/// One process's place at its terminal, as `ps` says it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Job {
    pid: u32,
    pgid: i64,
    /// Its terminal's foreground process group; none without a terminal.
    tpgid: i64,
    stopped: bool,
    /// Its terminal's device name, such as `ttys012`.
    tty: Option<String>,
}

/// How the worker's harness stands at its terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Holds {
    /// No process of the worker is on a terminal: no harness started yet,
    /// one gone, or one this host cannot see.
    Unseen,
    /// The harness is its terminal's foreground process group, running.
    Foreground { tty: String },
    /// The harness is alive on its terminal but not its foreground process
    /// group, or stopped.
    Background(Backgrounded),
}

impl Holds {
    /// The terminal device the harness is on, when it is on one.
    pub(crate) fn tty(&self) -> Option<&str> {
        match self {
            Holds::Unseen => None,
            Holds::Foreground { tty } => Some(tty),
            Holds::Background(held) => Some(&held.tty),
        }
    }
}

/// A harness alive on its terminal that does not hold it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Backgrounded {
    pub(crate) pid: u32,
    pgid: i64,
    /// Stopped (`T`): at its first read of a terminal it does not hold, or by
    /// a signal.
    pub(crate) stopped: bool,
    /// Its shell is the terminal's foreground process group: `fg` typed
    /// there brings the harness back.
    pub(crate) shell_holds: bool,
    pub(crate) tty: String,
}

impl Backgrounded {
    /// What was seen, in words a diagnosis and a log line carry.
    pub(crate) fn evidence(&self) -> String {
        format!(
            "the worker harness (pid {}) on {} is {} and not its terminal's foreground process \
             group: {}",
            self.pid,
            self.tty,
            if self.stopped { "stopped" } else { "running" },
            if self.shell_holds {
                "its shell holds the terminal"
            } else {
                "another process group holds the terminal"
            }
        )
    }
}

/// How the harness of the worker in `worktree` stands at its terminal, of
/// `procs`: every process with a terminal and every one above one, at least
/// (`hot::on_terminals`).
pub(crate) fn holds(procs: &[Proc], worktree: &Path) -> Result<Holds> {
    let tree = crate::hot::tree(procs, worktree);
    let ours: Vec<u32> = tree.iter().map(|proc| proc.pid).collect();
    // A process handed to pid 1 was started by no shell of a terminal.
    let roots: Vec<&Proc> =
        tree.into_iter().filter(|proc| proc.ppid != 1 && !ours.contains(&proc.ppid)).collect();
    if roots.is_empty() {
        return Ok(Holds::Unseen);
    }
    let mut asked: Vec<u32> = roots.iter().flat_map(|proc| [proc.pid, proc.ppid]).collect();
    asked.sort_unstable();
    asked.dedup();
    let jobs = jobs(&asked)?;
    let mut background = None;
    for root in roots {
        let Some(job) = jobs.iter().find(|job| job.pid == root.pid) else { continue };
        let Some(tty) = job.tty.clone() else { continue };
        if job.tpgid <= 0 {
            continue;
        }
        if job.pgid == job.tpgid && !job.stopped {
            return Ok(Holds::Foreground { tty });
        }
        let shell = jobs.iter().find(|shell| shell.pid == root.ppid);
        background.get_or_insert(Backgrounded {
            pid: job.pid,
            pgid: job.pgid,
            stopped: job.stopped,
            shell_holds: shell.is_some_and(|shell| shell.pgid == job.tpgid),
            tty,
        });
    }
    Ok(background.map_or(Holds::Unseen, Holds::Background))
}

/// What `ps` says of `pids` at their terminals. A pid gone is not listed.
fn jobs(pids: &[u32]) -> Result<Vec<Job>> {
    let list = pids.iter().map(u32::to_string).collect::<Vec<_>>().join(",");
    let out = crate::process::output(crate::census::command("ps").args([
        "-o",
        "pid=,pgid=,tpgid=,stat=,tty=",
        "-p",
        &list,
    ]))?;
    // ps exits non-zero when a pid asked is gone; what it listed still holds.
    Ok(String::from_utf8_lossy(&out.stdout).lines().filter_map(job).collect())
}

fn job(row: &str) -> Option<Job> {
    let mut fields = row.split_whitespace();
    let pid = fields.next()?.parse().ok()?;
    let pgid = fields.next()?.parse().ok()?;
    let tpgid = fields.next()?.parse().ok()?;
    let stopped = fields.next()?.starts_with('T');
    let tty = fields.next().filter(|tty| !tty.starts_with('?')).map(str::to_string);
    Some(Job { pid, pgid, tpgid, stopped, tty })
}

/// Brings `held` back to holding its terminal: its unread input dropped
/// first, so the shell runs none of it, then `fg` typed at its shell
/// (`type_fg`) where the shell holds the terminal, or a stopped harness that
/// holds it continued. `true` once the harness is its terminal's foreground
/// process group again, running, within a bound; `false` where it is not,
/// or nothing could bring it back: such a harness is to be restarted.
pub(crate) fn recover(
    worktree: &Path,
    held: &Backgrounded,
    type_fg: impl FnOnce() -> Result<()>,
) -> Result<bool> {
    if held.shell_holds {
        drop_unread(&held.tty);
        type_fg()?;
    } else if held.stopped {
        let group = format!("-{}", held.pgid);
        crate::process::output(crate::census::command("kill").args(["-CONT", "--", &group]))?;
    } else {
        return Ok(false);
    }
    let until = Instant::now() + RECOVER_WAIT;
    loop {
        if let Holds::Foreground { .. } = holds(&crate::hot::on_terminals()?, worktree)? {
            return Ok(true);
        }
        if Instant::now() >= until {
            return Ok(false);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Drops whatever was typed at the terminal `tty` and is not read yet, so no
/// shell reads it later as a command. Soft: a terminal that cannot be opened
/// is said on stderr.
pub(crate) fn drop_unread(tty: &str) {
    if let Err(error) = flush(tty) {
        eprintln!("warning: the unread input of {tty} was not dropped: {error:#}");
    }
}

fn flush(tty: &str) -> Result<()> {
    use std::os::unix::fs::OpenOptionsExt;
    use std::os::unix::io::AsRawFd;
    let device = Path::new("/dev").join(tty);
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOCTTY | libc::O_NONBLOCK)
        .open(&device)
        .with_context(|| format!("cannot open {}", device.display()))?;
    // SAFETY: the descriptor is open for as long as `file` is.
    let flushed = unsafe { libc::tcflush(file.as_raw_fd(), libc::TCIFLUSH) };
    anyhow::ensure!(
        flushed == 0,
        "tcflush {}: {}",
        device.display(),
        std::io::Error::last_os_error()
    );
    Ok(())
}

/// Whether the terminal `tty`, which a harness of the worker in `worktree`
/// was last seen on, is a bare shell: some process is on it, and none of
/// the worker's. A terminal nothing is on is gone, not bare.
pub(crate) fn bare_shell(procs: &[Proc], worktree: &Path, tty: &str) -> Result<bool> {
    let out = crate::process::output(crate::census::command("ps").args(["-o", "pid=", "-t", tty]))?;
    let on: Vec<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|row| row.trim().parse().ok())
        .collect();
    let ours = crate::hot::tree(procs, worktree);
    Ok(!on.is_empty() && !ours.iter().any(|proc| on.contains(&proc.pid)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::os::fd::{FromRawFd, OwnedFd};
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Stdio};
    use std::sync::{Arc, Mutex};

    #[test]
    fn a_row_of_ps_is_its_terminal_place() {
        assert_eq!(
            job("  412   412   400 T+   ttys012"),
            Some(Job {
                pid: 412,
                pgid: 412,
                tpgid: 400,
                stopped: true,
                tty: Some("ttys012".into())
            })
        );
        assert_eq!(
            job("9 9 0 Ss ??"),
            Some(Job { pid: 9, pgid: 9, tpgid: 0, stopped: false, tty: None })
        );
        assert_eq!(job("9 9 -1 S ?").unwrap().tty, None);
    }

    /// A login zsh on a terminal of its own, as Orca runs one, with what it
    /// printed read away so it never blocks on output.
    struct Terminal {
        master: std::fs::File,
        shell: Child,
        worktree: tempfile::TempDir,
        _home: tempfile::TempDir,
        output: Arc<Mutex<Vec<u8>>>,
    }

    impl Terminal {
        fn open() -> Option<Terminal> {
            let zsh = Path::new("/bin/zsh");
            if !zsh.exists() {
                eprintln!("skipped: no /bin/zsh on this host");
                return None;
            }
            let (mut master, mut slave) = (0, 0);
            // SAFETY: openpty writes two descriptors it opened; null names
            // and settings take the defaults.
            let opened = unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            };
            assert_eq!(opened, 0, "openpty: {}", std::io::Error::last_os_error());
            // SAFETY: both descriptors were just opened, and each is owned once.
            let (master, slave) =
                unsafe { (OwnedFd::from_raw_fd(master), OwnedFd::from_raw_fd(slave)) };
            let home = tempfile::tempdir().unwrap();
            let mut command = crate::census::command(zsh);
            command
                .args(["-l", "-i"])
                .env_clear()
                .env("HOME", home.path())
                .env("ZDOTDIR", home.path())
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("TERM", "dumb")
                .stdin(Stdio::from(slave.try_clone().unwrap()))
                .stdout(Stdio::from(slave.try_clone().unwrap()))
                .stderr(Stdio::from(slave));
            // SAFETY: between fork and exec this calls only setsid and ioctl,
            // both async-signal-safe.
            unsafe {
                command.pre_exec(|| {
                    if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            let shell = command.spawn().unwrap();
            let master = std::fs::File::from(master);
            let output = Arc::new(Mutex::new(Vec::new()));
            let mut reader = master.try_clone().unwrap();
            let read = Arc::clone(&output);
            std::thread::spawn(move || {
                let mut buffer = [0; 4096];
                while let Ok(n) = reader.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    read.lock().unwrap().extend_from_slice(&buffer[..n]);
                }
            });
            let worktree = tempfile::tempdir().unwrap();
            Some(Terminal { master, shell, worktree, _home: home, output })
        }

        /// Types `line` and Enter at the terminal.
        fn type_line(&self, line: &str) {
            (&self.master).write_all(format!("{line}\n").as_bytes()).unwrap();
        }

        /// Starts the stand-in harness doing `does` (`stand_in`), run by the
        /// shell as Orca's line runs a harness: with the worker's `TMPDIR`, in
        /// the foreground. It is this test program: macOS shows no other
        /// user's process the environment of a program of its own, `/bin/sh`.
        fn start_harness(&self, does: &str) -> u32 {
            let tmp = crate::worker_tmp::of(self.worktree.path());
            self.type_line(&format!(
                "cd '{}' && export TMPDIR='{}' && {STAND_IN}='{does}' '{}' --exact \
                 foreground::tests::stand_in --nocapture --test-threads 1",
                self.worktree.path().display(),
                tmp.display(),
                std::env::current_exe().unwrap().display()
            ));
            let held = self.until("the harness holds its terminal", |holds| {
                matches!(holds, Holds::Foreground { .. })
            });
            let procs = crate::hot::listed_checked().unwrap();
            let tree = crate::hot::tree(&procs, self.worktree.path());
            assert!(held.tty().is_some());
            tree.iter().find(|proc| proc.ppid == self.shell.id()).expect("the harness").pid
        }

        fn holds(&self) -> Holds {
            holds(&crate::hot::listed_checked().unwrap(), self.worktree.path()).unwrap()
        }

        fn until(&self, what: &str, done: impl Fn(&Holds) -> bool) -> Holds {
            let until = afkconvoy_patience::deadline();
            loop {
                let holds = self.holds();
                if done(&holds) {
                    return holds;
                }
                assert!(
                    Instant::now() < until,
                    "{what}: {holds:?}; screen: {}",
                    String::from_utf8_lossy(&self.output.lock().unwrap())
                );
                std::thread::sleep(Duration::from_millis(50));
            }
        }

        /// Waits until `file` exists, or the one bound on a wait for an event
        /// has gone; whether it does.
        fn appears(file: &Path) -> bool {
            let until = afkconvoy_patience::deadline();
            while Instant::now() < until {
                if file.exists() {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
            file.exists()
        }
    }

    impl Drop for Terminal {
        fn drop(&mut self) {
            if let Ok(procs) = crate::hot::listed_checked() {
                for proc in crate::hot::tree(&procs, self.worktree.path()) {
                    signal(proc.pid, libc::SIGKILL);
                }
            }
            let _ = self.shell.kill();
            let _ = self.shell.wait();
        }
    }

    const STAND_IN: &str = "AFKCONVOY_FOREGROUND_STAND_IN";

    /// The stand-in harness, run only under `STAND_IN`: `read:<file>` reads
    /// its terminal, a line at a time, onto the end of the file; `sleep`
    /// reads nothing for half a minute.
    #[test]
    fn stand_in() {
        let Ok(does) = std::env::var(STAND_IN) else { return };
        if let Some(file) = does.strip_prefix("read:") {
            for line in std::io::stdin().lines() {
                let mut said =
                    std::fs::OpenOptions::new().create(true).append(true).open(file).unwrap();
                writeln!(said, "{}", line.unwrap()).unwrap();
            }
        } else {
            std::thread::sleep(Duration::from_secs(30));
        }
    }

    fn signal(pid: u32, signal: libc::c_int) {
        // SAFETY: the pid is this test's own process.
        unsafe { libc::kill(pid as libc::pid_t, signal) };
    }

    #[test]
    fn a_harness_stopped_and_continued_under_a_login_zsh_is_backgrounded_and_brought_back() {
        let Some(terminal) = Terminal::open() else { return };
        let said = terminal.worktree.path().join("said");
        let harness = terminal.start_harness(&format!("read:{}", said.display()));
        signal(harness, libc::SIGSTOP);
        terminal.until(
            "the shell takes its terminal back",
            |holds| matches!(holds, Holds::Background(held) if held.shell_holds),
        );
        signal(harness, libc::SIGCONT);
        // Continued in the background, its first read stops it again.
        let held = terminal.until(
            "the harness is resumed in the background",
            |holds| matches!(holds, Holds::Background(held) if held.stopped && held.shell_holds),
        );
        let Holds::Background(held) = held else { unreachable!() };
        assert_eq!(held.pid, harness);
        assert!(held.evidence().contains("not its terminal's foreground"), "{}", held.evidence());

        let fg = || {
            terminal.type_line("fg");
            Ok(())
        };
        let recovered = recover(terminal.worktree.path(), &held, fg).unwrap();
        assert!(recovered, "fg brings it back; screen: {:?}", terminal.output);
        assert!(matches!(terminal.holds(), Holds::Foreground { .. }));
        terminal.type_line("heard");
        assert!(Terminal::appears(&said), "it reads its terminal again");
        assert_eq!(std::fs::read_to_string(&said).unwrap(), "heard\n");
    }

    #[test]
    fn a_harness_whose_shell_does_not_hold_its_terminal_is_not_brought_back_by_typing() {
        let Some(terminal) = Terminal::open() else { return };
        let harness = terminal.start_harness("sleep");
        let held = Backgrounded {
            pid: harness,
            pgid: harness as i64,
            stopped: false,
            shell_holds: false,
            tty: terminal.holds().tty().unwrap().to_string(),
        };
        let typed = recover(terminal.worktree.path(), &held, || panic!("nothing is typed"));
        assert!(!typed.unwrap(), "a running harness nothing can bring back is to be restarted");
    }

    #[test]
    fn an_undelivered_line_leaves_nothing_a_shell_reads_after_the_harness_exits() {
        let Some(terminal) = Terminal::open() else { return };
        let tty = |terminal: &Terminal| terminal.holds().tty().unwrap().to_string();

        // Typed at a harness that reads none of it, a line waits for the shell.
        let ran = terminal.worktree.path().join("ran");
        let harness = terminal.start_harness("sleep");
        terminal.type_line(&format!("touch {}", ran.display()));
        signal(harness, libc::SIGKILL);
        assert!(Terminal::appears(&ran), "the shell ran the unread line");

        // Dropped once it is not confirmed delivered, nothing is left to run.
        let dropped = terminal.worktree.path().join("dropped");
        let harness = terminal.start_harness("sleep");
        terminal.type_line(&format!("touch {}", dropped.display()));
        drop_unread(&tty(&terminal));
        signal(harness, libc::SIGKILL);
        let marker = terminal.worktree.path().join("shell-reads");
        terminal.type_line(&format!("touch {}", marker.display()));
        assert!(Terminal::appears(&marker), "the shell reads again");
        assert!(!dropped.exists(), "the dropped line never ran");
    }

    #[test]
    fn a_terminal_its_harness_left_is_a_bare_shell() {
        let Some(terminal) = Terminal::open() else { return };
        let harness = terminal.start_harness("sleep");
        let tty = terminal.holds().tty().unwrap().to_string();
        let procs = crate::hot::listed_checked().unwrap();
        assert!(!bare_shell(&procs, terminal.worktree.path(), &tty).unwrap());
        signal(harness, libc::SIGKILL);
        terminal.until("the harness is gone", |holds| *holds == Holds::Unseen);
        let procs = crate::hot::listed_checked().unwrap();
        assert!(bare_shell(&procs, terminal.worktree.path(), &tty).unwrap());
    }
}
