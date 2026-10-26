//! What shows a worker hot, taken down there and then as it turns hot
//! (docs/adr/0019) — the process may be gone before anyone looks: the process
//! tree with each command line, a five-second `sample` of the hottest
//! process, the resident memory over the last minutes, the processes that
//! wrote the most, and the largest directories of its worktree. It is written
//! to a directory of the worker's worktree's own git dir, and handed to its
//! watch (`crate::watch`).

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result};

use crate::hot::written_by;
use crate::measure;
use crate::orca::Worktree;

/// The file the evidence is written to, in the worktree's evidence directory.
const FILE: &str = "evidence.md";
/// The file the `sample` of the hottest process is written to, beside it.
const SAMPLE: &str = "sample.txt";
/// How long the hottest process is sampled, in seconds.
const SAMPLED_FOR: &str = "5";
/// How many of the processes that wrote the most, and of the largest
/// directories, are taken down.
const TOP: usize = 10;

/// What the sampler knew of a worker as it turned hot.
pub struct Taken {
    /// `#7`, or the name of its worktree.
    pub worker: String,
    /// When its episode started: the evidence is of that episode.
    pub since: u64,
    pub why: String,
    pub held: String,
    /// The processes of its tree.
    pub pids: Vec<u32>,
    /// The one that took the most CPU in the last sample, or held the most.
    pub hottest: Option<u32>,
    /// The bytes its processes held resident at each sample of the last
    /// minutes, oldest first.
    pub resident: Vec<(u64, u64)>,
}

/// The first line of the evidence of the episode that started at `since`:
/// what tells it from the last episode's.
fn head(worker: &str, since: u64) -> String {
    format!("# What shows {worker} hot, episode since {since}")
}

/// Takes down what shows the worker in `worktree` hot → the file it is in.
pub fn take(worktree: &Worktree, taken: &Taken) -> Result<PathBuf> {
    let dir = worktree.evidence_dir()?;
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot make {}", dir.display()))?;
    let mut out = head(&taken.worker, taken.since);
    let _ = write!(out, "\n\nHot for {}; it held {}.\n", taken.why, taken.held);

    out.push_str("\n## The process tree, with each command line\n\n```\n");
    out.push_str(&tree(&taken.pids));
    out.push_str("```\n");

    out.push_str("\n## A five-second sample of the hottest process\n\n");
    out.push_str(&sample(taken.hottest, &dir.join(SAMPLE)));

    out.push_str("\n## Resident memory over the last minutes\n\n```\n");
    for (at, resident) in &taken.resident {
        let _ = writeln!(out, "{at}  {}", measure::said(*resident));
    }
    out.push_str("```\n");

    out.push_str("\n## The processes that wrote the most, since each started\n\n```\n");
    let mut wrote: Vec<(u64, u32)> = taken
        .pids
        .iter()
        .map(|pid| (written_by(*pid), *pid))
        .filter(|(bytes, _)| *bytes > 0)
        .collect();
    wrote.sort_unstable_by(|a, b| b.cmp(a));
    if wrote.is_empty() {
        out.push_str("(none wrote anything this host can say)\n");
    }
    for (bytes, pid) in wrote.into_iter().take(TOP) {
        let _ = writeln!(out, "{pid}  {}", measure::said(bytes));
    }
    out.push_str("```\n");

    out.push_str("\n## The largest directories of its worktree\n\n```\n");
    out.push_str(&largest(&worktree.path));
    out.push_str("```\n");

    let file = dir.join(FILE);
    std::fs::write(&file, out).with_context(|| format!("cannot write {}", file.display()))?;
    Ok(file)
}

/// The evidence of the episode of `worker` that started at `since`, once it
/// is taken down.
pub fn of(worktree: &Worktree, worker: &str, since: u64) -> Option<PathBuf> {
    let file = worktree.evidence_dir().ok()?.join(FILE);
    let text = std::fs::read_to_string(&file).ok()?;
    (text.lines().next() == Some(head(worker, since).as_str())).then_some(file)
}

/// The processes `pids`, as `ps` says them: no environment, which may hold
/// what a human keeps to themselves.
pub(crate) fn tree(pids: &[u32]) -> String {
    if pids.is_empty() {
        return "(no process left)\n".to_string();
    }
    let listed: Vec<String> = pids.iter().map(u32::to_string).collect();
    let out = crate::process::output(Command::new("ps").args([
        "-ww",
        "-o",
        "pid,ppid,rss,%cpu,command",
        "-p",
        &listed.join(","),
    ]));
    out.map(|out| String::from_utf8_lossy(&out.stdout).into_owned())
        .unwrap_or_else(|error| format!("(ps could not be run: {error})\n"))
}

/// Samples `pid` for five seconds into `file` → what the evidence says of it.
fn sample(pid: Option<u32>, file: &Path) -> String {
    let Some(pid) = pid else { return "No process was left to sample.\n".to_string() };
    if !cfg!(target_os = "macos") {
        return format!("The hottest process was {pid}; this host has no `sample`.\n");
    }
    let ran = crate::process::output(
        Command::new("sample").args([&pid.to_string(), SAMPLED_FOR, "-mayDie", "-file"]).arg(file),
    );
    match ran {
        Ok(out) if out.status.success() => {
            format!("The hottest process, {pid}, sampled into `{}`.\n", file.display())
        }
        Ok(out) => format!(
            "The hottest process, {pid}, could not be sampled: {}\n",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
        Err(error) => format!("The hottest process, {pid}, could not be sampled: {error}\n"),
    }
}

/// The largest entries of `dir`, each with its size, largest first.
fn largest(dir: &Path) -> String {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return "(the worktree cannot be read)\n".to_string();
    };
    let paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    if paths.is_empty() {
        return "(nothing in it)\n".to_string();
    }
    let out = crate::process::output(Command::new("du").arg("-sk").args(&paths));
    let Ok(out) = out else { return "(du could not be run)\n".to_string() };
    let mut sized: Vec<(u64, String)> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|line| {
            let (kib, path) = line.split_once('\t')?;
            Some((kib.trim().parse::<u64>().ok()?.saturating_mul(1024), path.to_string()))
        })
        .collect();
    sized.sort_unstable_by(|a, b| b.cmp(a));
    sized
        .into_iter()
        .take(TOP)
        .map(|(bytes, path)| format!("{}  {path}\n", measure::said(bytes)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_largest_entries_of_a_directory_come_first() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("big")).unwrap();
        std::fs::write(dir.path().join("big/blob"), vec![0u8; 256 << 10]).unwrap();
        std::fs::write(dir.path().join("small"), b"x").unwrap();
        let said = largest(dir.path());
        let first = said.lines().next().unwrap();
        assert!(first.ends_with("/big"), "{said}");
        assert_eq!(said.lines().count(), 2, "{said}");
    }
}
