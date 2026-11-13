//! Kernel-held worker capacity survives owner and host-runner death. A gate
//! inherits its descriptor, so no replacement can consume that slot until
//! every gate descendant exits. The JSON is recovery input only; flock is
//! the authority on whether the capacity is still occupied — but for a lock
//! its owner left behind (`left_behind`): one that only a server shared by
//! the host still holds, as sccache's server, started by a gate's build,
//! inherits the descriptor and serves every worktree long after the gate
//! ended (sunfmin/control#822). Such a slot is free.
use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::Command;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant};

use afkconvoy_core::exclusive::Ticket;
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
struct Owner {
    process: afkconvoy_core::candidate::Process,
    dir: String,
    candidate: bool,
    #[serde(default)]
    ticket: Option<Ticket>,
}

/// Kernel-held slots are visible after runner restart, including green
/// candidates waiting to publish. No file or process is changed by this read.
pub(crate) fn standing() -> Result<Vec<Ticket>> {
    let cache = crate::copies::cache()?.join("worker-slots");
    if !cache.exists() {
        return Ok(vec![]);
    }
    let mut tickets = vec![];
    for entry in std::fs::read_dir(cache)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("repo-") {
            continue;
        }
        let file = OpenOptions::new().read(true).write(true).open(entry.path())?;
        // SAFETY: nonblocking inspection; success means no inherited owner.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            continue;
        }
        ensure!(
            std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock,
            "cannot observe inherited worker lease"
        );
        let owner: Owner = serde_json::from_slice(&read(&file)?)?;
        if left_behind(&file, &owner) {
            continue;
        }
        let mut ticket = owner.ticket.ok_or_else(|| {
            anyhow::anyhow!(
                "an inherited lease has no queue metadata; wait for its owning command to end"
            )
        })?;
        ticket.number = u64::MAX - u64::from(owner.process.pid);
        tickets.push(ticket);
    }
    Ok(tickets)
}

pub(crate) struct Slot(File, Option<File>);

/// Discover dead candidate owners before repo exclusion can hold a new
/// ordinary command outside the capacity-acquisition path. Kernel locks
/// distinguish live inherited leases from stale recovery notes. A slot its
/// owner left behind is freed (`free_left_behind`).
pub(crate) fn recover() -> Result<()> {
    let cache = crate::copies::cache()?.join("worker-slots");
    if !cache.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(cache)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().starts_with("repo-") {
            continue;
        }
        let file = OpenOptions::new().read(true).write(true).open(entry.path())?;
        // SAFETY: inspect this slot without waiting or assuming owner death.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
            continue;
        }
        ensure!(
            std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock,
            "cannot inspect inherited candidate lease"
        );
        if let Ok(bytes) = read(&file)
            && let Ok(owner) = serde_json::from_slice::<Owner>(&bytes)
            && owner.candidate
            && dead(&owner)?
        {
            crate::candidates::abandoned(&owner.dir)?;
        }
        free_left_behind(&entry.path(), &file)?;
    }
    Ok(())
}

/// Whether the process that took the slot `owner` names, on this host, ended.
fn dead(owner: &Owner) -> Result<bool> {
    Ok(owner.process.host == crate::this_host()
        && crate::candidates::identity(owner.process.pid)?.as_ref() != Some(&owner.process))
}

/// What the slot file open as `file` says, read from its start.
fn read(mut file: &File) -> std::io::Result<Vec<u8>> {
    let mut bytes = vec![];
    file.seek(SeekFrom::Start(0))?;
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

/// How long a look at what holds a dead owner's slot stands: a waiting turn
/// reads the slots every 100 ms, and each look reads the process table.
const LOOKED_FOR: Duration = Duration::from_secs(2);

/// Each dead owner's slot looked at, by its file and owner, as when and what
/// was found.
static LOOKED: Mutex<BTreeMap<Looked, (Instant, bool)>> = Mutex::new(BTreeMap::new());

/// A slot's file, by its device and inode, and the pid of its owner.
type Looked = (u64, u64, u32);

/// Whether the slot open as `file`, whose lock is held, was left behind by
/// `owner`: its owner ended, and every process still holding the file is a
/// server shared by the host or under one (`only_shared`). One whose gate's
/// own work runs on — a test or a build whose gate was killed — is held
/// until it ends (docs/adr/0037). What cannot be looked at is held.
fn left_behind(file: &File, owner: &Owner) -> bool {
    if !dead(owner).unwrap_or(false) {
        return false;
    }
    let Ok(meta) = file.metadata() else { return false };
    let key = (meta.dev(), meta.ino(), owner.process.pid);
    let mut looked = LOOKED.lock().unwrap_or_else(PoisonError::into_inner);
    looked.retain(|_, (at, _)| at.elapsed() < LOOKED_FOR);
    if let Some((_, found)) = looked.get(&key) {
        return *found;
    }
    let worktree = owner.ticket.as_ref().map_or(owner.dir.as_str(), |ticket| ticket.dir.as_str());
    let worktree = std::fs::canonicalize(worktree).unwrap_or_else(|_| worktree.into());
    let found = holders(file)
        .and_then(|holders| Ok(only_shared(&holders, &crate::hot::listed_checked()?, &worktree)))
        .unwrap_or(false);
    looked.insert(key, (Instant::now(), found));
    found
}

/// The processes but this one that hold the file open as `file`, as `lsof`
/// finds them by its path. Errors: they cannot be listed.
fn holders(file: &File) -> Result<Vec<u32>> {
    let path = path_of(file)?;
    let out = crate::process::output(Command::new("lsof").args(["-w", "-t", "--"]).arg(&path))?;
    let pids: Vec<u32> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| l.trim().parse().ok())
        .collect();
    // `lsof` says it found none by its exit status alone.
    ensure!(
        out.status.success() || (pids.is_empty() && out.stderr.is_empty()),
        "cannot list what holds {}: {}",
        path.display(),
        String::from_utf8_lossy(&out.stderr).trim()
    );
    let own = std::process::id();
    Ok(pids.into_iter().filter(|pid| *pid != own).collect())
}

/// The path the kernel names the file open as `file` by.
#[cfg(target_os = "macos")]
fn path_of(file: &File) -> Result<std::path::PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let mut buf = [0u8; libc::PATH_MAX as usize];
    // SAFETY: a live descriptor and a buffer of PATH_MAX bytes, as F_GETPATH asks.
    let got = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETPATH, buf.as_mut_ptr()) };
    ensure!(got != -1, "cannot name a worker slot: {}", std::io::Error::last_os_error());
    let end = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    Ok(std::ffi::OsStr::from_bytes(&buf[..end]).into())
}

#[cfg(not(target_os = "macos"))]
fn path_of(file: &File) -> Result<std::path::PathBuf> {
    Ok(std::fs::read_link(format!("/proc/self/fd/{}", file.as_raw_fd()))?)
}

/// Whether each of `holders`, of the process table `procs`, is a server
/// shared by the host — handed to pid 1, working outside `worktree`
/// (`hot::shared`) — or under one, as the compilers sccache's server starts
/// are: none is the work of the gate run in `worktree`.
fn only_shared(holders: &[u32], procs: &[crate::hot::Proc], worktree: &Path) -> bool {
    let servers: Vec<u32> =
        procs.iter().filter(|p| crate::hot::shared(p, worktree)).map(|p| p.pid).collect();
    let table: Vec<(u32, u32)> = procs.iter().map(|p| (p.pid, p.ppid)).collect();
    let theirs = afkconvoy_core::exclusive::under(&servers, &table);
    holders.iter().all(|pid| theirs.contains(pid))
}

/// Frees the slot at `path`, open as `file` and locked by another, where its
/// owner left it behind (`left_behind`) → whether it did. A candidate's dead
/// owner is cleaned up first (`candidates::abandoned`). The file is taken
/// off its path: its lock stays with what holds it, on a file nothing opens
/// any more, and the next to open the path makes a new one. One frees at a
/// time, under the lock of the slots' directory, and only the very file it
/// read: one another made there meanwhile is kept.
fn free_left_behind(path: &Path, file: &File) -> Result<bool> {
    let Some(owner) =
        read(file).ok().and_then(|bytes| serde_json::from_slice::<Owner>(&bytes).ok())
    else {
        return Ok(false);
    };
    if !left_behind(file, &owner) {
        return Ok(false);
    }
    if owner.candidate {
        crate::candidates::abandoned(&owner.dir)?;
    }
    let dir = File::open(path.parent().unwrap_or(path))?;
    // SAFETY: a blocking lock of a live descriptor, released as it closes.
    ensure!(
        unsafe { libc::flock(dir.as_raw_fd(), libc::LOCK_EX) } == 0,
        "cannot lock the worker slots: {}",
        std::io::Error::last_os_error()
    );
    let open = file.metadata()?;
    match std::fs::metadata(path) {
        Ok(named) if (named.dev(), named.ino()) == (open.dev(), open.ino()) => {
            std::fs::remove_file(path)?;
            let lane = owner
                .ticket
                .as_ref()
                .map_or_else(|| "worker".to_string(), afkconvoy_core::exclusive::lane);
            eprintln!(
                "[afkconvoy] exclusive: freed the {lane} lane's place pid {} left behind: \
                 only a server shared by the host still held it",
                owner.process.pid
            );
            Ok(true)
        }
        Ok(_) => Ok(true),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

/// A fast gate holds no repo lease: fast gates run in their own worktrees,
/// side by side whatever their repo (docs/adr/0049).
fn repo_file(ticket: &Ticket) -> Result<Option<File>> {
    let Some(repo) = ticket.repo.as_ref().filter(|_| !afkconvoy_core::exclusive::is_fast(ticket))
    else {
        return Ok(None);
    };
    let cache = crate::copies::cache()?.join("worker-slots");
    std::fs::create_dir_all(&cache)?;
    let key =
        afkconvoy_core::gate::key(&format!("{repo}:{}", afkconvoy_core::exclusive::lane(ticket)));
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
        recover()?;
        let repo = repo_file(ticket)?;
        if let Some(file) = &repo {
            while !try_repo(file, ticket)? {
                recover()?;
                std::thread::sleep(Duration::from_millis(50));
            }
        }
        let cache = crate::copies::cache()?.join("worker-slots");
        std::fs::create_dir_all(&cache)?;
        let lane = afkconvoy_core::gate::key(&afkconvoy_core::exclusive::lane(ticket));
        let owner = Owner {
            process: crate::candidates::identity(std::process::id())?
                .ok_or_else(|| anyhow::anyhow!("cannot identify slot owner"))?,
            dir: ticket
                .repo
                .as_deref()
                .and_then(|path| std::path::Path::new(path).parent())
                .map_or_else(|| ticket.dir.clone(), |path| path.to_string_lossy().into_owned()),
            candidate: ticket.candidate,
            ticket: Some(Ticket { ran: Some(crate::clock()), ..ticket.clone() }),
        };
        let owner_bytes = serde_json::to_vec(&owner)?;
        loop {
            let count =
                afkconvoy_core::exclusive::limit(ticket, crate::host_settings::of_host()?.lanes());
            let mut n = 0;
            'places: while n < count {
                let path = cache.join(format!("{lane:016x}-{n}"));
                let mut file = OpenOptions::new()
                    .create(true)
                    .truncate(false)
                    .read(true)
                    .write(true)
                    .open(&path)?;
                // SAFETY: a live descriptor; a nonblocking kernel lease.
                if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                    file.seek(SeekFrom::Start(0))?;
                    file.write_all(&owner_bytes)?;
                    file.set_len(owner_bytes.len() as u64)?;
                    file.flush()?;
                    return Ok(Self(file, repo));
                }
                ensure!(
                    std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock,
                    "cannot inspect kernel worker slot"
                );
                // A dead owner cannot silently free an inherited lease. Ask
                // its repo to invalidate and confirm the recorded gate's exit.
                if let Ok(bytes) = read(&file)
                    && let Ok(owner) = serde_json::from_slice::<Owner>(&bytes)
                    && owner.candidate
                    && dead(&owner)?
                {
                    crate::candidates::abandoned(&owner.dir)?;
                }
                // One its owner left behind is free: the path is taken
                // again at once, as a new file.
                if free_left_behind(&path, &file)? {
                    continue 'places;
                }
                n += 1;
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
