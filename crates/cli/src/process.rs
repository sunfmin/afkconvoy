//! Subprocess starts share one boundary: on macOS a pipe is created before
//! its close-on-exec flags are set. A concurrent start in that window can
//! inherit it, holding another command's captured output open until it exits.
//! Only starting is held in turn; children run and are waited for together.

use std::io::{self, Read};
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Has `command` start with no descriptor it would inherit but stdin,
/// stdout and stderr. A process that outlives its starter — the host
/// runner — would otherwise hold whatever its starter was handed: a worker
/// lane's lease, kept by its kernel lock, held the lane for as long as a
/// host runner a test started lived (2026-10-29). Descriptors marked
/// close-on-exec are left to exec, and Rust's own pipe among them.
pub(crate) fn inheriting_nothing(command: &mut Command) -> &mut Command {
    use std::os::unix::process::CommandExt;
    // SAFETY: getdtablesize and fcntl and close are async-signal-safe.
    let limit = unsafe { libc::getdtablesize() };
    unsafe {
        command.pre_exec(move || {
            for fd in 3..limit {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 && flags & libc::FD_CLOEXEC == 0 {
                    libc::close(fd);
                }
            }
            Ok(())
        })
    }
}

/// This process's descriptors past stderr that a child would inherit: those
/// it was handed itself and has not marked close-on-exec. In an afkconvoy a
/// gate started, one of them is the gate's process scope (`process_scope`).
fn inherited() -> Vec<i32> {
    let listing = if cfg!(target_os = "linux") { "/proc/self/fd" } else { "/dev/fd" };
    let Ok(entries) = std::fs::read_dir(listing) else { return vec![] };
    entries
        .flatten()
        .filter_map(|entry| entry.file_name().to_str()?.parse::<i32>().ok())
        .filter(|&fd| {
            // SAFETY: F_GETFD only reads the descriptor's flags.
            let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
            fd > 2 && flags >= 0 && flags & libc::FD_CLOEXEC == 0
        })
        .collect()
}

/// The descriptors this process was handed and would pass on (`inherited`),
/// marked close-on-exec until it is dropped, then passed on again: what is
/// started meanwhile — afkconvoy's own `ps`, `git` and `gh` — holds no gate's
/// process scope or worker lane. A setuid `ps` an afkconvoy inside a gate
/// started held its scope, which no kill of its user could end, and so every
/// lane of the host, for four hours (sunfmin/control#1096).
///
/// The flags are set in this process, not closed in the child: a child
/// prepared by a hook of its own (`pre_exec`) is started with `fork`, and one
/// without by `posix_spawn`, which on macOS took 1.4 to 1.9 ms more wall and
/// 0.6 ms more CPU a start (sunfmin/control#1229). Inside a gate every
/// afkconvoy is handed its lane's lease, so each of its looks paid that.
/// Held only while starting is (`starting`): no other start of this process
/// sees the flags it sets.
struct Withheld(Vec<i32>);

impl Withheld {
    fn inherited() -> Self {
        let fds = inherited();
        for &fd in &fds {
            // SAFETY: the descriptor was open and inheritable as listed; only
            // its close-on-exec flag is set.
            unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) };
        }
        Withheld(fds)
    }
}

impl Drop for Withheld {
    fn drop(&mut self) {
        for &fd in &self.0 {
            // SAFETY: as above, only the flag set in `inherited` is cleared.
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC);
                }
            }
        }
    }
}

/// Starts `command` with its configured streams, without inheriting a pipe
/// another start is still preparing. Waiting for the child holds no turn.
pub(crate) fn spawn(command: &mut Command) -> io::Result<Child> {
    starting(|| command.spawn())
}

/// One pipe for a child's stdout and stderr, created while starting is held
/// so no other start can inherit it before its close-on-exec flags are set.
/// The two `Stdio`s share the write end; the caller reads the read end.
pub(crate) fn merged_stdio() -> io::Result<(Stdio, Stdio, OwnedFd)> {
    starting(|| {
        let mut fds = [0; 2];
        // SAFETY: pipe() writes two fresh descriptors into `fds`.
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: both descriptors came from pipe() and are open.
        let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
        let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
        for fd in [read.as_raw_fd(), write.as_raw_fd()] {
            // SAFETY: the descriptor is open. CLOEXEC keeps it out of a
            // later start once this turn ends.
            if unsafe { libc::fcntl(fd, libc::F_SETFD, libc::FD_CLOEXEC) } != 0 {
                return Err(io::Error::last_os_error());
            }
        }
        let stdout = Stdio::from(write.try_clone()?);
        let stderr = Stdio::from(write);
        Ok((stdout, stderr, read))
    })
}

/// How long a pipe is drained after the direct child has exited. A grandchild
/// that inherited the write end would otherwise keep `wait_with_output`
/// blocked after the child is gone.
const DRAIN_AFTER_EXIT: Duration = Duration::from_secs(2);

/// Runs `command` with no input, capturing both output streams. Every caller
/// here wants these streams; commands with other streams use `spawn`. It
/// inherits no descriptor this process was handed (`Withheld`):
/// what is run for its output is afkconvoy's own look, never a gate's work.
///
/// The pipes are read while the child runs, so a full pipe cannot stall it.
/// Once it has exited, a write end that stays open is drained for
/// `DRAIN_AFTER_EXIT` and then dropped: the output already written is kept,
/// and a leaked writer cannot hold the caller.
pub(crate) fn output(command: &mut Command) -> io::Result<Output> {
    output_given(command, None)
}

/// As `output`, with `input` written to the command's stdin and then closed:
/// for text too long for an argument, which the OS bounds by `ARG_MAX`.
pub(crate) fn output_with_input(command: &mut Command, input: &[u8]) -> io::Result<Output> {
    output_given(command, Some(input))
}

/// As `output`, with whether the command outlived `limit`. Its output is kept
/// either way, so a caller can report what a timed-out command had written.
pub(crate) fn output_within_with_timeout(
    command: &mut Command,
    limit: Duration,
) -> io::Result<(Output, bool)> {
    use std::os::unix::process::CommandExt;
    output_bounded(command.process_group(0), None, limit)
}

fn output_given(command: &mut Command, input: Option<&[u8]>) -> io::Result<Output> {
    output_bounded(command, input, None).map(|(out, _)| out)
}

fn output_bounded(
    command: &mut Command,
    input: Option<&[u8]>,
    limit: impl Into<Option<Duration>>,
) -> io::Result<(Output, bool)> {
    let limit = limit.into().map(crate::waits::bound);
    let stdin = if input.is_some() { Stdio::piped() } else { Stdio::null() };
    let mut child = starting(|| {
        let _withheld = Withheld::inherited();
        command.stdin(stdin).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
    })?;
    let writer = child.stdin.take();
    let stdout = child.stdout.take().ok_or_else(|| io::Error::other("stdout was not piped"))?;
    let stderr = child.stderr.take().ok_or_else(|| io::Error::other("stderr was not piped"))?;
    let exited = AtomicBool::new(false);
    let (stdout, stderr, status, timed_out) = std::thread::scope(|scope| {
        if let (Some(mut writer), Some(input)) = (writer, input) {
            // A child that exits before reading it all closes the pipe; its
            // status says what went wrong, not the broken write.
            scope.spawn(move || {
                use std::io::Write;
                let _ = writer.write_all(input);
            });
        }
        let stdout = scope.spawn(|| read_bounded(stdout, &exited));
        let stderr = scope.spawn(|| read_bounded(stderr, &exited));
        let started = Instant::now();
        let mut timed_out = false;
        let status = if let Some(limit) = limit {
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => break Ok(status),
                    Ok(None) if started.elapsed() >= limit => {
                        timed_out = true;
                        let pid = child.id();
                        if pid > 0 {
                            // SAFETY: this child leads the dedicated group made above.
                            unsafe { libc::kill(-(pid as i32), libc::SIGKILL) };
                        }
                        let _ = child.kill();
                        break child.wait();
                    }
                    Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                    Err(error) => break Err(error),
                }
            }
        } else {
            child.wait()
        };
        exited.store(true, Ordering::Release);
        let stdout =
            stdout.join().unwrap_or_else(|_| Err(io::Error::other("stdout drain panicked")));
        let stderr =
            stderr.join().unwrap_or_else(|_| Err(io::Error::other("stderr drain panicked")));
        (stdout, stderr, status, timed_out)
    });
    let out = Output { status: status?, stdout: stdout?, stderr: stderr? };
    Ok((out, timed_out))
}

fn set_nonblock(fd: i32) -> io::Result<()> {
    // SAFETY: the descriptor is open. O_NONBLOCK is flipped on that descriptor alone.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: the same open descriptor. The new flags keep every bit fcntl returned.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn poll_in(fd: i32, timeout_ms: i32) -> io::Result<bool> {
    let mut fds = [libc::pollfd { fd, events: libc::POLLIN, revents: 0 }];
    loop {
        // SAFETY: `fds` is one open descriptor and a caller-owned pollfd.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), 1, timeout_ms) };
        if ready < 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(error);
        }
        return Ok(ready > 0);
    }
}

/// Reads `file` until EOF. After `exited` is set, a writer that never closes
/// the pipe — a grandchild that inherited it — ends the read at
/// `DRAIN_AFTER_EXIT` instead of holding the caller.
pub(crate) fn read_bounded(
    mut file: impl Read + AsRawFd,
    exited: &AtomicBool,
) -> io::Result<Vec<u8>> {
    set_nonblock(file.as_raw_fd())?;
    let mut buf = [0u8; 8192];
    let mut got = Vec::new();
    let mut deadline = None;
    loop {
        if let Some(end) = deadline
            && Instant::now() >= end
        {
            return Ok(got);
        }
        match file.read(&mut buf) {
            Ok(0) => return Ok(got),
            Ok(n) => {
                got.extend_from_slice(&buf[..n]);
                if exited.load(Ordering::Acquire) {
                    deadline.get_or_insert_with(|| Instant::now() + DRAIN_AFTER_EXIT);
                }
            }
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                if !exited.load(Ordering::Acquire) {
                    poll_in(file.as_raw_fd(), 200)?;
                    continue;
                }
                let end = *deadline.get_or_insert_with(|| Instant::now() + DRAIN_AFTER_EXIT);
                let left = end.saturating_duration_since(Instant::now());
                if left.is_zero() {
                    return Ok(got);
                }
                let slice = left.min(Duration::from_millis(50)).as_millis() as i32;
                poll_in(file.as_raw_fd(), slice)?;
            }
            Err(error) => return Err(error),
        }
    }
}

/// Runs `command` with its configured streams, inheriting them by default.
pub(crate) fn status(command: &mut Command) -> io::Result<ExitStatus> {
    spawn(command)?.wait()
}

fn starting<T>(start: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    // macOS has no pipe2(O_CLOEXEC): cover pipe creation and flag setting
    // against every other start, including one capturing no output itself.
    // On every system, a start of the gate's own work is not to find the
    // descriptors it passes on withheld by another's (`Withheld`).
    static STARTING: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _starting = STARTING.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    start()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::ffi::CString;
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::mpsc;
    use std::time::{Duration, Instant};

    #[test]
    fn input_past_arg_max_reaches_the_command_whole() {
        // macOS's ARG_MAX is 1 MiB; a candidate record was refused there
        // as an argument (sunfmin/control#308).
        let input = "x".repeat(2 << 20);
        let out =
            output_with_input(crate::census::command("wc").arg("-c"), input.as_bytes()).unwrap();
        assert!(out.status.success());
        assert_eq!(String::from_utf8(out.stdout).unwrap().trim(), (2 << 20).to_string());
    }

    #[test]
    fn a_start_inheriting_nothing_is_handed_no_descriptor_its_starter_was() {
        let dir = tempfile::tempdir().unwrap();
        let lease = std::fs::File::create(dir.path().join("lease")).unwrap();
        let fd = lease.as_raw_fd();
        // As a worker's lease is handed on: inheritable.
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, 0);
        let open = |command: &mut Command| {
            let probe = format!("[ -e /dev/fd/{fd} ] && echo open || echo closed");
            let out = command.args(["-c", &probe]).output().unwrap();
            String::from_utf8(out.stdout).unwrap().trim().to_string()
        };
        assert_eq!(open(&mut crate::census::command("sh")), "open");
        assert_eq!(open(inheriting_nothing(&mut crate::census::command("sh"))), "closed");
    }

    #[test]
    fn waiting_for_one_child_does_not_hold_back_another_start() {
        let dir = tempfile::tempdir().unwrap();
        let fifo = dir.path().join("release");
        let ready = dir.path().join("ready");
        let name = CString::new(fifo.as_os_str().as_encoded_bytes()).unwrap();
        // SAFETY: name is a nul-terminated path in this test's directory.
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        std::thread::scope(|scope| {
            let first = scope.spawn(|| {
                output(crate::census::command("sh").args([
                    "-c",
                    "printf ready > \"$1\"; read -r line < \"$2\"; printf first",
                    "sh",
                    ready.to_str().unwrap(),
                    fifo.to_str().unwrap(),
                ]))
                .unwrap()
            });
            let until = Instant::now() + Duration::from_secs(10);
            while !ready.exists() {
                assert!(Instant::now() < until, "the first child never started");
                std::thread::sleep(Duration::from_millis(10));
            }
            let (sent, received) = mpsc::channel();
            scope.spawn(move || {
                let second = output(crate::census::command("sh").args(["-c", "printf second"]));
                let _ = sent.send(second);
            });
            let second = received.recv_timeout(Duration::from_secs(5));
            // Release the first even if the second was held back: a wrong
            // lock scope fails this test instead of hanging its cleanup.
            std::fs::File::create(&fifo).unwrap().write_all(b"go\n").unwrap();
            let first = first.join().unwrap();
            let second = second.expect("waiting for output held back another start").unwrap();
            assert!(first.status.success() && second.status.success());
            assert_eq!(first.stdout, b"first");
            assert_eq!(second.stdout, b"second");
        });
    }

    #[test]
    fn a_grandchild_holding_stdout_does_not_keep_output_waiting() {
        let dir = tempfile::tempdir().unwrap();
        let pidfile = dir.path().join("holder");
        let script = format!("sleep 30 & echo $! > '{}'; printf done", pidfile.display());
        let started = Instant::now();
        let out = output(crate::census::command("sh").args(["-c", &script])).unwrap();
        if let Ok(pid) = std::fs::read_to_string(&pidfile) {
            let _ = status(crate::census::command("kill").arg(pid.trim()));
        }
        assert!(out.status.success());
        assert_eq!(out.stdout, b"done");
        assert!(
            started.elapsed() < Duration::from_secs(8),
            "output waited {:?} after the child exited",
            started.elapsed()
        );
    }

    #[test]
    fn a_child_cannot_inherit_a_pipe_while_its_close_on_exec_flags_are_being_set() {
        std::thread::scope(|scope| {
            let child = starting(|| {
                // Pin the window inside std's macOS pipe setup: pipe(),
                // then fcntl(F_SETFD, FD_CLOEXEC), separately for each end.
                let mut fds = [-1; 2];
                // SAFETY: pipe writes two owned descriptors into this array.
                assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0);
                // SAFETY: pipe succeeded, and these take ownership once.
                let pipes = unsafe { fds.map(|fd| OwnedFd::from_raw_fd(fd)) };
                let fd = pipes[1].as_raw_fd().to_string();
                let (attempting, attempted) = mpsc::channel();
                let (finished, finishing) = mpsc::channel();
                let child = scope.spawn(move || {
                    attempting.send(()).unwrap();
                    let ran = status(crate::census::command("sh").args([
                        "-c",
                        "test ! -e /dev/fd/\"$1\"",
                        "sh",
                        &fd,
                    ]));
                    let _ = finished.send(());
                    ran
                });
                attempted.recv().unwrap();
                // With an unprotected start the child observes the open
                // descriptor and ends here. A protected one waits for the
                // flags, without this test relying on a lucky interleaving.
                let _ = finishing.recv_timeout(Duration::from_millis(250));
                for pipe in &pipes {
                    // SAFETY: each descriptor is still owned by pipes.
                    assert_eq!(
                        unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) },
                        0
                    );
                }
                Ok((pipes, child))
            })
            .unwrap();
            let (_pipes, child) = child;
            assert!(
                child.join().unwrap().unwrap().success(),
                "the child inherited another command's pipe"
            );
        });
    }
}

#[cfg(test)]
mod inherited {
    use std::os::fd::AsRawFd;

    /// A descriptor this process was handed and would pass on — a gate's
    /// process scope, in an afkconvoy a gate started — is not inherited by
    /// what it runs for its output, as afkconvoy's own `ps` is
    /// (sunfmin/control#1096).
    #[test]
    fn what_is_run_for_its_output_inherits_no_descriptor_this_process_was_handed() {
        let dir = tempfile::tempdir().unwrap();
        let file = std::fs::File::create(dir.path().join("w.processes")).unwrap();
        let fd = file.as_raw_fd();
        // SAFETY: clears close-on-exec of this test's own open descriptor.
        assert_eq!(unsafe { libc::fcntl(fd, libc::F_SETFD, 0) }, 0);
        let probe = format!("if [ -e /dev/fd/{fd} ]; then echo held; else echo closed; fi");
        let out = super::output(crate::census::command("sh").args(["-c", &probe])).unwrap();
        let mut spawned = super::spawn(
            crate::census::command("sh").args(["-c", &probe]).stdout(std::process::Stdio::piped()),
        )
        .unwrap();
        let mut passed = String::new();
        std::io::Read::read_to_string(&mut spawned.stdout.take().unwrap(), &mut passed).unwrap();
        spawned.wait().unwrap();
        drop(file);
        assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "closed");
        assert_eq!(passed.trim(), "held", "a start of the gate's own work still passes it on");
    }
}
