//! Kernel-held worker capacity survives owner and host-runner death. A gate
//! inherits its descriptor, so no replacement can consume that slot until
//! every gate descendant exits. The JSON is recovery input only; flock is
//! the authority on whether the capacity is still occupied.
use std::fs::{File, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::process::CommandExt;
use std::process::Command;
use std::time::Duration;

use afkconvoy_core::exclusive::Ticket;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Owner {
    process: afkconvoy_core::candidate::Process,
    dir: String,
    candidate: bool,
}

pub(crate) struct Slot(File, Option<File>);

fn repo_file(ticket: &Ticket) -> Result<Option<File>> {
    let Some(repo) = &ticket.repo else {
        return Ok(None);
    };
    let cache = crate::copies::cache()?.join("worker-slots");
    std::fs::create_dir_all(&cache)?;
    let key = afkconvoy_core::gate::key(repo);
    Ok(Some(
        OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(cache.join(format!("repo-{key:016x}")))?,
    ))
}

fn try_repo(file: &File, ticket: &Ticket) -> Result<bool> {
    let mode = if ticket.candidate { libc::LOCK_SH } else { libc::LOCK_EX };
    // SAFETY: lock only this live descriptor, without waiting.
    if unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    ensure!(
        std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock,
        "cannot inspect repo command ownership"
    );
    Ok(false)
}

/// A replacement runner may have no ticket for a still-running gate. Its
/// inherited repo lease retains ordinary-command exclusion independently.
pub(crate) fn repo_available(ticket: &Ticket) -> Result<bool> {
    match repo_file(ticket)? {
        Some(file) => try_repo(&file, ticket),
        None => Ok(true),
    }
}

impl Slot {
    pub fn take(ticket: &Ticket) -> Result<Self> {
        let repo = repo_file(ticket)?;
        if let Some(file) = &repo {
            while !try_repo(file, ticket)? {
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let cache = crate::copies::cache()?.join("worker-slots");
        std::fs::create_dir_all(&cache)?;
        let lane = afkconvoy_core::gate::key(&afkconvoy_core::exclusive::lane(ticket));
        loop {
            let count = if ticket.repo_lane.is_some() {
                1
            } else {
                crate::host_settings::of_host()?.gate_lanes()
            };
            for n in 0..count {
                let path = cache.join(format!("{lane:016x}-{n}"));
                let mut file = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(&path)?;
                // SAFETY: a live descriptor; a nonblocking kernel lease.
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    file.set_len(0)?;
                    file.seek(SeekFrom::Start(0))?;
                    let owner = Owner {
                        process: crate::candidates::identity(std::process::id())?
                            .ok_or_else(|| anyhow::anyhow!("cannot identify slot owner"))?,
                        dir: ticket
                            .repo
                            .as_deref()
                            .and_then(|path| std::path::Path::new(path).parent())
                            .map_or_else(
                                || ticket.dir.clone(),
                                |path| path.to_string_lossy().into_owned(),
                            ),
                        candidate: ticket.candidate,
                    };
                    serde_json::to_writer(&mut file, &owner)?;
                    file.flush()?;
                    return Ok(Self(file, repo));
                }
                ensure!(
                    std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock,
                    "cannot inspect kernel worker slot"
                );
                // A dead owner cannot silently free an inherited lease. Ask
                // its repo to invalidate and confirm the recorded gate's exit.
                if let Ok(bytes) = std::fs::read(&path)
                    && let Ok(owner) = serde_json::from_slice::<Owner>(&bytes)
                    && owner.candidate
                    && owner.process.host == crate::this_host()
                    && crate::candidates::identity(owner.process.pid)?.as_ref()
                        != Some(&owner.process)
                {
                    crate::candidates::abandoned(&owner.dir)?;
                }
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// Only the owned child receives the descriptor. Other subprocesses
    /// retain close-on-exec, including concurrent git and ps observations.
    pub fn inherit(&self, command: &mut Command) {
        let fds = [self.0.as_raw_fd(), self.1.as_ref().map_or(-1, AsRawFd::as_raw_fd)];
        // SAFETY: the child only clears one descriptor flag before exec.
        unsafe {
            command.pre_exec(move || {
                for fd in fds {
                    if fd >= 0 && libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                        return Err(std::io::Error::last_os_error());
                    }
                }
                Ok(())
            });
        }
    }
}
