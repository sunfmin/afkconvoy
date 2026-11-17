//! Landing with plain git (docs/adr/0058), for a program that has no runner
//! of its own: one process takes its changes into a round, pairs them and
//! fast-gates each pair, its pairs' gates of one level at once, merges the
//! round's root onto the target tip, gates it whole once and lands the round
//! as one atomic fast-forward (docs/adr/0083), on the chain its host keeps
//! (docs/adr/0117), through the round's rules (`pairing`), the turn's (`turn`) and
//! the gate crate's plain gate runner (`afkconvoy_gate::run`). It asks its
//! program's forge what it must (`Forge`).
//!
//! A run that asks while another run's round runs its whole gate stacks on
//! that round's root (sunfmin/control#1140): it pairs its changes, fast-gates
//! their root merged onto that root-onto-tip commit, and waits. That round
//! landed, its own lands with only its slow gate run; ended otherwise, its
//! changes pair again in a round of their own.
//!
//! What afkconvoy's own runner adds is not here: lanes of a host's
//! exclusive queue, a gate's processes owned, frozen and confirmed gone
//! across turns and hosts, a candidate adopted by a later turn, a base tip
//! certified alone before a candidate is assigned on it. Here each gate runs
//! to its end in this process, so a candidate is stopped once its gate
//! returns; a source head is read unchanged just before the push that lands
//! it, not within it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use afkconvoy_gate::gate::{Ended, GateCommand, Judged, Red, Tree};
use afkconvoy_gate::run::{self, Git};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::chain::{
    Barrier, Candidate, Chain, Decision, Evidence, Observed, Phase, Process, Reused, Status,
};
use crate::forge::{self, Change, Forge, SentBack, source_ref};
use crate::pairing::{self, Asks, Built, Left, Merged, RoundNext};
use crate::turn::{self, Declared};

/// How many of a run's last lines its evidence keeps.
const EXCERPT_LINES: usize = 40;

/// What a landing fails with: git, or the forge.
#[derive(Debug)]
pub enum Error<E> {
    Git(String),
    Forge(E),
}

impl<E> From<String> for Error<E> {
    fn from(said: String) -> Error<E> {
        Error::Git(said)
    }
}

/// What became of one change a landing was asked to land.
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome<R> {
    /// It landed on its target, in the commit named.
    Landed(String),
    /// The chain decided it otherwise (`Candidate::decision`).
    Decided(Decision),
    /// Merging its head onto its predecessor conflicts: nothing was assigned.
    Conflict,
    /// Its forge says it may not land now.
    Refused(R),
    /// Its forge no longer has it asking to land, or the remote has no ref
    /// at its source (`Change::source`).
    Gone,
    /// It is on the chain, valid, and not landed yet.
    Waiting,
    /// It left its round carrying these changes (docs/adr/0083): its forge
    /// was told (`Forge::sent_back`), and its author merges them into it,
    /// fixes it and asks again.
    Carrying(Vec<u64>),
    /// It is carried by the change named: closed once that change's head
    /// holds its own, it lands with it.
    Carried(u64),
    /// It is ripe, and the push that would land it was refused: what git
    /// said. Its candidate stands, green, for the next run to publish.
    Unpublished(String),
}

/// A change assigned its candidate, by the candidate's id; or why not.
pub type Assigned<R> = Result<String, Outcome<R>>;
/// The changes a publication landed, each with what its forge said when told.
pub type Told<E> = Vec<(u64, Result<(), E>)>;
/// What became of each change, by its number.
pub type Outcomes<R> = Vec<(u64, Outcome<R>)>;
/// What became of each change one round decided, the changes that left it
/// for the next, and whether the round was a stacked one discarded, or one
/// to build again on a target another host moved, which decided nothing of
/// them.
type Rounded<R> = (Outcomes<R>, Vec<Change>, bool);

/// How long a stacked round waits between reads of the round it stands on.
const STACKED_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// A change's place in a round: the change as its forge has it, the place's
/// id, and the head it asks to land.
struct Place {
    change: Change,
    id: String,
    head: String,
}

/// What a publication came to: the changes it landed, each with what its
/// forge said when told; or the push that would land them refused, with
/// the changes it would have landed and what git said.
enum Published<E> {
    Landed(Told<E>),
    Refused(Vec<u64>, String),
}

/// One program's landing on one remote, by plain git.
pub struct Lander<'a, F: Forge> {
    /// A checkout of the repo: the candidates are made in its object store,
    /// and its remote holds the target, the changes' sources and the gate's
    /// records. Only a publication changes the target (docs/adr/0117).
    pub git: Git,
    /// Where this host keeps the candidate chains of the repo: a bare repo
    /// the lander makes there where there is none, one per repo, which every
    /// process and checkout of the host landing on the repo names. Each
    /// target's chain is a commit at `refs/convoy/held/<target>` there, and
    /// at the same ref of the checkout, whose parents keep the candidates
    /// reachable; it is written as a fast-forward of the commit it was read
    /// at, so that git's ref update is the lock.
    pub chains: PathBuf,
    pub forge: &'a F,
    /// The host name a gate run is recorded as having run on.
    pub host: String,
    /// The kind of machine it is (`afkconvoy_gate::gate::machine`): a green
    /// run on record of a candidate's tree on this kind answers for its
    /// gate, which does not run again.
    pub machine: String,
    /// This process, which owns every candidate it assigns and every one
    /// whose gate it runs.
    pub owner: Process,
    /// Whether a process of this host, as an owner on the chain names it,
    /// still runs: its pid, under the start time it is named with. The
    /// program names its processes (`owner`) and reads them back by the same
    /// rule. A candidate whose owner runs no more is taken over
    /// (`Chain::take_over`); one of another host is left to the lease.
    pub running: fn(&Process) -> bool,
    /// Where each candidate's worktree is made, one directory each.
    pub worktrees: PathBuf,
    /// What the commit named declares of its gate, read in the checkout
    /// named.
    pub declared: fn(&Path, &str) -> Result<Declared, String>,
    /// The time now, in epoch seconds.
    pub now: fn() -> u64,
}

/// The chain of one target as read: its record's commit on the host, the
/// target's tip on the remote, and the chain.
struct Read<N> {
    sha: Option<String>,
    base: String,
    chain: Chain<N>,
}

/// One git call in `dir` that must succeed → its stdout, trimmed.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_in(dir, args, None)
}

/// Whether `head` holds `other`: `other` is `head` or an ancestor of it.
fn contains(dir: &Path, head: &str, other: &str) -> Result<bool, String> {
    let out = Command::new("git")
        .current_dir(dir)
        .args(["merge-base", "--is-ancestor", other, head])
        .output()
        .map_err(|error| error.to_string())?;
    match out.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!("git merge-base: {}", String::from_utf8_lossy(&out.stderr).trim())),
    }
}

/// One git call in `dir`, given `input`, that must succeed → its stdout,
/// trimmed.
fn git_in(dir: &Path, args: &[&str], input: Option<&str>) -> Result<String, String> {
    use std::io::Write;
    let mut child = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| format!("git {} could not run: {error}", args.join(" ")))?;
    if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
        stdin.write_all(input.as_bytes()).map_err(|error| error.to_string())?;
    }
    let out = child.wait_with_output().map_err(|error| error.to_string())?;
    if !out.status.success() {
        return Err(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// `git commit-tree` of `tree` on `parents`, its `message` on stdin and
/// ended by a newline, as `-m` ends it: no message is too long for git to
/// start, as one in its arguments can be (sunfmin/control#1267).
fn commit_tree(dir: &Path, tree: &str, parents: &[&str], message: &str) -> Result<String, String> {
    let mut args = vec!["commit-tree", tree];
    for parent in parents {
        args.extend(["-p", parent]);
    }
    args.extend(["-F", "-"]);
    git_in(dir, &args, Some(&format!("{message}\n")))
}

impl<F> Lander<'_, F>
where
    F: Forge,
    F::Note: Clone + PartialEq + Serialize + DeserializeOwned,
{
    fn dir(&self) -> &Path {
        &self.git.worktree
    }

    /// The ref the chain of `target` is kept at, in the host's store of
    /// chains and in the checkout.
    fn held_ref(target: &str) -> String {
        format!("refs/convoy/held/{target}")
    }

    /// The host's store of chains, made where there is none yet.
    fn store(&self) -> Result<String, String> {
        let store = self.chains.to_string_lossy().into_owned();
        if !self.chains.join("HEAD").exists() {
            std::fs::create_dir_all(&self.chains).map_err(|error| error.to_string())?;
            git(&self.chains, &["init", "--quiet", "--bare"])?;
        }
        Ok(store)
    }

    /// The sha the remote has at `name`, fetched into the checkout; `None`
    /// where it has none.
    fn fetched(&self, name: &str) -> Result<Option<String>, String> {
        self.fetched_from(&self.git.remote, name)
    }

    /// The sha `from` has at `name`, fetched into the checkout; `None`
    /// where it has none.
    fn fetched_from(&self, from: &str, name: &str) -> Result<Option<String>, String> {
        let listed = git(self.dir(), &["ls-remote", from, name])?;
        let Some(sha) = listed
            .lines()
            .find_map(|line| line.split_once('\t').filter(|(_, n)| *n == name))
            .map(|(sha, _)| sha.to_string())
        else {
            return Ok(None);
        };
        if git(self.dir(), &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_err() {
            git(self.dir(), &["fetch", "--quiet", "--no-tags", from, name])?;
        }
        Ok(Some(sha))
    }

    fn read(&self, target: &str) -> Result<Read<F::Note>, String> {
        let base = self
            .fetched(&format!("refs/heads/{target}"))?
            .ok_or_else(|| format!("the remote has no branch {target:?}"))?;
        let sha = self.fetched_from(&self.store()?, &Self::held_ref(target))?;
        let chain = match &sha {
            Some(sha) => {
                let message = git(self.dir(), &["show", "-s", "--format=%B", sha])?;
                serde_json::from_str(&message).map_err(|error| error.to_string())?
            }
            None => Chain::default(),
        };
        Ok(Read { sha, base, chain })
    }

    /// The record of `chain` as a commit on the record `sha`: its first
    /// parent the record before, and each candidate's commit a parent, so
    /// the record keeps them reachable. An ended candidate whose worktree is
    /// gone and whose owner runs no more is not carried on past the record
    /// it ended in: its worktree goes as its gate run ends
    /// (sunfmin/control#1063), and the record it ended in still says how.
    fn commit(&self, sha: Option<&str>, chain: &Chain<F::Note>) -> Result<String, String> {
        let before: Option<Chain<F::Note>> = match sha {
            Some(sha) => {
                let text = git(self.dir(), &["show", "-s", "--format=%B", sha])?;
                serde_json::from_str(&text).ok()
            }
            None => None,
        };
        let ended_before = |id: &str| {
            before.as_ref().is_some_and(|b| b.candidates.iter().any(|c| c.id == id && !c.active()))
        };
        // A process that runs still holds what it owns: its turn may yet
        // read what became of it.
        let held = |c: &Candidate<F::Note>| c.owner == self.owner || (self.running)(&c.owner);
        let mut chain = chain.clone();
        chain.prune(|c| {
            c.stopped && !Path::new(&c.worktree).exists() && ended_before(&c.id) && !held(c)
        });
        let text = serde_json::to_string(&chain).map_err(|error| error.to_string())?;
        let tree = git(self.dir(), &["hash-object", "-t", "tree", "/dev/null"])?;
        let mut args = vec!["commit-tree", &tree, "-F", "-"];
        if let Some(sha) = sha {
            args.extend(["-p", sha]);
        }
        for c in &chain.candidates {
            args.extend(["-p", &c.commit]);
        }
        git_in(self.dir(), &args, Some(&text))
    }

    /// Writes `chain` over the record `sha` in the host's store, as a
    /// fast-forward of it alone → whether it took: another writer's record
    /// is not one this extends. The checkout's own ref follows, keeping the
    /// candidates reachable in its object store.
    fn write(
        &self,
        target: &str,
        sha: Option<&str>,
        chain: &Chain<F::Note>,
    ) -> Result<bool, String> {
        let next = self.commit(sha, chain)?;
        let name = Self::held_ref(target);
        let store = self.store()?;
        let pushed = git(self.dir(), &["push", "--quiet", &store, &format!("{next}:{name}")]);
        match pushed {
            Ok(_) => {
                // Moved only forward: a later record another writer of this
                // checkout set is not taken back.
                let local = git(self.dir(), &["rev-parse", "--verify", "--quiet", &name]).ok();
                if local.is_none_or(|l| contains(self.dir(), &next, &l).unwrap_or(false)) {
                    git(self.dir(), &["update-ref", &name, &next])?;
                }
                Ok(true)
            }
            Err(_) if self.fetched_from(&store, &name)?.as_deref() != sha => Ok(false),
            Err(said) => Err(said),
        }
    }

    /// Takes over in `chain` what a process of this host owned and that runs
    /// no more (`Chain::take_over`): this process runs.
    fn take_over(&self, chain: &mut Chain<F::Note>) {
        let mut gone: Vec<Process> = vec![];
        for p in chain.processes(&self.owner.host) {
            if p != self.owner && !(self.running)(&p) {
                gone.push(p);
            }
        }
        chain.take_over(&gone);
    }

    /// Changes the chain of `target` by `edit`, from the record as it is →
    /// the chain written.
    fn change(
        &self,
        target: &str,
        mut edit: impl FnMut(&mut Chain<F::Note>),
    ) -> Result<Chain<F::Note>, String> {
        loop {
            let mut read = self.read(target)?;
            let before = read.chain.clone();
            edit(&mut read.chain);
            if before == read.chain || self.write(target, read.sha.as_deref(), &read.chain)? {
                return Ok(read.chain);
            }
        }
    }

    /// Assigns `change` a candidate: its source's head merged onto the
    /// chain's last valid candidate, or the target's tip, as one merge
    /// commit → the candidate's id. Where the chain already holds a valid
    /// candidate of it merged from its head now — one an earlier run
    /// assigned and did not land — that one is the change's, and no second
    /// is assigned (`Chain::reuse`); one merged from a head it has moved
    /// from is invalidated first. What a process of this host that runs no
    /// more left gating is taken over first (`Chain::take_over`).
    pub fn assign(&self, change: &Change) -> Result<Assigned<F::Refusal>, Error<F::Error>> {
        self.assign_as(change, None)
    }

    /// Assigns `change` a candidate; for the round's root where `ask` names
    /// its place in the round (`Chain::root_assigned`): one not gated alone,
    /// with no gate process to stop.
    fn assign_as(
        &self,
        change: &Change,
        ask: Option<&str>,
    ) -> Result<Assigned<F::Refusal>, Error<F::Error>> {
        let batch = ask.is_some();
        let Some(change) = self.forge.current(change).map_err(Error::Forge)? else {
            return Ok(Err(Outcome::Gone));
        };
        if let Err(refusal) = self.forge.may_land(&change).map_err(Error::Forge)? {
            return Ok(Err(Outcome::Refused(refusal)));
        }
        // A source the remote has not is this change's outcome alone: the
        // round goes on with the others.
        let Some(original) = self.fetched(&source_ref(&change.source))? else {
            return Ok(Err(Outcome::Gone));
        };
        loop {
            let mut read = self.read(&change.target)?;
            let before = read.chain.clone();
            read.chain.observe_base(&read.base);
            self.take_over(&mut read.chain);
            if let Some(id) = read.chain.reuse(change.number, &original, batch) {
                // Taken up for a round, it is this run's to carry on.
                if let Some(ask) = ask {
                    read.chain.root_assigned(ask, &id);
                    for c in read.chain.candidates.iter_mut().filter(|c| c.id == id) {
                        c.owner = self.owner.clone();
                    }
                }
                if before == read.chain
                    || self.write(&change.target, read.sha.as_deref(), &read.chain)?
                {
                    return Ok(Ok(id));
                }
                continue;
            }
            let predecessor = read.chain.predecessor(&read.base);
            let merged = Command::new("git")
                .current_dir(self.dir())
                .args(["merge-tree", "--write-tree", &predecessor, &original])
                .output()
                .map_err(|error| error.to_string())?;
            if !merged.status.success() {
                return Ok(Err(Outcome::Conflict));
            }
            let tree = String::from_utf8_lossy(&merged.stdout)
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
            let message = self.forge.message(&change);
            let commit = commit_tree(self.dir(), &tree, &[&predecessor, &original], &message)?;
            let own = (self.declared)(self.dir(), &commit)?.own();
            let id = read.chain.unused_id(format!(
                "{}-{}-{}",
                change.number,
                (self.now)(),
                commit.get(..12).unwrap_or(&commit)
            ));
            let worktree = self.worktrees.join(&id);
            read.chain.assign(Candidate {
                id: id.clone(),
                change: change.number,
                note: self.forge.note(&change),
                branch: change.source.clone(),
                original_head: original.clone(),
                predecessor,
                commit,
                tree,
                command: own.command,
                seconds: own.seconds,
                phase: if batch { Phase::Batched } else { Phase::Building },
                owner: self.owner.clone(),
                gate: None,
                descendants: vec![],
                worktree: worktree.to_string_lossy().into_owned(),
                barrier: false,
                cancelled_by: None,
                cause: None,
                stopped: batch,
                abandoned: false,
                result: None,
                slow: own.slow,
                slowed: false,
                batch,
                pair: None,
                assigned: Some((self.now)()),
            });
            if let Some(ask) = ask {
                read.chain.root_assigned(ask, &id);
            }
            if self.write(&change.target, read.sha.as_deref(), &read.chain)? {
                return Ok(Ok(id));
            }
        }
    }

    /// Runs `command` in the worktree of `c` under its limit of `seconds`,
    /// recording its run → its evidence. Where the remote holds a green run
    /// of it on the candidate's tree on this kind of machine, that run
    /// answers and nothing runs: the evidence names it (`Evidence::reused`).
    fn run(&self, c: &Candidate<F::Note>, command: &str, seconds: u64) -> Result<Evidence, String> {
        let on = (self.host.as_str(), self.machine.as_str());
        gate_run(&self.git, on, (self.now)(), (&c.worktree, command, seconds))
    }

    /// Runs the candidate `id`'s own gate on its commit, in a worktree of
    /// its own, and lets the chain decide it; where it is still building:
    /// one gating, or gated, is left as it is. This process owns it while
    /// its gate runs.
    pub fn gate(&self, target: &str, id: &str) -> Result<Chain<F::Note>, String> {
        let mut started = false;
        let chain =
            self.change(target, |chain| {
                started = false;
                for c in chain.candidates.iter_mut().filter(|c| {
                    c.id == id && c.phase == Phase::Building && c.cancelled_by.is_none()
                }) {
                    c.phase = Phase::Validating;
                    c.owner = self.owner.clone();
                    started = true;
                }
            })?;
        let c = chain.candidates.iter().find(|c| c.id == id).ok_or("no such candidate")?;
        if !started {
            return Ok(chain);
        }
        self.add_worktree(c)?;
        let evidence = self.run(c, &c.command, c.seconds);
        let evidence = match evidence {
            Ok(evidence) => evidence,
            Err(said) => {
                self.change(target, |chain| chain.invalidate(id, "gate_interrupted"))?;
                return Err(said);
            }
        };
        let chain = self.change(target, |chain| {
            chain.finish(id, evidence.clone());
            stop(chain, id);
        })?;
        self.remove_ended(&chain);
        Ok(chain)
    }

    /// Runs the round at `id` when it is the round's tip: the slow gate,
    /// once, on its commit, in a worktree added again where its fast gate's
    /// was; green, the whole gate is put on record for its tree from its two
    /// parts → whether it ran.
    pub fn round(&self, target: &str, id: &str) -> Result<bool, String> {
        let mut started = false;
        let chain = self.change(target, |chain| {
            started = chain.start_round(id);
            own(chain, id, started, &self.owner);
        })?;
        if !started {
            return Ok(false);
        }
        let tip = chain.candidates.iter().find(|c| c.id == id).ok_or("no such candidate")?;
        self.add_worktree(tip)?;
        let evidence = match self.slow(tip) {
            Ok(evidence) => evidence,
            Err(said) => {
                self.change(target, |chain| chain.invalidate(id, "gate_interrupted"))?;
                return Err(said);
            }
        };
        let chain = self.change(target, |chain| {
            chain.finish(id, evidence.clone());
            stop(chain, id);
        })?;
        self.remove_ended(&chain);
        Ok(true)
    }

    /// Runs the slow gate of `c` on its commit, in its worktree; green, the
    /// whole gate is put on record for its tree from its two parts → its
    /// evidence.
    fn slow(&self, c: &Candidate<F::Note>) -> Result<Evidence, String> {
        let slow = c.slow.clone().ok_or("a round needs a slow gate")?;
        let mut evidence = self.run(c, &slow.command, slow.seconds)?;
        if evidence.green() {
            let parts = (
                &GateCommand::new(&slow.whole),
                &GateCommand::new(&c.command),
                &GateCommand::new(&slow.command),
            );
            let tree = Tree::named(&c.tree);
            let why = run::record_whole(&self.git, &tree, &c.commit, parts, &[], (self.now)())?;
            if let Some(why) = why {
                evidence.recorded = false;
                evidence.not_recorded = Some(why);
            }
        }
        Ok(evidence)
    }

    /// Lands the candidate `id` on `target`, with every member before it,
    /// where it is ripe: as one atomic push that fast-forwards the target
    /// to its commit and records them landed; then tells the forge each
    /// landed → the changes it landed, each with what its forge said of it.
    pub fn publish(
        &self,
        target: &str,
        id: &str,
        changes: &[Change],
    ) -> Result<Told<F::Error>, Error<F::Error>> {
        match self.published(target, id, changes)? {
            Published::Landed(told) => Ok(told),
            Published::Refused(..) => Ok(vec![]),
        }
    }

    /// Lands the candidate `id` as `publish` does → what it came to: where
    /// the push is refused, the changes it would have landed and what git
    /// said; the chain stands as it was read.
    fn published(
        &self,
        target: &str,
        id: &str,
        changes: &[Change],
    ) -> Result<Published<F::Error>, Error<F::Error>> {
        let none = || Ok(Published::Landed(vec![]));
        let read = self.read(target)?;
        if !read.chain.ripe(id, &read.base) {
            return none();
        }
        let members: Vec<&Candidate<F::Note>> = read.chain.members(id);
        let asked = |c: &Candidate<F::Note>| {
            let found = changes.iter().find(|change| change.number == c.change).cloned();
            found.unwrap_or_else(|| Change {
                number: c.change,
                source: c.branch.clone(),
                target: target.to_string(),
                title: String::new(),
            })
        };
        for m in &members {
            if let Err(_refusal) = self.forge.may_land(&asked(m)).map_err(Error::Forge)? {
                let m = m.id.clone();
                self.change(target, |chain| chain.invalidate(&m, "refused"))?;
                return none();
            }
        }
        // What a member carried closes with it, though its author rewrote
        // its description since (sunfmin/control#1248).
        for m in &members {
            forge::described(self.forge, &asked(m)).map_err(Error::Forge)?;
        }
        let last =
            members.last().ok_or_else(|| "a landing takes at least one candidate".to_string())?;
        let whole = GateCommand::new(turn::whole(last));
        let recorded =
            run::recorded(&self.git, &Tree::named(&last.tree), &whole, (self.now)())?.is_some();
        let mut heads = true;
        for m in &members {
            heads &=
                self.fetched(&source_ref(&m.branch))?.as_deref() == Some(m.original_head.as_str());
        }
        let parent = |commit: &str| git(self.dir(), &["rev-parse", &format!("{commit}^1")]);
        let ordered = turn::fast_forwards(&members, &read.base, parent)?;
        let Some(_publication) =
            read.chain.publication(id, &read.base, recorded, heads).filter(|_| ordered)
        else {
            self.change(target, |chain| chain.invalidate(id, "publication_evidence_changed"))?;
            return none();
        };
        let landed: Vec<Candidate<F::Note>> = members.iter().map(|c| (*c).clone()).collect();
        let mut refspecs = vec![];
        for update in turn::publication(&members, target) {
            // A source kept at its head was read so just before; plain git
            // has no condition on a ref it does not change.
            if update.old != update.new {
                refspecs.push(format!("{}:{}", update.new, update.name));
            }
        }
        let mut args = vec!["push", "--quiet", "--atomic", self.git.remote.as_str()];
        args.extend(refspecs.iter().map(String::as_str));
        if let Err(said) = git(self.dir(), &args) {
            return Ok(Published::Refused(landed.iter().map(|c| c.change).collect(), said));
        }
        // The push took: the chain says so. A run that ends before this
        // leaves a candidate whose predecessor is not the target's tip, which
        // the next look cancels; its change is then found on the target.
        self.change(target, |chain| chain.land(id))?;
        let commit = last.commit.clone();
        let told = landed.iter().map(|c| (c.change, self.forge.landed(&asked(c), &commit)));
        Ok(Published::Landed(told.collect()))
    }

    /// Adds the worktree of `c`, detached at its commit, where it is not
    /// there yet: each gate run of it, its own gate or a round's slow gate,
    /// runs in one, which goes as that run ends.
    fn add_worktree(&self, c: &Candidate<F::Note>) -> Result<(), String> {
        if !Path::new(&c.worktree).exists() {
            git(self.dir(), &["worktree", "add", "--quiet", "--detach", &c.worktree, &c.commit])?;
        }
        Ok(())
    }

    /// Removes the worktree of each candidate whose gate run has stopped,
    /// whatever its phase: landed, red, cancelled, or green and waiting for
    /// its round or its predecessor (sunfmin/control#1063).
    fn remove_ended(&self, chain: &Chain<F::Note>) {
        for c in &chain.candidates {
            if c.stopped && Path::new(&c.worktree).exists() {
                let _ = git(self.dir(), &["worktree", "remove", "--force", &c.worktree]);
            }
        }
    }

    /// Lands `changes`, which ask to land on one target, as this process
    /// alone → what became of each. They are taken into a round in the order
    /// given (docs/adr/0083): paired, each pair fast-gated, the pairs of one
    /// level at once; the root merged onto the target's tip, one candidate
    /// per change, and gated whole once at the last; green, the round lands
    /// as one fast-forward. A change that left the round for the next is
    /// taken into the next, while each round decides one change or more, or
    /// was a stacked round discarded as the round it stood on ended
    /// otherwise than landed, or a round whose publication another host's
    /// move of the target refused, built again on the new tip.
    pub fn land(&self, changes: &[Change]) -> Result<Outcomes<F::Refusal>, Error<F::Error>> {
        let Some(target) = changes.first().map(|c| c.target.clone()) else {
            return Ok(vec![]);
        };
        self.change(&target, |chain| self.take_over(chain))?;
        let mut outcomes: Outcomes<F::Refusal> = vec![];
        let mut refused: Refusals = vec![];
        let mut asking: Vec<Change> = changes.to_vec();
        while !asking.is_empty() {
            let before = asking.len();
            let (decided, next, again) =
                self.land_round(&target, &asking, changes, &mut refused)?;
            outcomes.extend(decided);
            asking = next;
            if asking.len() >= before && !again {
                break;
            }
        }
        outcomes.extend(asking.into_iter().map(|change| (change.number, Outcome::Waiting)));
        outcomes.sort_by_key(|(number, _)| {
            changes.iter().position(|c| c.number == *number).unwrap_or(usize::MAX)
        });
        Ok(outcomes)
    }

    /// Lands `asking` on `target` as one round → what became of each change
    /// the round decided, and the changes that left it for the next.
    fn land_round(
        &self,
        target: &str,
        asking: &[Change],
        changes: &[Change],
        refused: &mut Refusals,
    ) -> Result<Rounded<F::Refusal>, Error<F::Error>> {
        let mut outcomes: Outcomes<F::Refusal> = vec![];
        let places = self.places(asking, &mut outcomes)?;
        let Some(first) = places.first() else { return Ok((outcomes, vec![], false)) };
        let round = format!("round-{}-{}", (self.now)(), first.change.number);
        let ids: Vec<String> = places.iter().map(|p| p.id.clone()).collect();
        if !self.open(target, &places, &round)? {
            // Another round is open, or a barrier holds the chain: nothing ran.
            self.change(target, |chain| chain.pending.retain(|b| !ids.contains(&b.id)))?;
            outcomes.extend(places.iter().map(|p| (p.change.number, Outcome::Waiting)));
            return Ok((outcomes, vec![], false));
        }
        let mut rebuilt = false;
        loop {
            match self.step(target, &round, &places, changes, refused, &mut outcomes)? {
                Stepped::More => {}
                Stepped::Done => break,
                Stepped::Rebuilt => {
                    rebuilt = true;
                    break;
                }
            }
        }
        let chain = self.change(target, |_| {})?;
        let held = chain.rounds().any(|r| r.id == round);
        if !held && chain.pending.iter().any(|b| ids.contains(&b.id)) {
            // A stacked round discarded: its changes pair in the next round.
            self.change(target, |chain| chain.pending.retain(|b| !ids.contains(&b.id)))?;
            return Ok((outcomes, places.into_iter().map(|p| p.change).collect(), true));
        }
        let mut next = vec![];
        for place in &places {
            match self.decided(&chain, place, refused)? {
                Some(outcome) => outcomes.push((place.change.number, outcome)),
                None => next.push(place.change.clone()),
            }
        }
        let chain = self.change(target, |chain| chain.leave_round(|a| ids.contains(&a.id)))?;
        self.remove_ended(&chain);
        Ok((outcomes, next, rebuilt))
    }

    /// The places `asking` takes in a round, each change as its forge has
    /// it now and its head; what became of one that may not land is put in
    /// `outcomes`.
    fn places(
        &self,
        asking: &[Change],
        outcomes: &mut Outcomes<F::Refusal>,
    ) -> Result<Vec<Place>, Error<F::Error>> {
        let mut places: Vec<Place> = vec![];
        for change in asking {
            let Some(current) = self.forge.current(change).map_err(Error::Forge)? else {
                outcomes.push((change.number, Outcome::Gone));
                continue;
            };
            if let Err(refusal) = self.forge.may_land(&current).map_err(Error::Forge)? {
                outcomes.push((change.number, Outcome::Refused(refusal)));
                continue;
            }
            // A source the remote has not is this change's outcome alone: the
            // round goes on with the others.
            let Some(head) = self.fetched(&source_ref(&current.source))? else {
                outcomes.push((change.number, Outcome::Gone));
                continue;
            };
            let id = format!("{}-{}-asks", current.number, (self.now)());
            places.push(Place { change: current, id, head });
        }
        self.carry_held(&mut places, outcomes)?;
        Ok(places)
    }

    /// Carries each change of `places` whose head another's holds, and the
    /// target does not (docs/adr/0083): that other change carries it
    /// (`forge::carry`), and it leaves `places`, its outcome `Carried`.
    fn carry_held(
        &self,
        places: &mut Vec<Place>,
        outcomes: &mut Outcomes<F::Refusal>,
    ) -> Result<(), Error<F::Error>> {
        let Some(target) = places.first().map(|p| p.change.target.clone()) else { return Ok(()) };
        let tip = self.fetched(&format!("refs/heads/{target}"))?;
        let mut carried: Vec<(usize, usize)> = vec![];
        for (b, held) in places.iter().enumerate() {
            if tip
                .as_deref()
                .is_some_and(|tip| contains(self.dir(), tip, &held.head).unwrap_or(false))
            {
                continue;
            }
            for (a, by) in places.iter().enumerate() {
                let taken = carried.iter().any(|&(x, y)| x == b || y == b || y == a);
                if a != b
                    && !taken
                    && by.head != held.head
                    && contains(self.dir(), &by.head, &held.head)?
                {
                    carried.push((a, b));
                    break;
                }
            }
        }
        for &(a, b) in &carried {
            let (Some(by), Some(held)) = (places.get(a), places.get(b)) else { continue };
            forge::carry(self.forge, &by.change, &held.change, &held.head).map_err(Error::Forge)?;
            outcomes.push((held.change.number, Outcome::Carried(by.change.number)));
        }
        let gone: Vec<usize> = carried.iter().map(|&(_, b)| b).collect();
        let mut kept = vec![];
        for (at, place) in std::mem::take(places).into_iter().enumerate() {
            if !gone.contains(&at) {
                kept.push(place);
            }
        }
        *places = kept;
        Ok(())
    }

    /// Takes `places` pending and opens the round `round` of them, or
    /// stacks it on the round whose whole gate runs → whether it did.
    fn open(&self, target: &str, places: &[Place], round: &str) -> Result<bool, String> {
        let mut opened = false;
        self.change(target, |chain| {
            for place in places {
                if !chain.pending.iter().any(|b| b.id == place.id) {
                    chain.pending.push(Barrier {
                        id: place.id.clone(),
                        owner: self.owner.clone(),
                        since: Some((self.now)()),
                        asks: Some(Asks {
                            change: place.change.number,
                            branch: place.change.source.clone(),
                            head: place.head.clone(),
                            carried_by: None,
                        }),
                    });
                }
            }
            opened = chain.open_round(round);
        })?;
        Ok(opened)
    }

    /// Takes the round `round` one step on: pairs a level, gates a level's
    /// pairs, makes a stacked round's stacked merge, assigns a candidate of
    /// its root, gates the root whole, or lands it → what is left to do. A
    /// stacked round with nothing to do waits for the round it
    /// stands on, unless that round's candidate is wedged.
    fn step(
        &self,
        target: &str,
        round: &str,
        places: &[Place],
        changes: &[Change],
        refused: &mut Refusals,
        outcomes: &mut Outcomes<F::Refusal>,
    ) -> Result<Stepped, Error<F::Error>> {
        // No gate of this process runs between its steps.
        let owner = self.owner.clone();
        self.change(target, |chain| {
            chain.stop_unstarted(&owner);
        })?;
        let read = self.read(target)?;
        let chain = &read.chain;
        let Some(open) = chain.rounds().find(|r| r.id == round) else { return Ok(Stepped::Done) };
        if let Some(level) = chain.to_pair().filter(|(id, ..)| id == round) {
            self.pair(target, level)?;
            return Ok(Stepped::More);
        }
        let mut gates: Vec<String> = vec![];
        let mut assign: Option<(String, u64)> = None;
        let mut stack = false;
        for ask in &open.asked {
            match chain.round_next(&ask.id) {
                Some(RoundNext::Gate(pair)) => gates.push(pair),
                Some(RoundNext::Assign) if assign.is_none() => {
                    assign = Some((ask.id.clone(), ask.asks.change));
                }
                Some(RoundNext::Stack) => stack = true,
                _ => {}
            }
        }
        if !gates.is_empty() {
            self.gate_pairs(target, &gates)?;
            return Ok(Stepped::More);
        }
        if stack {
            self.stack_root(target)?;
            return Ok(Stepped::More);
        }
        if let Some((ask, number)) = assign {
            self.assign_root(target, (&ask, number), places, outcomes)?;
            return Ok(Stepped::More);
        }
        if let Some(on) = &open.on {
            let stood = chain.candidates.iter().find(|c| c.id == on.candidate);
            if stood.is_some_and(|c| c.wedged((self.now)())) {
                return Ok(Stepped::Done);
            }
            std::thread::sleep(STACKED_WAIT);
            return Ok(Stepped::More);
        }
        if let Some(tip) = chain.whole_tip() {
            let tip = tip.id.clone();
            self.gate_whole(target, &tip)?;
            return Ok(Stepped::More);
        }
        if self.moved_under(chain, open, &read.base) {
            return self.rebuild(target, open, &read.base, refused);
        }
        let mut ripe: Option<String> = None;
        for c in chain.chained().into_iter().rev() {
            if ripe.is_none() && chain.ripe(&c.id, &read.base) {
                ripe = Some(c.id.clone());
            }
        }
        let Some(ripe) = ripe else { return Ok(Stepped::Done) };
        if self.publish_noting(target, &ripe, changes, refused)?.is_empty() {
            let again = self.read(target)?;
            if self.moved_under(&again.chain, open, &again.base) {
                return self.rebuild(target, open, &again.base, refused);
            }
            return Ok(if again.chain.ripe(&ripe, &again.base) {
                Stepped::Done
            } else {
                Stepped::More
            });
        }
        Ok(Stepped::More)
    }

    /// Whether the target moved, outside the chain, from under the round
    /// `open`: the first valid candidate of the chain is the round's, and
    /// stands on another commit than `base`, the target's tip.
    fn moved_under(&self, chain: &Chain<F::Note>, open: &pairing::Round, base: &str) -> bool {
        let first = chain.chained().into_iter().find(|c| c.valid());
        first.is_some_and(|c| {
            c.predecessor != base && open.asked.iter().any(|a| a.candidate.as_ref() == Some(&c.id))
        })
    }

    /// Another host moved the target to `base` from under the round
    /// `open`: its candidates are cancelled `base_moved`, and its changes
    /// leave it to ask anew, built again on the new tip and gated again
    /// (docs/adr/0117); a push of it refused is not their outcome.
    fn rebuild(
        &self,
        target: &str,
        open: &pairing::Round,
        base: &str,
        refused: &mut Refusals,
    ) -> Result<Stepped, Error<F::Error>> {
        let asked: Vec<(String, u64)> =
            open.asked.iter().map(|a| (a.id.clone(), a.asks.change)).collect();
        self.change(target, |chain| {
            chain.observe_base(base);
            for (id, _) in &asked {
                chain.leave(id, Left::Next);
            }
        })?;
        refused.retain(|(number, _)| !asked.iter().any(|(_, n)| n == number));
        Ok(Stepped::Rebuilt)
    }

    /// Assigns the candidate of the round's root of the change `number`,
    /// whose place is `ask`: a conflict leaves the round; a change that may
    /// no longer land leaves it, its outcome put in `outcomes`.
    fn assign_root(
        &self,
        target: &str,
        (ask, number): (&str, u64),
        places: &[Place],
        outcomes: &mut Outcomes<F::Refusal>,
    ) -> Result<(), Error<F::Error>> {
        let Some(place) = places.iter().find(|p| p.change.number == number) else {
            self.change(target, |chain| chain.leave_round(|a| a.id == ask))?;
            return Ok(());
        };
        match self.assign_as(&place.change, Some(ask))? {
            Ok(_) => {}
            Err(Outcome::Conflict) => {
                self.change(target, |chain| chain.leave(ask, Left::Conflict))?;
            }
            Err(outcome) => {
                outcomes.push((number, outcome));
                self.change(target, |chain| chain.leave_round(|a| a.id == ask))?;
            }
        }
        Ok(())
    }

    /// What became of the change of `place` in its round, as `chain` says
    /// it → `None` where it left the round for the next. A change sent
    /// back from a red pair is told to its forge with that pair's run.
    fn decided(
        &self,
        chain: &Chain<F::Note>,
        place: &Place,
        refused: &Refusals,
    ) -> Result<Option<Outcome<F::Refusal>>, Error<F::Error>> {
        let asked = chain.rounds().flat_map(|r| &r.asked).find(|a| a.id == place.id);
        let Some(ask) = asked else { return Ok(Some(Outcome::Waiting)) };
        let outcome = match (&ask.left, &ask.candidate) {
            (Some(Left::Red(pair)), _) => {
                let red = chain.candidates.iter().find(|c| &c.id == pair);
                let run = red.and_then(|c| c.result.as_ref());
                let sent = SentBack { run, merge: &[], carried: &[] };
                self.forge.sent_back(&place.change, &sent).map_err(Error::Forge)?;
                Outcome::Decided(Decision::Red)
            }
            (Some(Left::Carrying(carry)), _) => {
                let red = chain.candidates.iter().find(|c| Some(&c.id) == carry.pair.as_ref());
                let run = red.and_then(|c| c.result.as_ref());
                let sent = SentBack { run, merge: &carry.merge, carried: &carry.carried };
                self.forge.sent_back(&place.change, &sent).map_err(Error::Forge)?;
                Outcome::Carrying(carry.carried.clone())
            }
            (Some(Left::Carried(by)), _) => Outcome::Carried(*by),
            (Some(Left::Conflict), _) => Outcome::Conflict,
            (Some(Left::Unpaired | Left::Next), _) => return Ok(None),
            (None, Some(candidate)) => match chain.candidates.iter().find(|c| &c.id == candidate) {
                Some(c) if c.phase == Phase::Landed => Outcome::Landed(c.commit.clone()),
                // A red root is fixed on the spot by the round's last-asked
                // change, which carries every other (docs/adr/0083).
                Some(c) => match (chain.root_carry(&c.id), chain.round_carrier(c)) {
                    (Some(carry), _) => {
                        let run = c.result.as_ref();
                        let sent = SentBack { run, merge: &carry.merge, carried: &carry.carried };
                        self.forge.sent_back(&place.change, &sent).map_err(Error::Forge)?;
                        Outcome::Carrying(carry.carried)
                    }
                    (None, Some(by)) => Outcome::Carried(by),
                    (None, None) => c.decision().map_or(Outcome::Waiting, Outcome::Decided),
                },
                None => Outcome::Waiting,
            },
            (None, None) => Outcome::Waiting,
        };
        let said = refused.iter().find(|(n, _)| *n == place.change.number).map(|(_, said)| said);
        Ok(Some(match (outcome, said) {
            (Outcome::Waiting, Some(said)) => Outcome::Unpublished(said.clone()),
            (outcome, _) => outcome,
        }))
    }

    /// Groups the level `level` of the round, as `Chain::to_pair` names it
    /// (`pairing::groups`): each group its nodes merged one after another,
    /// each onto the merge before, where `git merge-tree` finds it clean, as
    /// a commit of its own that its fast gate runs on.
    fn pair(
        &self,
        target: &str,
        (round, level, nodes): (String, u32, Vec<pairing::Node>),
    ) -> Result<(), String> {
        let named = |changes: &[u64]| {
            changes.iter().map(|n| format!("!{n}")).collect::<Vec<_>>().join(", ")
        };
        let made = pairing::groups(nodes.len(), |held: Option<&String>, at: &[usize], node| {
            let (Some(first), Some(next)) =
                (at.first().and_then(|&i| nodes.get(i)), nodes.get(node))
            else {
                return Ok(None);
            };
            let onto = held.unwrap_or(&first.commit);
            let Some(tree) = merged_tree(self.dir(), onto, &next.commit)? else { return Ok(None) };
            let mut changes: Vec<u64> = vec![];
            for &i in at {
                changes.extend(nodes.get(i).iter().flat_map(|n| &n.changes));
            }
            let message = format!("Group {} with {}", named(&changes), named(&next.changes));
            let commit = commit_tree(self.dir(), &tree, &[onto, &next.commit], &message)?;
            Ok::<_, String>(Some(commit))
        })?;
        let mut built = vec![];
        for (at, commit) in made.groups {
            let tree = git(self.dir(), &["rev-parse", &format!("{commit}^{{tree}}")])?;
            let own = (self.declared)(self.dir(), &commit)?.own();
            built.push(Built {
                nodes: at,
                commit,
                tree,
                command: own.command,
                seconds: own.seconds,
            });
        }
        let note = |number: u64| {
            self.forge.note(&Change {
                number,
                source: String::new(),
                target: target.to_string(),
                title: String::new(),
            })
        };
        let worktree = |id: &str| self.worktrees.join(id).to_string_lossy().into_owned();
        let now = (self.now)();
        self.change(target, |chain| {
            chain.pair_level((&round, level), (built.clone(), made.carry), note, worktree, now);
        })?;
        Ok(())
    }

    /// Makes the stacked round's stacked merge, as `Chain::to_stack` names
    /// it: its root's heads merged one after another, in the order they
    /// asked, onto the commit it stands on, each a merge commit as its
    /// candidate of the root will be, so the last has the tree its root's
    /// last candidate will have on that commit as the tip. A conflict makes
    /// none.
    fn stack_root(&self, target: &str) -> Result<(), String> {
        let Some((round, on, heads)) = self.read(target)?.chain.to_stack() else { return Ok(()) };
        let mut merged = Some((on.clone(), String::new()));
        for head in &heads {
            let Some((at, _)) = merged.take() else { break };
            let Some(tree) = merged_tree(self.dir(), &at, &head.head)? else { break };
            let message = format!("Stack !{} onto {}", head.change, on.get(..12).unwrap_or(&on));
            let commit = commit_tree(self.dir(), &tree, &[&at, &head.head], &message)?;
            merged = Some((commit, tree));
        }
        let made = match merged {
            Some((commit, tree)) => {
                let own = (self.declared)(self.dir(), &commit)?.own();
                Some(Merged { commit, tree, command: own.command, seconds: own.seconds })
            }
            None => None,
        };
        let note = |number: u64| {
            self.forge.note(&Change {
                number,
                source: String::new(),
                target: target.to_string(),
                title: String::new(),
            })
        };
        let worktree = |id: &str| self.worktrees.join(id).to_string_lossy().into_owned();
        let now = (self.now)();
        self.change(target, |chain| {
            chain.stack_merge(&round, made.clone(), note, worktree, now);
        })?;
        Ok(())
    }

    /// Gates the pairs `ids`, each in a worktree of its own, all at once,
    /// and lets the round carry on from what each said. This process owns
    /// each while its gate runs.
    fn gate_pairs(&self, target: &str, ids: &[String]) -> Result<(), String> {
        let mut started: Vec<String> = vec![];
        let chain = self.change(target, |chain| {
            started.clear();
            for c in chain
                .candidates
                .iter_mut()
                .filter(|c| ids.contains(&c.id) && c.phase == Phase::Building && c.valid())
            {
                c.phase = Phase::Validating;
                c.owner = self.owner.clone();
                started.push(c.id.clone());
            }
        })?;
        let mut runs: Vec<(String, String, String, u64)> = vec![];
        for c in chain.candidates.iter().filter(|c| started.contains(&c.id)) {
            self.add_worktree(c)?;
            runs.push((c.id.clone(), c.worktree.clone(), c.command.clone(), c.seconds));
        }
        let on = (self.host.as_str(), self.machine.as_str());
        let now = (self.now)();
        let git = &self.git;
        let ran: Vec<(String, Result<Evidence, String>)> = std::thread::scope(|scope| {
            let running: Vec<_> = runs
                .iter()
                .map(|(id, worktree, command, seconds)| {
                    scope.spawn(move || {
                        (id.clone(), gate_run(git, on, now, (worktree, command, *seconds)))
                    })
                })
                .collect();
            running.into_iter().filter_map(|run| run.join().ok()).collect()
        });
        let mut failed = None;
        for (id, evidence) in ran {
            let chain = self.change(target, |chain| {
                match &evidence {
                    Ok(evidence) => chain.finish(&id, evidence.clone()),
                    Err(_) => chain.invalidate(&id, "gate_interrupted"),
                }
                stop(chain, &id);
                chain.advance_round();
            })?;
            self.remove_ended(&chain);
            failed = failed.or(evidence.err());
        }
        failed.map_or(Ok(()), Err)
    }

    /// Runs the round's whole gate at `id`, its root's last candidate, where
    /// it is the round's tip (`Chain::whole_tip`): the whole gate's green
    /// run on record of its tree, on this kind of machine, answers for it;
    /// otherwise its fast gate and, that green, its slow gate, which puts the
    /// whole gate on record for its tree.
    fn gate_whole(&self, target: &str, id: &str) -> Result<(), String> {
        let mut started = false;
        let chain = self.change(target, |chain| {
            started = chain.start_whole(id);
            own(chain, id, started, &self.owner);
        })?;
        if !started {
            return Ok(());
        }
        let c = chain.candidates.iter().find(|c| c.id == id).ok_or("no such candidate")?.clone();
        let whole = GateCommand::new(turn::whole(&c));
        let tree = Tree::named(&c.tree);
        let evidence = match run::reusable(&self.git, &tree, &whole, &self.machine, (self.now)())? {
            Some(green) => Ok(Evidence {
                command: Some(whole.text().to_string()),
                head: Some(c.commit.clone()),
                recorded: true,
                reused: Some(Reused::of(&green)),
                status: Status::Green,
                ..Evidence::default()
            }),
            None => {
                self.add_worktree(&c)?;
                self.run(&c, &c.command, c.seconds).and_then(|fast| match &c.slow {
                    Some(_) if fast.green() => self.slow(&c),
                    _ => Ok(fast),
                })
            }
        };
        let chain = self.change(target, |chain| {
            match &evidence {
                Ok(evidence) => chain.finish(id, evidence.clone()),
                Err(_) => chain.invalidate(id, "gate_interrupted"),
            }
            stop(chain, id);
        })?;
        self.remove_ended(&chain);
        evidence.map(|_| ())
    }

    /// Publishes the candidate `id` → the changes it landed; a refused push
    /// is noted in `refused`, against each change it would have landed.
    fn publish_noting(
        &self,
        target: &str,
        id: &str,
        changes: &[Change],
        refused: &mut Refusals,
    ) -> Result<Vec<u64>, Error<F::Error>> {
        match self.published(target, id, changes)? {
            Published::Landed(told) => Ok(told.into_iter().map(|(number, _)| number).collect()),
            Published::Refused(numbers, said) => {
                refused.retain(|(number, _)| !numbers.contains(number));
                refused.extend(numbers.into_iter().map(|number| (number, said.clone())));
                Ok(vec![])
            }
        }
    }
}

/// The changes whose publication's push was refused, each with what git
/// said.
type Refusals = Vec<(u64, String)>;

/// What one step of a round left to do.
enum Stepped {
    /// More steps.
    More,
    /// Nothing more of this round, in this run.
    Done,
    /// The round's publication was refused as another host moved the
    /// target: its changes left it to be built again on the new tip.
    Rebuilt,
}

/// The tree of `first` and `second` merged, where `git merge-tree` finds
/// their merge clean; `None` where it conflicts.
fn merged_tree(dir: &Path, first: &str, second: &str) -> Result<Option<String>, String> {
    let merged = Command::new("git")
        .current_dir(dir)
        .args(["merge-tree", "--write-tree", first, second])
        .output()
        .map_err(|error| error.to_string())?;
    if !merged.status.success() {
        return Ok(None);
    }
    let out = String::from_utf8_lossy(&merged.stdout);
    Ok(out.split_whitespace().next().map(str::to_string))
}

/// Runs `command` in `worktree`, a checkout of `git`'s repo, under its limit
/// of `seconds`, on the host and kind of machine `on`, recording its run →
/// its evidence. Where the remote holds a green run of it on the worktree's
/// tree on this kind of machine, that run answers and nothing runs: the
/// evidence names it (`Evidence::reused`).
fn gate_run(
    git: &Git,
    on: (&str, &str),
    now: u64,
    (worktree, command, seconds): (&str, &str, u64),
) -> Result<Evidence, String> {
    let checkout = Git { worktree: PathBuf::from(worktree), ..git.clone() };
    let command = GateCommand::new(command);
    let limit = std::time::Duration::from_secs(seconds);
    let ran = run::gate(&checkout, &command, on, now, limit, &[])?;
    let (written, not_recorded, reused) = match &ran.settled {
        run::Settled::Written { not_recorded, .. } => (true, not_recorded.clone(), None),
        run::Settled::Reused(green) => (false, None, Some(Reused::of(green))),
        run::Settled::Dropped | run::Settled::Left => (false, None, None),
    };
    let skip = ran.output.len().saturating_sub(EXCERPT_LINES);
    let termination = match &ran.judged {
        Judged::Red(Red { ended: Ended::Signalled(termination), .. }) => Some(*termination),
        Judged::Green | Judged::Red(_) | Judged::Unsettled(_) => None,
    };
    Ok(Evidence::of(Observed {
        command: command.text(),
        ran: ran.ran,
        judged: &ran.judged,
        head: ran.head,
        excerpt: ran.output.iter().skip(skip).cloned().collect(),
        strays: ran.strays,
        written,
        not_recorded,
        termination,
        cancelled: false,
        reused,
    }))
}

/// The candidate `id`, whose gate this run `started`, is `owner`'s while it
/// runs.
fn own<N>(chain: &mut Chain<N>, id: &str, started: bool, owner: &Process) {
    for c in chain.candidates.iter_mut().filter(|c| started && c.id == id) {
        c.owner = owner.clone();
    }
}

/// The candidate `id`'s gate has returned: its processes are gone with it,
/// and a cancelled one is cancelled.
fn stop<N>(chain: &mut Chain<N>, id: &str) {
    for c in chain.candidates.iter_mut().filter(|c| c.id == id) {
        c.stopped = true;
        if c.phase == Phase::Cancelling {
            c.phase = Phase::Cancelled;
        }
    }
}
