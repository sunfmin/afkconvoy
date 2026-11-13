//! What a host watch is given, taken down as it starts (docs/adr/0019):
//! admission's reading and shares; for disk, what each of this host's
//! worktrees of every fleet holds — with its issue, its stage and whether a
//! worker holds it — the fleet's own among them, the fleet's caches, and
//! the largest directories of the whole volume under the home folder outside
//! them, found within five minutes; for load or memory, the process
//! trees of each worker, and `hostvitals now` where it answers, otherwise
//! the fleet's hottest processes outside them. It is written to a directory
//! of the cache every fleet of the host shares, and handed to the watch
//! (`crate::watch`).

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use afkconvoy_core::resources::Reason;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::admission::Admission;
use crate::copies::cache;
use crate::evidence::tree as ps_of;
use crate::git::Repo;
use crate::hot::{processes, tree};
use crate::measure;
use crate::orca::{OnHost, Worktree};

/// The directory of the cache the evidence is written to.
const DIR: &str = "watch/evidence";
/// The file it is written to there.
const FILE: &str = "evidence.md";
/// How many of the largest directories outside the worktrees, and of the
/// hottest processes outside the trees, are taken down.
const TOP: usize = 10;
/// How long the fleet's caches are measured for, at the most: what is not
/// measured by then is said to be.
const MEASURED_WITHIN: Duration = Duration::from_secs(60);
/// How long the volume under the home folder is measured for, at the most:
/// what `du` had not reached by then is said not to be listed.
const VOLUME_WITHIN: Duration = Duration::from_secs(300);
/// How deep under the home folder directories are listed.
const DEPTH: &str = "4";
/// How many of the largest directories of the volume are listed.
const LARGEST: usize = 40;

/// What the observing fleet knows of its own worktrees.
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
    /// Whether it is of the observing fleet's repo.
    pub own: bool,
    /// Its issue's stage, of the observing fleet's issues.
    pub stage: Option<String>,
    /// What git ignores in it — what its builds made — and the files git
    /// tracks; `None` where it cannot be read.
    pub built: Option<u64>,
    pub source: Option<u64>,
}

/// Takes down what a host watch for `reason` is given, `admission`
/// being what held back → the file it is in.
pub fn take(admission: &Admission, reason: Reason, reading: &str, own: &Own) -> Result<PathBuf> {
    let dir = cache()?.join(DIR).join(format!("{}-{}", reason.word(), crate::unique()));
    std::fs::create_dir_all(&dir).with_context(|| format!("cannot make {}", dir.display()))?;
    let on_host = Worktree::on_host()?;
    let mut out = format!("# What a host watch for {} was given\n\n", reason.word());
    let _ = writeln!(out, "What calls for this watch: {reading}.\n");
    out.push_str("## Admission's reading and shares\n\n```json\n");
    out.push_str(&serde_json::to_string_pretty(&admission.json()).unwrap_or_default());
    out.push_str("\n```\n");
    match reason {
        Reason::Disk => disk(&mut out, on_host, own),
        Reason::Load | Reason::Memory | Reason::Share => processes_of(&mut out, &on_host),
        // The reading above is the whole of it: the account's window, or
        // the queue's commands and their recent run times.
        Reason::Window | Reason::Queue => {}
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
    out.push_str(
        "\n## The largest directories of the volume under the home folder, but for the \
         worktrees above\n\n```\n",
    );
    let worktrees: Vec<PathBuf> = listed.iter().map(|listed| listed.on.path.clone()).collect();
    match std::env::var_os("HOME") {
        Some(home) => {
            let deadline = Instant::now() + VOLUME_WITHIN;
            out.push_str(&largest_said(Path::new(&home), &worktrees, deadline));
        }
        None => out.push_str("(HOME is not set)\n"),
    }
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
            (true, _) => "the fleet's own (a watch's, a caster's or a cut's)".to_string(),
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

/// The largest directories under `home` on its volume, `DEPTH` deep — as
/// `du -x` says them, so no other volume is crossed — but for the `worktrees`
/// and what is in them, measured until `deadline`: what `du` had not reached
/// by then is said not to be listed.
fn largest_said(home: &Path, worktrees: &[PathBuf], deadline: Instant) -> String {
    let child = crate::process::spawn(
        Command::new("du")
            .args(["-xk", "-d", DEPTH])
            .arg(home)
            .stdout(Stdio::piped())
            .stderr(Stdio::null()),
    );
    let Ok(mut child) = child else { return "(du could not be run)\n".to_string() };
    let Some(stdout) = child.stdout.take() else { return "(du said nothing)\n".to_string() };
    // Read beside, for `du` says each directory once it is measured: what it
    // said by the deadline stands.
    let read = std::thread::spawn(move || {
        std::io::BufRead::lines(std::io::BufReader::new(stdout))
            .map_while(Result::ok)
            .collect::<Vec<String>>()
    });
    let mut stopped = false;
    while child.try_wait().ok().flatten().is_none() {
        if Instant::now() >= deadline {
            let _ = child.kill();
            stopped = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    let _ = child.wait();
    let said = read.join().unwrap_or_default();
    let mut out = largest(&said, worktrees);
    if stopped {
        let _ = writeln!(
            out,
            "(du was stopped at {} minutes: what it had not reached is not listed)",
            VOLUME_WITHIN.as_secs() / 60
        );
    }
    out
}

/// The `LARGEST` directories `du` `said` — `<KiB>\t<path>` a line — largest
/// first, but for the `worktrees` and what is in them.
fn largest(said: &[String], worktrees: &[PathBuf]) -> String {
    let mut sized: Vec<(u64, &str)> = said
        .iter()
        .filter_map(|line| {
            let (kib, path) = line.split_once('\t')?;
            Some((kib.trim().parse::<u64>().ok()?.saturating_mul(1024), path))
        })
        .filter(|(_, path)| !worktrees.iter().any(|worktree| Path::new(path).starts_with(worktree)))
        .collect();
    if sized.is_empty() {
        return "(none measured)\n".to_string();
    }
    sized.sort_unstable_by(|a, b| b.cmp(a));
    sized
        .into_iter()
        .take(LARGEST)
        .map(|(bytes, path)| format!("{}  {path}\n", measure::said(bytes)))
        .collect()
}

/// The bytes under `dir` on its own volume, as `du -sxk` says them; `None`
/// where it could not say by `deadline`.
pub(crate) fn du(dir: &Path, deadline: Instant) -> Option<u64> {
    if Instant::now() >= deadline {
        return None;
    }
    let mut child = crate::process::spawn(
        Command::new("du").arg("-sxk").arg(dir).stdout(Stdio::piped()).stderr(Stdio::null()),
    )
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
/// fleet, and the hottest processes outside them.
fn processes_of(out: &mut String, on_host: &[OnHost]) {
    let procs = processes();
    let mut inside: HashSet<u32> = HashSet::new();
    out.push_str("\n## The process tree of each worker\n");
    for on in on_host {
        let pids: Vec<u32> = tree(&procs, &on.path).iter().map(|proc| proc.pid).collect();
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
    match hostvitals() {
        Ok(answer) => {
            out.push_str("\n## Host processes and Apps\n\nSource: `hostvitals now` (the whole host).\n\n```json\n");
            out.push_str(&answer);
            out.push_str("\n```\n");
            return;
        }
        Err(error) => {
            let said = format!("{error:#}").split_whitespace().collect::<Vec<_>>().join(" ");
            let _ = writeln!(out, "\nSource: afkconvoy; `hostvitals now`: {said}.");
        }
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

/// Optional whole-host evidence, preserving every reading but placing
/// findings first and process and App rankings next. Missing, failed or
/// unrecognisable answers leave the fleet's evidence available.
fn hostvitals() -> Result<String> {
    #[derive(Deserialize, Serialize)]
    struct Now {
        findings: Vec<Value>,
        rankings: Map<String, Value>,
        #[serde(flatten)]
        readings: Map<String, Value>,
    }

    let output = crate::process::output(Command::new("hostvitals").arg("now"))
        .context("could not be run from the host runner's PATH")?;
    let said = format!(
        "{} {}",
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    );
    if !output.status.success() {
        bail!("{}: {}", output.status, said.trim());
    }
    let answer: Now = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("unrecognisable answer: {}", said.trim()))?;
    if answer.readings.get("error").is_some_and(|error| !error.is_null()) {
        bail!("{}", said.trim());
    }
    Ok(serde_json::to_string_pretty(&answer)?)
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

    #[test]
    fn the_largest_directories_of_the_volume_are_listed_but_for_the_worktrees() {
        let said: Vec<String> = [
            "1048576\t/h/.cache/huggingface",
            "15728640\t/h/w/a/target",
            "15728640\t/h/w/a",
            "2048\t/h/Downloads",
            "not a line",
        ]
        .map(String::from)
        .to_vec();
        let listed = largest(&said, &["/h/w/a".into()]);
        assert_eq!(listed, "1.0 GiB  /h/.cache/huggingface\n2.0 MiB  /h/Downloads\n");
        let home = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(home.path().join("big")).unwrap();
        std::fs::write(home.path().join("big/blob"), vec![1u8; 512 << 10]).unwrap();
        let measured = largest_said(home.path(), &[], Instant::now() + MEASURED_WITHIN);
        assert!(measured.lines().any(|line| line.ends_with("/big")), "{measured}");
        let late = largest_said(home.path(), &[], Instant::now());
        assert!(late.contains("du was stopped at 5 minutes"), "{late}");
    }
}
