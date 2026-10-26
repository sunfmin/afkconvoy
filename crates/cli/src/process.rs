//! Subprocess starts share one boundary: on macOS a pipe is created before
//! its close-on-exec flags are set. A concurrent start in that window can
//! inherit it, holding another command's captured output open until it exits.
//! Only starting is held in turn; children run and are waited for together.

use std::io;
use std::process::{Child, Command, ExitStatus, Output, Stdio};

/// Starts `command` with its configured streams, without inheriting a pipe
/// another start is still preparing. Waiting for the child holds no turn.
pub(crate) fn spawn(command: &mut Command) -> io::Result<Child> {
    starting(|| command.spawn())
}

/// Runs `command` with no input, capturing both output streams. Every caller
/// here wants these streams; commands with other streams use `spawn`.
pub(crate) fn output(command: &mut Command) -> io::Result<Output> {
    let child = starting(|| {
        command.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn()
    })?;
    child.wait_with_output()
}

/// Runs `command` with its configured streams, inheriting them by default.
pub(crate) fn status(command: &mut Command) -> io::Result<ExitStatus> {
    spawn(command)?.wait()
}

fn starting<T>(start: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    start()
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::sync::mpsc;
    use std::time::Duration;

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
                    let ran = status(Command::new("sh").args([
                        "-c", "test ! -e /dev/fd/\"$1\"", "sh", &fd,
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
                    assert_eq!(unsafe { libc::fcntl(pipe.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) }, 0);
                }
                Ok((pipes, child))
            }).unwrap();
            let (_pipes, child) = child;
            assert!(child.join().unwrap().unwrap().success(), "the child inherited another command's pipe");
        });
    }
}
