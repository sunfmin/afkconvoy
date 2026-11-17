//! The hold a running cut keeps on its checkout (sunfmin/control#1331): two
//! cuts of one checkout at once share its `target/` and the tests' TMPDIR,
//! and each kills the other's test processes as leaks. An `afkconvoy cut`
//! holds an exclusive `flock` on a file in the checkout's git dir for as long
//! as it runs, and writes there its pid and since when; a second cut of that
//! checkout — the fleet's or one run by hand with `--from` — is refused at
//! once, naming it, and runs nothing. The kernel lets go of the lock when its
//! process ends, however it ends: a dead holder's file is taken over by the
//! next cut that asks.

use std::fs::{File, OpenOptions, TryLockError};
use std::io::{Read, Seek, SeekFrom, Write};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::orca::Worktree;

/// The file in a cut checkout's git dir its running cut holds.
const FILE: &str = "convoy-cut-holder.json";

/// The cut that holds a checkout: its process and since when.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Holder {
    pub pid: u32,
    pub since: u64,
}

/// A cut's hold on its checkout: held until it is dropped, or its process ends.
#[derive(Debug)]
pub struct Held {
    _file: File,
}

/// Holds the cut checkout `checkout` for this process, at `now` → `Ok(held)`;
/// `Err(holder)` where another running cut holds it, `holder` what it wrote
/// (`None` where it wrote nothing readable yet).
pub fn take(checkout: &Worktree, now: u64) -> Result<std::result::Result<Held, Option<Holder>>> {
    let mut file = open(checkout)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(TryLockError::WouldBlock) => return Ok(Err(read(&mut file))),
        Err(TryLockError::Error(error)) => {
            return Err(error).context("cannot lock the cut checkout's holder file");
        }
    }
    let holder = Holder { pid: std::process::id(), since: now };
    file.set_len(0)?;
    file.seek(SeekFrom::Start(0))?;
    file.write_all(serde_json::to_string(&holder)?.as_bytes())?;
    file.flush()?;
    Ok(Ok(Held { _file: file }))
}

/// What a cut that holds `checkout` is, in words: its pid, the step its
/// record says it is at, and since when.
pub fn said(checkout: &Worktree, holder: Option<&Holder>) -> String {
    let record = checkout.cut_record().ok().flatten();
    let step = match record.as_ref() {
        Some(record) => match record.at {
            Some(at) => format!("at its step `{}`", at.word()),
            None => format!("before its step `{}`", record.step().word()),
        },
        None => "at no step its record says".to_string(),
    };
    match holder {
        Some(holder) => format!(
            "another cut holds {}: pid {}, {step}, since {} ({} UTC)",
            checkout.path.display(),
            holder.pid,
            holder.since,
            crate::log::utc(holder.since)
        ),
        None => format!("another cut holds {}, {step}", checkout.path.display()),
    }
}

fn open(checkout: &Worktree) -> Result<File> {
    let path = checkout.note(FILE)?;
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("cannot open {}", path.display()))
}

fn read(file: &mut File) -> Option<Holder> {
    let mut said = String::new();
    file.read_to_string(&mut said).ok()?;
    serde_json::from_str(&said).ok()
}
