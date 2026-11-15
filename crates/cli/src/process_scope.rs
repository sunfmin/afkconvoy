//! A dedicated inherited descriptor identifies gate descendants even after
//! reparenting, a new process group, or macOS hiding a system binary's env.
//! Only the gate receives it. The parent closes its copy after spawn; its
//! shared kernel lock is free only after every inheriting process has exited.
use std::fs::{File, OpenOptions};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, ensure};

pub(crate) fn path(worktree: &str) -> PathBuf {
    PathBuf::from(format!("{worktree}.processes"))
}

pub(crate) fn create(path: &Path, command: &mut Command) -> Result<File> {
    let file =
        OpenOptions::new().create_new(true).read(true).write(true).open(path).with_context(
            || format!("cannot create the gate's process scope {}", path.display()),
        )?;
    // SAFETY: this live descriptor is shared by the gate and its descendants.
    ensure!(
        unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH) } == 0,
        "cannot establish gate process ownership"
    );
    let fd = file.as_raw_fd();
    // SAFETY: clear one descriptor flag in the child, with no allocation.
    unsafe {
        command.pre_exec(move || {
            if libc::fcntl(fd, libc::F_SETFD, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    Ok(file)
}

/// Whether no process holds the scope at `path`. It looks through a
/// descriptor open for reading alone, which `owns` never counts: a process
/// only looking at a scope is never taken for one of its holders, and never
/// killed as one (sunfmin/control#1096).
pub(crate) fn free(path: &Path) -> Result<bool> {
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .with_context(|| format!("cannot open the gate's process scope {}", path.display()))?;
    // SAFETY: nonblocking observation of this scope's kernel lease.
    if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
        return Ok(true);
    }
    ensure!(
        std::io::Error::last_os_error().kind() == std::io::ErrorKind::WouldBlock,
        "cannot confirm gate descendant exit"
    );
    Ok(false)
}

/// Whether process `pid` holds the scope at `path`: has it open for writing,
/// as only the descriptor its gate inherits is (`create`). One open for
/// reading alone is a look (`free`), not a hold.
pub(crate) fn owns(pid: u32, path: &Path) -> Result<bool> {
    let meta = std::fs::metadata(path)
        .with_context(|| format!("cannot read the gate's process scope {}", path.display()))?;
    owns_file(pid, meta.dev(), meta.ino())
}

#[cfg(target_os = "linux")]
fn owns_file(pid: u32, dev: u64, ino: u64) -> Result<bool> {
    let files = match std::fs::read_dir(format!("/proc/{pid}/fd")) {
        Ok(files) => files,
        Err(_) => return Ok(false), // exit/permissions do not prove scope exit; free() does.
    };
    for entry in files.flatten() {
        if let Ok(meta) = std::fs::metadata(entry.path())
            && meta.dev() == dev
            && meta.ino() == ino
            && writes(pid, &entry.file_name())
        {
            return Ok(true);
        }
    }
    Ok(false)
}

/// Whether descriptor `fd` of `pid` is open for writing, as its fdinfo's
/// octal `flags` say.
#[cfg(target_os = "linux")]
fn writes(pid: u32, fd: &std::ffi::OsStr) -> bool {
    let Ok(info) = std::fs::read_to_string(format!("/proc/{pid}/fdinfo/{}", fd.to_string_lossy()))
    else {
        return false;
    };
    info.lines()
        .find_map(|line| line.strip_prefix("flags:"))
        .and_then(|flags| u32::from_str_radix(flags.trim(), 8).ok())
        .is_some_and(|flags| flags & libc::O_ACCMODE as u32 != libc::O_RDONLY as u32)
}

#[cfg(target_os = "macos")]
fn owns_file(pid: u32, dev: u64, ino: u64) -> Result<bool> {
    // sys/proc_info.h's proc_fileinfo and vnode_fdinfo. libc supplies the
    // vnode and descriptor-list types, but not this containing struct.
    #[repr(C)]
    struct FileInfo {
        flags: u32,
        status: u32,
        offset: i64,
        kind: i32,
        guard: u32,
    }
    #[repr(C)]
    struct VnodeFd {
        file: FileInfo,
        vnode: libc::vnode_info,
    }
    let size = std::mem::size_of::<libc::proc_fdinfo>();
    // SAFETY: a null zero-length buffer asks libproc for the required bytes.
    let bytes = unsafe {
        libc::proc_pidinfo(pid as i32, libc::PROC_PIDLISTFDS, 0, std::ptr::null_mut(), 0)
    };
    if bytes <= 0 {
        return Ok(false);
    }
    let capacity = (bytes as usize / size).saturating_add(128);
    let total = capacity
        .checked_mul(size)
        .ok_or_else(|| anyhow::anyhow!("process descriptor list is too large"))?;
    let total = i32::try_from(total)?;
    let mut fds: Vec<std::mem::MaybeUninit<libc::proc_fdinfo>> = Vec::with_capacity(capacity);
    fds.resize_with(capacity, std::mem::MaybeUninit::zeroed);
    // SAFETY: the buffer holds total bytes, and the kernel returns that bound.
    let got = unsafe {
        libc::proc_pidinfo(pid as i32, libc::PROC_PIDLISTFDS, 0, fds.as_mut_ptr().cast(), total)
    };
    if got <= 0 {
        return Ok(false);
    }
    ensure!(got < total, "process descriptor list changed during observation; retry cancellation");
    for fd in fds.iter().take(got as usize / size) {
        // SAFETY: only entries included in the kernel's returned length.
        let fd = unsafe { fd.assume_init_ref() };
        if fd.proc_fdtype != libc::PROX_FDTYPE_VNODE as u32 {
            continue;
        }
        let mut info = std::mem::MaybeUninit::<VnodeFd>::zeroed();
        let size = std::mem::size_of::<VnodeFd>() as i32;
        // SAFETY: flavor 1 is PROC_PIDFDVNODEINFO, of the exact SDK layout above.
        let got = unsafe {
            libc::proc_pidfdinfo(pid as i32, fd.proc_fd, 1, info.as_mut_ptr().cast(), size)
        };
        if got != size {
            continue;
        }
        // SAFETY: the initialized structure consists only of numeric fields.
        let info = unsafe { info.assume_init() };
        // sys/fcntl.h's FWRITE: the descriptor is open for writing.
        const FWRITE: u32 = 0x2;
        if u64::from(info.vnode.vi_stat.vst_dev) == dev
            && info.vnode.vi_stat.vst_ino == ino
            && info.file.flags & FWRITE != 0
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn owns_file(_pid: u32, _dev: u64, _ino: u64) -> Result<bool> {
    anyhow::bail!(
        "this host cannot confirm inherited gate descriptors; its candidate lane remains occupied"
    )
}
