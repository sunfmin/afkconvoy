//! What `run`, `tick` and `plan` check before anything else: a host or a repo
//! the fleet cannot work is refused here, not at its first tick or landing.
//!
//! A probe that passed is noted on the host that made it (`Passed`). `run`
//! probes at every start; `tick` and `plan` trust a note that still stands
//! (`standing`) and probe only without one.
//!
//! A check that found the checkout's tracker private is noted beside it
//! (`Private`): a running fleet instance asks again once it no longer stands
//! (`public_tracker`), and a probe stands only while it does.

use std::path::{Path, PathBuf};

use afkconvoy_core::config::{Config, GateNeed};
use afkconvoy_core::cut::NeedFailed;
use afkconvoy_core::gate::outlived;
use afkconvoy_core::layout::{
    Answer, BaseCheck, GATE_KIND, Layout, PROBE_STANDS, answer, base_check, no_base_branch,
    no_gate_records, probe_stands, settle,
};
use afkconvoy_core::tools::{Found, Tool};
use afkconvoy_core::tracker::{Tracker, is_public, public};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::checkout_list::{named_repo, tracker_of};
use crate::config;
use crate::gh::GitHub;
use crate::git::{REMOTE, Repo};
use crate::launch::Launches;
use crate::remote::{Fresh, Push};
use crate::role;
use crate::tools;
use crate::topic::{Explain, See};

/// What a probe's commit says it is.
const PROBE_RECORD: &str = "convoy:probe";

/// The note of the last startup probe that passed (`Repo::note`).
const PASSED_NOTE: &str = "convoy-probe";

/// The note of the last check that found the checkout's tracker private
/// (`Private`).
const TRACKER_NOTE: &str = "convoy-tracker";
const NEEDS_NOTE: &str = "convoy-gate-needs";

#[derive(Serialize, Deserialize)]
struct NeedsPassed {
    at: u64,
    checks: Vec<GateNeed>,
}

/// A check's output is private to its caller. Running instances report only
/// the name and the command to fix it, never output in their tick log.
/// An install that ran and passed is said in `installed`.
fn gate_needs(
    repo: &Repo,
    config: &Config,
    now: u64,
    trusting: bool,
    installed: &mut Vec<String>,
) -> Result<Option<String>> {
    let note = repo.note(NEEDS_NOTE)?;
    if trusting {
        let passed = std::fs::read_to_string(&note)
            .ok()
            .and_then(|text| serde_json::from_str::<NeedsPassed>(&text).ok());
        if passed.is_some_and(|p| p.checks == config.gate_needs && probe_stands(p.at, now)) {
            return Ok(None);
        }
    }
    let _ = std::fs::remove_file(&note);
    for check in &config.gate_needs {
        let (failed, install) = need(repo, check, GATE_NEEDS, now)?;
        if failed.is_none()
            && let Some(install) = install
        {
            installed.push(format!(
                "gate need {} was installed on this host{}",
                check.name,
                install.said()
            ));
        }
        if let Some(failed) = failed {
            let _ = std::fs::remove_file(repo.note(PASSED_NOTE)?);
            return Ok(Some(format!(
                "gate need {} failed ({}); stderr: {}{}; fix it in {}: {}",
                check.name,
                failed.reason,
                failed.stderr.display(),
                failed.install.as_ref().map(Installed::said).unwrap_or_default(),
                repo.dir.display(),
                check.command
            )));
        }
    }
    std::fs::write(
        note,
        serde_json::to_string(&NeedsPassed { at: now, checks: config.gate_needs.clone() })?,
    )?;
    Ok(None)
}

/// The kinds of need, each with a host-cache directory of its own.
const GATE_NEEDS: &str = "gate-needs";
const CUT_NEEDS: &str = "cut-needs";

/// A need that failed on this host: why, the private file of its check's
/// stderr, and the install that ran for it or that is not tried again yet.
struct Failed {
    reason: String,
    stderr: PathBuf,
    install: Option<Installed>,
}

/// What a check's last install did on this host, noted in the host cache
/// beside its private stderr (`installed_of`): when it ran, how long it
/// took, its status, and whether the check passed after it — of the very
/// declaration it was run for. A failed one stands for an hour
/// (`probe_stands`) of that declaration: within it no install of it runs
/// again; a changed declaration installs at once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct Installed {
    pub at: u64,
    pub seconds: u64,
    /// The install's own status: `exit status: 0`, or what ended it.
    pub status: String,
    /// Whether the check passed when it ran again after the install.
    pub passed: bool,
    /// The user-only file of the install's last 20 stderr lines.
    pub stderr: PathBuf,
    /// Whether this was a failed install noted before, not one run now.
    #[serde(skip)]
    pub held: bool,
    pub check: GateNeed,
}

impl Installed {
    /// How a refusal, a warning or doctor says it: never its output.
    pub(crate) fn said(&self) -> String {
        let result = if self.passed { "the check then passed" } else { "the check still failed" };
        let held = if self.held {
            "; not tried again within the hour unless the declaration changes"
        } else {
            ""
        };
        format!(
            "; install ran at {} for {}s ({}), {result}; install stderr: {}{held}",
            self.at,
            self.seconds,
            self.status,
            self.stderr.display()
        )
    }
}

/// The host-cache files of `check` of the checkout `repo`, of `kind`: its
/// check's stderr, its install's stderr, and its install's note.
fn need_files(repo: &Repo, check: &str, kind: &str) -> Result<(PathBuf, PathBuf, PathBuf)> {
    use std::hash::{Hash, Hasher};
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    repo.common_dir()?.hash(&mut hash);
    check.hash(&mut hash);
    let key = format!("{:016x}", hash.finish());
    let cache = crate::copies::cache()?;
    let installs = cache.join(format!("{kind}-install"));
    Ok((
        cache.join(kind).join(format!("{key}.stderr")),
        installs.join(format!("{key}.stderr")),
        installs.join(format!("{key}.json")),
    ))
}

/// The last install of `check` of `kind` noted on this host for `repo`.
pub(crate) fn installed_of(repo: &Repo, check: &GateNeed, kind: &str) -> Option<Installed> {
    let (_, _, note) = need_files(repo, &check.name, kind).ok()?;
    let installed: Installed = serde_json::from_str(&std::fs::read_to_string(note).ok()?).ok()?;
    (installed.check == *check).then_some(installed)
}

/// The last installs noted on this host for the needs `config` declares.
pub(crate) fn installs(repo: &Repo, config: &Config) -> Vec<(&'static str, Installed)> {
    let gate = config.gate_needs.iter().map(|check| (GATE_NEEDS, check));
    let cut = config.cut_needs.iter().map(|check| (CUT_NEEDS, check));
    gate.chain(cut)
        .filter_map(|(kind, check)| Some((kind, installed_of(repo, check, kind)?)))
        .collect()
}

/// One need on this host at `now`: its check; where it fails and declares
/// an install, that install — in the host's exclusive queue, within its
/// limit — and the check again. A failed install of the same declaration
/// within the hour is not run again: the need fails naming it.
fn need(
    repo: &Repo,
    check: &GateNeed,
    kind: &str,
    now: u64,
) -> Result<(Option<Failed>, Option<Installed>)> {
    let (stderr, install_stderr, note) = need_files(repo, &check.name, kind)?;
    let Some(reason) = run_private(repo, &check.command, check.seconds, &stderr)? else {
        return Ok((None, None));
    };
    let (Some(install), Some(seconds)) = (&check.install, check.install_seconds) else {
        return Ok((Some(Failed { reason, stderr, install: None }), None));
    };
    if let Some(mut failed) =
        installed_of(repo, check, kind).filter(|noted| !noted.passed && probe_stands(noted.at, now))
    {
        failed.held = true;
        return Ok((Some(Failed { reason, stderr, install: Some(failed) }), None));
    }
    let began = std::time::Instant::now();
    let status = {
        // An install is heavy: it waits its turn on the host as a gate does,
        // unless this command already holds one.
        let _permit = if crate::exclusive::held() {
            None
        } else {
            Some(crate::exclusive::Permit::take(
                repo,
                &format!("install {kind} {}", check.name),
                true,
                false,
            )?)
        };
        run_private(repo, install, seconds, &install_stderr)?
    };
    let again = run_private(repo, &check.command, check.seconds, &stderr)?;
    let installed = Installed {
        at: now,
        seconds: began.elapsed().as_secs(),
        status: status.unwrap_or_else(|| "exit status: 0".to_string()),
        passed: again.is_none(),
        stderr: install_stderr,
        held: false,
        check: check.clone(),
    };
    std::fs::write(&note, serde_json::to_string(&installed)?)
        .with_context(|| format!("cannot write {}", note.display()))?;
    let failed = again.map(|reason| Failed { reason, stderr, install: Some(installed.clone()) });
    Ok((failed, Some(installed)))
}

/// Run `command` with `sh -e` in the checkout within `seconds`, its stdout
/// discarded → `None` where it passed, removing `stderr`; else why it failed,
/// its last 20 stderr lines kept in the user-only file `stderr`.
fn run_private(repo: &Repo, command: &str, seconds: u64, stderr: &Path) -> Result<Option<String>> {
    use std::io::Write;
    use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
    if let Some(dir) = stderr.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let script = format!("exec 1>/dev/null\n{command}");
    let mut shell = std::process::Command::new("sh");
    shell.args(["-e", "-c", &script]).current_dir(&repo.dir);
    crate::exclusive::inherit(&mut shell, repo)?;
    let (output, timed_out) = crate::process::output_within_with_timeout(
        &mut shell,
        std::time::Duration::from_secs(seconds),
    )?;
    if !timed_out && output.status.success() {
        let _ = std::fs::remove_file(stderr);
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<_> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(20)..].join("\n");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(stderr)?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    file.write_all(tail.as_bytes())?;
    Ok(Some(if timed_out {
        format!("timed out after {seconds}s")
    } else {
        output.status.to_string()
    }))
}

/// The first cut need that fails on this host at `now`, after any install
/// it declares (`need`), with its status and private stderr file. `install`
/// is false for a look that must not install — `plan`'s — which then says a
/// check as it finds it. A check that passes removes its stderr file, so a
/// tick says only the evaluation it starts the cut by (sunfmin/control#532).
pub(crate) fn check_cut_needs(
    repo: &Repo,
    config: &Config,
    now: u64,
    install: bool,
) -> Result<Option<NeedFailed>> {
    for check in &config.cut_needs {
        let failed = if install {
            need(repo, check, CUT_NEEDS, now)?.0
        } else {
            let (stderr, _, _) = need_files(repo, &check.name, CUT_NEEDS)?;
            run_private(repo, &check.command, check.seconds, &stderr)?
                .map(|reason| Failed { reason, stderr, install: None })
        };
        if let Some(failed) = failed {
            return Ok(Some(NeedFailed {
                name: check.name.clone(),
                reason: failed.reason,
                stderr: failed.stderr.display().to_string(),
                install: failed.install.as_ref().map(Installed::said),
            }));
        }
    }
    Ok(None)
}

pub(crate) fn check_gate_needs(repo: &Repo, now: u64) -> Result<Option<String>> {
    let config = config::load(&config::file_of(repo))?;
    gate_needs(repo, &config, now, true, &mut vec![])
}

/// A check that found the checkout's tracker private, as the host that made
/// it notes it: when, and of which tracker — the repo it names, or none for
/// the checkout's own (`noted_as`). It stands for an hour, as a startup probe
/// does (`probe_stands`), and of that tracker alone.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Private {
    /// When it found it private, in epoch seconds.
    at: u64,
    tracker: Option<String>,
}

/// The tracker `tracker` as a note of it names it.
fn noted_as(tracker: &Tracker) -> Option<String> {
    tracker.named().map(|named| named.as_str().to_string())
}

/// Whether a check that found the tracker `tracker` of the checkout `repo`
/// private stands at `now` on this host (`Private`).
fn found_private(repo: &Repo, tracker: &Tracker, now: u64) -> bool {
    let read = || -> Result<Private> {
        Ok(serde_json::from_str(&std::fs::read_to_string(repo.note(TRACKER_NOTE)?)?)?)
    };
    read().is_ok_and(|found| found.tracker == noted_as(tracker) && probe_stands(found.at, now))
}

/// Why the fleet of the checkout `repo` is refused, where GitHub says its
/// tracker is public (`tracker::public`); `None` where it is not. A start's
/// probe asks GitHub every time; one `trusting` — a running fleet instance's,
/// before each tick — asks nothing while a check that found this same
/// tracker private stands, so a tracker made public is found within the
/// hour, and one said anew at once. One found private is noted; one found
/// public is not, and is asked about again at every check, until it is
/// private again or another is said.
pub fn public_tracker(repo: &Repo, now: u64, trusting: bool) -> Result<Option<String>> {
    let gh = GitHub::of(repo)?;
    if trusting && found_private(repo, &gh.tracker, now) {
        return Ok(None);
    }
    let note = repo.note(TRACKER_NOTE)?;
    // Gone before GitHub is asked: no check stands but one that found it private.
    let _ = std::fs::remove_file(&note);
    if let Some(why) = refused_public(repo, &gh)? {
        return Ok(Some(why));
    }
    let found = Private { at: now, tracker: noted_as(&gh.tracker) };
    std::fs::write(&note, serde_json::to_string(&found)?)
        .with_context(|| format!("cannot write {}", note.display()))?;
    Ok(None)
}

/// Why the fleet of the checkout `repo` is refused, where GitHub says the
/// tracker `gh` names is public (`tracker::public`), naming it and the line
/// that says a private one; `None` where it is not.
pub fn refused_public(repo: &Repo, gh: &GitHub) -> Result<Option<String>> {
    if !is_public(&gh.visibility()?) {
        return Ok(None);
    }
    Ok(Some(public(&named(repo, &gh.tracker), &repo.dir.display().to_string())))
}

/// The tracker `tracker` of the checkout `repo`, as a human is told it: the
/// repo it names — of the checkout's own, as its remote names it, where it
/// does.
pub fn named(repo: &Repo, tracker: &Tracker) -> String {
    match tracker.named() {
        Some(named) => named.to_string(),
        None => named_repo(repo).map_or_else(|_| "its own repo".to_string(), |own| own.to_string()),
    }
}

/// A startup probe that passed, as the host that made it notes it: when, what
/// it was asked about, and what it warned of. A probe asked about anything
/// else is another probe.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Passed {
    /// When it passed, in epoch seconds.
    at: u64,
    #[serde(flatten)]
    asked: Asked,
    warnings: Vec<String>,
    /// What the probe found of each harness the fleet could be asked for.
    /// Empty on a note written before this was recorded: a doctor then
    /// trusts the probe and says nothing more of those harnesses.
    #[serde(default)]
    harnesses: Vec<NotedHarness>,
}

/// One harness, as the startup probe found it. A doctor that trusts the
/// probe says this and starts the harness again for nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct NotedHarness {
    /// `Harness::key`.
    harness: String,
    /// `ready`, `missing`, `logged_out`, or `absent` when the command names
    /// no harness tool.
    state: String,
    /// The last answer when logged out, absent on an old note.
    #[serde(default)]
    answer: Option<afkconvoy_core::tools::LoginAnswer>,
    fault: Option<String>,
    flags_fault: Option<String>,
}

impl NotedHarness {
    fn of(checked: &crate::role::Checked) -> Self {
        let state = match &checked.tool {
            Some((_, Found::Missing)) => "missing",
            Some((_, Found::LoggedOut(_))) => "logged_out",
            Some((_, Found::Ready)) => "ready",
            _ => "absent",
        };
        let answer = match &checked.tool {
            Some((_, Found::LoggedOut(answer))) => Some(answer.clone()),
            _ => None,
        };
        Self {
            harness: checked.harness.key().to_string(),
            state: state.to_string(),
            answer,
            fault: checked.fault.clone(),
            flags_fault: checked.flags_fault.clone(),
        }
    }

    pub(crate) fn harness(&self) -> &str {
        &self.harness
    }

    pub(crate) fn state(&self) -> &str {
        &self.state
    }

    pub(crate) fn answer(&self) -> Option<afkconvoy_core::tools::LoginAnswer> {
        self.answer.clone()
    }

    pub(crate) fn fault(&self) -> Option<&str> {
        self.fault.as_deref()
    }

    pub(crate) fn flags_fault(&self) -> Option<&str> {
        self.flags_fault.as_deref()
    }
}

/// What a startup probe is asked about.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
struct Asked {
    base_branch: String,
    /// Where the remote is.
    remote: String,
    /// The team the repo is bound to: a bound repo's workers start in roles.
    team: Option<String>,
    /// The ref layout it settled on, as the repo has it noted.
    layout: String,
    /// The worker launch commands it checked, when the start knew the
    /// default harness's — as their key (`Launches::key`): a command itself
    /// is written to no file.
    launch: Option<u64>,
    #[serde(default)]
    gate_needs: Vec<GateNeed>,
}

impl Asked {
    /// What a probe of `repo` under `config` is asked about now.
    fn now(repo: &Repo, config: &Config, launch: Option<u64>) -> Result<Asked> {
        Ok(Asked {
            base_branch: config.base_branch.clone(),
            remote: repo.git(&["remote", "get-url", REMOTE])?,
            team: config.bound.as_ref().map(|bound| bound.team.to_string()),
            layout: repo.layout().name().to_string(),
            launch,
            gate_needs: config.gate_needs.clone(),
        })
    }
}

/// The warnings of a startup probe that still stands for the checkout
/// `repo`, said again; `None` when there is none, and one is to
/// be made. One stands for an hour (`probe_stands`) on the host that made it,
/// while the config's base branch and team, the remote, and the noted ref
/// layout are what it was asked about — and, for a command that starts
/// workers, `launches` are the worker launch commands it checked — and while
/// a check that found the tracker the checkout says now private stands
/// (`Private`). Says on stderr
/// that no probe was made. Whatever keeps the note from being read or
/// compared is the probe's to report: none stands.
/// The startup probe that still stands for `repo` at `now`, read the way
/// `standing` reads it. `None` when none stands. Says nothing.
fn passed_note(repo: &Repo, now: u64, launches: &Launches) -> Option<Passed> {
    let config = config::load(&repo.dir.join(config::FILE)).ok()?;
    let text = std::fs::read_to_string(repo.note(PASSED_NOTE).ok()?).ok()?;
    let passed: Passed = serde_json::from_str(&text).ok()?;
    // A command that starts no worker asks nothing of the launch commands.
    let launch = if launches.starts() { launches.key().ok()? } else { passed.asked.launch };
    let asked = Asked::now(repo, &config, launch).ok()?;
    let tracker = tracker_of(repo).ok()?;
    if !probe_stands(passed.at, now) || passed.asked != asked || !found_private(repo, &tracker, now)
    {
        return None;
    }
    Some(passed)
}

pub fn standing(repo: &Repo, now: u64, launches: &Launches) -> Option<Vec<String>> {
    let passed = passed_note(repo, now, launches)?;
    eprintln!(
        "note: no startup probe was made: the one that passed on this host {} min ago stands \
         for {} min (`afkconvoy plan --help`)",
        (now - passed.at) / 60,
        PROBE_STANDS / 60
    );
    Some(passed.warnings)
}

/// Whether a startup probe still stands for `repo` at `now`, for a command
/// that starts no worker (`Launches::foreseen`). It is `standing` without
/// the note: `doctor` reads it and makes no probe, and says nothing about one.
///
/// A command that starts no worker does not compare launch commands inside
/// `passed_note`. This does: the probe is trusted only when it checked the
/// commands the kit has now. A doctor then starts no harness.
pub fn stands(repo: &Repo, now: u64) -> bool {
    let launches = Launches::foreseen(repo);
    let Some(passed) = passed_note(repo, now, &launches) else { return false };
    match launches.key() {
        Ok(key) => passed.asked.launch == key,
        Err(_) => false,
    }
}

/// What the standing probe found of each harness, when one stands.
/// Empty when none stands, or the note is from before this was recorded.
pub(crate) fn noted_harnesses(repo: &Repo, now: u64) -> Vec<NotedHarness> {
    let launches = Launches::foreseen(repo);
    passed_note(repo, now, &launches).map(|passed| passed.harnesses).unwrap_or_default()
}

/// Why no checkout is found on a host with no git, which finds one: what a
/// start's first check (`tools::check`) refuses it with in the directory
/// `dir` — git, and every other command the fleet needs that this host lacks,
/// each with the line that installs it, in one refusal. `None` where git is
/// on the `PATH`.
pub fn without_git(dir: &Path) -> Option<anyhow::Error> {
    if tools::on_path(Tool::Git.name()) {
        return None;
    }
    // A checkout git cannot be asked about: what the check asks of it — its
    // remote, whether orca knows it — it answers as of none.
    let unfound = Repo::new(dir.to_path_buf());
    let checked = tools::check(&unfound, &Launches::foreseen(&unfound));
    let refused = checked.err().unwrap_or_else(|| anyhow::anyhow!("git is not on the PATH"));
    Some(See::Troubleshooting.refusal(format!("{refused:#}")))
}

/// Checks this host and the checkout `repo` → what the human
/// should know before the fleet carries on. An error is a host or a repo the
/// fleet cannot work. `launches` are the worker launch commands of the start.
///
/// In order: every command the fleet needs is here and logged in
/// (`tools::check`), before anything is read or written; in a bound repo,
/// every harness the fleet could be asked for can be started, its worker
/// launch command taking what starts a worker in its role
/// (`role::probe`); the base branch exists (none is ever created); no rule of its
/// protection refuses the train's push; the tracker is not public
/// (`public_tracker`), before anything is pushed; where the remote takes the fleet's
/// refs — noted for every later command (`Repo::layout`); whether it takes
/// gate records, and the ones past their day are swept. A probe that passed is
/// noted for `standing`.
pub fn startup(repo: &Repo, now: u64, launches: &Launches) -> Result<Vec<String>> {
    tools::check(repo, launches).see(See::Troubleshooting)?;
    let config = config::load(&config::file_of(repo)).see(See::Troubleshooting)?;
    let mut installed = vec![];
    if let Some(why) = gate_needs(repo, &config, now, false, &mut installed)? {
        return Err(See::Troubleshooting.refusal(why));
    }
    // A bound repo's workers start in their roles, each on the harness its
    // stage names: every one's command must be there, and take that.
    if let Some(bound) = &config.bound {
        role::probe(repo, launches, bound, &config.labels()).see(See::Troubleshooting)?;
    }
    let branch = &config.base_branch;
    let mut warnings = installed;

    if repo.remote().sha(&format!("refs/heads/{branch}"), Fresh::Process)?.is_none() {
        return Err(See::Troubleshooting.refusal(no_base_branch(branch)));
    }
    match base_check(branch, &GitHub::of(repo)?.protection(branch)) {
        BaseCheck::Clear(_) => {}
        BaseCheck::Warn(warning) => warnings.push(warning),
        BaseCheck::Refuse(why) => return Err(See::Troubleshooting.refusal(why)),
    }
    if let Some(why) = public_tracker(repo, now, false)? {
        return Err(See::Troubleshooting.refusal(why));
    }
    crate::remote::check(repo).see(See::Troubleshooting)?;

    // The probe's own name: two starts probing at once never meet on one ref.
    let token = format!("{now}-{}", crate::unique());
    let commit = repo.record_commit(PROBE_RECORD)?;
    let push = |refname: &str| match repo.remote().try_push(&Push::set(&commit, refname)) {
        Ok(ran) => answer(ran.ok, &ran.stderr),
        Err(error) => Answer::Unreached(format!("{error:#}")),
    };
    // A noted layout is asked first and moves only after two probes in a row
    // have seen it rejected while the other was taken. One rejection leaves
    // the note, and a push that reaches no verdict settles nothing.
    let settled = settle(repo.noted(), |layout| push(&layout.probe("", &token)))
        .map_err(|why| See::Troubleshooting.refusal(why))?;
    warnings.extend(settled.warning);
    repo.note_settled(settled.layout, settled.streak)?;
    let layout = settled.layout;

    let records = match push(&layout.probe(GATE_KIND, &token)) {
        Answer::Taken => true,
        Answer::Rejected(why) | Answer::Unreached(why) => {
            warnings.push(no_gate_records(layout, &why));
            false
        }
    };
    // Probes — this one's, on whichever layout it asked, and any a start
    // killed mid-probe left — and the gate records no landing would believe
    // any more. A run that still stands stays, including one left on the
    // layout the note moved off. Gate records of the noted layout are swept
    // only where that layout takes them.
    let patterns: Vec<String> = Layout::ALL.into_iter().map(|layout| layout.every("")).collect();
    let patterns: Vec<&str> = patterns.iter().map(String::as_str).collect();
    let mut litter = vec![];
    for record in repo.remote().records(&patterns, Fresh::Process)? {
        let Some(home) = Layout::ALL.into_iter().find(|home| home.kept(&record.name).is_some())
        else {
            continue;
        };
        let gate_record = home.kept(&record.name).is_some_and(|kept| kept.starts_with(GATE_KIND));
        // A ref that moved while it was read has no subject yet: not litter.
        let readable = gate_record && !record.subject.is_empty() && outlived(&record.subject, now);
        let noted_takes_records = home == layout && records;
        let left_behind = home != layout;
        if home.is_probe(&record.name) || (readable && (noted_takes_records || left_behind)) {
            litter.push(record.name);
        }
    }
    if !litter.is_empty() {
        // Soft: a ref that stays is litter the next start deletes.
        let _ = repo.remote().try_push(&Push::deleting(litter.iter().map(String::as_str)));
    }
    // Recorded so a later doctor can say a harness that is missing, logged
    // out, or unable to take the fleet's flags, without starting it again.
    // A fault here does not refuse the probe: one harness must not stop another.
    let called = role::called_on(repo, Some(&config), launches.default());
    let harnesses = role::observe(launches, &called).iter().map(NotedHarness::of).collect();
    let passed =
        Passed { at: now, asked: Asked::now(repo, &config, launches.key()?)?, warnings, harnesses };
    let note = repo.note(PASSED_NOTE)?;
    std::fs::write(&note, serde_json::to_string(&passed)?)
        .with_context(|| format!("cannot write {}", note.display()))?;
    Ok(passed.warnings)
}

/// Says each warning where a human reads it; stdout stays the command's answer.
pub fn announce(warnings: &[String]) {
    for warning in warnings {
        eprintln!("warning: {warning}");
    }
}
