//! What a host inspection is given, taken down as it starts (docs/adr/0019):
//! admission's reading and shares; for disk, what each of this host's
//! worktrees of every fleet holds — with its issue, its stage and whether a
//! worker holds it — the landing train's among them, the fleet's caches, and
//! the largest directories outside them; for load or memory, the process
//! trees of each worker and of the train, and the hottest processes outside
//! them. It is written to a directory of the cache every fleet of the host
//! shares, and handed to the inspection (`crate::inspection`).

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::host_inspection::Reason;
use anyhow::{Context, Result};

use crate::admission::Admission;
use crate::copies::cache;
use crate::evidence::tree as ps_of;
use crate::git::Repo;
use crate::hot::{processes, tree};
use crate::measure;
use crate::orca::{OnHost, TMP_DIR, Worktree};

/// The directory of the cache the evidence is written to.
const DIR: &str = "host-inspection";
/// The file it is written to there.
const FILE: &str = "evidence.md";
/// How many of the largest directories outside the worktrees, and of the
/// hottest processes outside the trees, are taken down.
const TOP: usize = 10;
/// How long the directories outside the worktrees are measured for, at the
/// most: what is not measured by then is said to be.
const MEASURED_WITHIN: Duration = Duration::from_secs(60);

/// What the inspecting fleet knows of its own worktrees.
pub struct Own<'a> {
    /// Its repo's main checkout.
    pub repo: &'a Path,
    /// Each of its issues, with the stage it is at, as its last tick saw it.
    pub stages: &'a [(u64, String)],
}

/// One worktree of the host, as the evidence lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Listed {
    pub on: OnHost,
    /// Whether it is of the inspecting fleet's repo.
    pub own: bool,
    /// Its issue's stage, of the inspecting fleet's issues.
    pub stage: Option<String>,
    /// What git ignores in it — what its builds made — and the files git
    /// tracks; `None` where it cannot be read.
    pub built: Option<u64>,
    pub source: Option<u64>,
}

/// Takes down what a host inspection for `reason` is given, `admission`
/// being what held back → the file it is in.
pub fn take(admission: &Admission, reason: Reason, own: &Own) -> Result<PathBuf> {
    let dir = cache()?.join(DIR);
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot make {}", dir.display()))?;
    let on_host = Worktree::on_host()?;
    let mut out = format!("# What a host inspection for {} was given\n\n", reason.word());
    let holds: Vec<String> = admission.holds().iter().map(|hold| admission.said(hold)).collect();
    let _ = writeln!(out, "Admission holds back new workers: {}.\n", holds.join("; "));
    out.push_str("## Admission's reading and shares\n\n```json\n");
    out.push_str(&serde_json::to_string_pretty(&admission.json()).unwrap_or_default());
    out.push_str("\n```\n");
    match reason {
        Reason::Disk => disk(&mut out, on_host, own),
        Reason::Load | Reason::Memory => processes_of(&mut out, &on_host),
    }
    let file = dir.join(FILE);
    std::fs::write(&file, out).with_context(|| format!("cannot write {}", file.display()))?;
    Ok(file)
}

/// The disk's part: every worktree of the host with what it holds, the
/// caches, and the largest directories outside them.
fn disk(out: &mut String, on_host: Vec<OnHost>, own: &Own) {
    let listed: Vec<Listed> = on_host.into_iter().map(|on| measured(on, own)).collect();
    out.push_str("\n## Each worktree of this host, of every fleet, largest first\n\n");
    out.push_str(&worktrees_said(&listed));
    out.push_str("\n## The fleet's caches\n\n```\n");
    let deadline = Instant::now() + MEASURED_WITHIN;
    let caches: Vec<PathBuf> = cache().into_iter().collect();
    out.push_str(&sizes_said(&caches, &[], deadline));
    out.push_str("```\n");
    out.push_str("\n## The largest directories of the home, but for the worktrees above\n\n```\n");
    let worktrees: Vec<(PathBuf, u64)> = listed
        .iter()
        .map(|listed| (listed.on.path.clone(), held_by(listed).unwrap_or(0)))
        .collect();
    let home: Vec<PathBuf> = std::env::var_os("HOME")
        .and_then(|home| std::fs::read_dir(home).ok())
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_dir() && !path.is_symlink())
        .collect();
    out.push_str(&sizes_said(&home, &worktrees, deadline));
    out.push_str("```\n");
}

/// `on`, with what it holds and its stage.
fn measured(on: OnHost, own: &Own) -> Listed {
    let repo = Repo::new(on.path.clone());
    let mine = on.repo.as_deref().is_some_and(|repo| same_dir(repo, own.repo));
    let stage = match (mine, on.issue) {
        (true, Some(issue)) => {
            own.stages.iter().find(|(number, _)| *number == issue).map(|(_, stage)| stage.clone())
        }
        _ => None,
    };
    let (built, source) = (measure::built(&repo).ok(), measure::source(&repo).ok());
    Listed { on, own: mine, stage, built, source }
}

fn same_dir(a: &Path, b: &Path) -> bool {
    match (a.canonicalize(), b.canonicalize()) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// What a listed worktree holds in all, as far as it was read.
fn held_by(listed: &Listed) -> Option<u64> {
    match (listed.built, listed.source) {
        (None, None) => None,
        (built, source) => Some(built.unwrap_or(0).saturating_add(source.unwrap_or(0))),
    }
}

/// The worktrees `listed`, largest first: each with what it holds, what of
/// that its builds made, its issue and stage, whose it is, and whether a
/// worker holds it.
pub fn worktrees_said(listed: &[Listed]) -> String {
    if listed.is_empty() {
        return "(orca knows no worktree on this host)\n".to_string();
    }
    let mut sorted: Vec<&Listed> = listed.iter().collect();
    sorted.sort_by_key(|listed| std::cmp::Reverse(held_by(listed).unwrap_or(0)));
    let mut out = String::new();
    for listed in sorted {
        let size = held_by(listed).map_or_else(|| "size unread".to_string(), measure::said);
        let built = listed
            .built
            .map(|built| format!(", {} of it build output", measure::said(built)))
            .unwrap_or_default();
        let what = match (listed.on.fleets, listed.on.issue) {
            (true, _) => "the fleet's own (the landing train's, or an inspection's)".to_string(),
            (false, Some(issue)) => match &listed.stage {
                Some(stage) => format!("issue #{issue}, {stage}"),
                None if listed.own => {
                    format!("issue #{issue}, not open at a stage of this fleet (closed?)")
                }
                None => format!("issue #{issue}"),
            },
            (false, None) => "linked to no issue".to_string(),
        };
        let whose = match (listed.own, &listed.on.repo) {
            (true, _) => "this fleet's".to_string(),
            (false, Some(repo)) => format!("of {}", repo.display()),
            (false, None) => "of a repo orca does not name".to_string(),
        };
        let held = if listed.on.held { "a worker holds it" } else { "no worker holds it" };
        let _ = writeln!(
            out,
            "- `{}`: {size}{built} · {what} · {whose} · {held}",
            listed.on.path.display()
        );
    }
    out
}

/// What each of `dirs` holds, largest first, less what the worktrees
/// `inside` of it hold, as `du` says it — each measured until `deadline`, and
/// what is not by then said to be.
fn sizes_said(dirs: &[PathBuf], inside: &[(PathBuf, u64)], deadline: Instant) -> String {
    if dirs.is_empty() {
        return "(none)\n".to_string();
    }
    let mut sized = vec![];
    let mut unmeasured = vec![];
    for dir in dirs {
        let Some(bytes) = du(dir, deadline) else {
            unmeasured.push(dir.display().to_string());
            continue;
        };
        let worktrees: u64 =
            inside.iter().filter(|(path, _)| path.starts_with(dir)).map(|(_, held)| *held).sum();
        sized.push((bytes.saturating_sub(worktrees), dir.display().to_string()));
    }
    sized.sort_unstable_by(|a, b| b.cmp(a));
    let mut out: String = sized
        .into_iter()
        .take(TOP)
        .map(|(bytes, path)| format!("{}  {path}\n", measure::said(bytes)))
        .collect();
    if !unmeasured.is_empty() {
        let _ = writeln!(out, "(not measured in time: {})", unmeasured.join(", "));
    }
    out
}

/// The bytes under `dir` on its own volume, as `du -sxk` says them; `None`
/// where it could not say by `deadline`.
fn du(dir: &Path, deadline: Instant) -> Option<u64> {
    if Instant::now() >= deadline {
        return None;
    }
    let mut child = Command::new("du")
        .arg("-sxk")
        .arg(dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    loop {
        if child.try_wait().ok()?.is_some() {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let mut said = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut said).ok()?;
    let kib: u64 = said.split_whitespace().next()?.parse().ok()?;
    Some(kib.saturating_mul(1024))
}

/// The load's and memory's part: the process tree of each worker and of the
/// train, and the hottest processes outside them.
fn processes_of(out: &mut String, on_host: &[OnHost]) {
    let procs = processes();
    let mut inside: HashSet<u32> = HashSet::new();
    out.push_str("\n## The process tree of each worker and of the landing train\n");
    for on in on_host {
        let tmp = on.path.join(TMP_DIR).display().to_string();
        let pids: Vec<u32> = tree(&procs, &tmp).iter().map(|proc| proc.pid).collect();
        if pids.is_empty() {
            continue;
        }
        inside.extend(&pids);
        let issue = on.issue.map(|issue| format!(" (issue #{issue})")).unwrap_or_default();
        let resident: u64 =
            procs.iter().filter(|proc| pids.contains(&proc.pid)).map(|proc| proc.resident).sum();
        let _ = write!(
            out,
            "\n### `{}`{issue}: {} resident\n\n```\n{}```\n",
            on.path.display(),
            measure::said(resident),
            ps_of(&pids)
        );
    }
    out.push_str("\n## The hottest processes outside them\n\n");
    let mut outside: Vec<_> = procs.iter().filter(|proc| !inside.contains(&proc.pid)).collect();
    outside.sort_by_key(|proc| std::cmp::Reverse(proc.cpu_ms));
    let by_cpu: Vec<u32> = outside.iter().take(TOP).map(|proc| proc.pid).collect();
    outside.sort_by_key(|proc| std::cmp::Reverse(proc.resident));
    let by_memory: Vec<u32> = outside.iter().take(TOP).map(|proc| proc.pid).collect();
    let _ = write!(
        out,
        "The most CPU taken since each started:\n\n```\n{}```\n\nThe most memory \
         resident:\n\n```\n{}```\n",
        ps_of(&by_cpu),
        ps_of(&by_memory)
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listed(path: &str, issue: Option<u64>, held: bool, built: u64) -> Listed {
        Listed {
            on: OnHost {
                path: path.into(),
                repo: Some("/src/widgets".into()),
                issue,
                fleets: false,
                held,
            },
            own: true,
            stage: None,
            built: Some(built),
            source: Some(1 << 20),
        }
    }

    #[test]
    fn a_worktree_no_worker_holds_is_listed_with_its_issue_and_stage() {
        const GIB: u64 = 1 << 30;
        let landing =
            Listed { stage: Some("landing".into()), ..listed("/w/a", Some(7), false, 14 * GIB) };
        let building =
            Listed { stage: Some("at build".into()), ..listed("/w/b", Some(8), true, 2 * GIB) };
        let train = Listed {
            on: OnHost { fleets: true, ..landing.on.clone() },
            ..listed("/w/convoy-train", None, false, 15 * GIB)
        };
        let theirs = Listed {
            own: false,
            on: OnHost { repo: Some("/src/gadgets".into()), ..building.on.clone() },
            ..listed("/w/c", Some(3), false, GIB)
        };
        let said = worktrees_said(&[building, landing, train, theirs]);
        let lines: Vec<&str> = said.lines().collect();
        assert_eq!(lines.len(), 4, "{said}");
        assert!(lines[0].contains("the fleet's own"), "largest first: {said}");
        assert!(
            lines[1].starts_with("- `/w/a`: 14.0 GiB, 14.0 GiB of it build output · issue #7, landing · this fleet's · no worker holds it"),
            "{said}"
        );
        assert!(
            lines[2].contains("issue #8, at build · this fleet's · a worker holds it"),
            "{said}"
        );
        assert!(lines[3].contains("of /src/gadgets"), "{said}");
    }

    #[test]
    fn what_holds_the_disk_outside_the_worktrees_is_measured_less_them() {
        let home = tempfile::tempdir().unwrap();
        let big = home.path().join("big");
        std::fs::create_dir_all(big.join("w")).unwrap();
        std::fs::write(big.join("blob"), vec![1u8; 512 << 10]).unwrap();
        std::fs::write(big.join("w/blob"), vec![1u8; 256 << 10]).unwrap();
        let worktree = (big.join("w"), 256 << 10);
        let deadline = Instant::now() + MEASURED_WITHIN;
        let all = sizes_said(std::slice::from_ref(&big), &[], deadline);
        let less = sizes_said(std::slice::from_ref(&big), &[worktree], deadline);
        assert!(less.lines().next().unwrap().ends_with("/big"), "{less}");
        assert_ne!(all, less, "the worktree inside is not counted twice");
        let late = sizes_said(&[big], &[], Instant::now());
        assert!(late.contains("not measured in time"), "{late}");
    }
}
