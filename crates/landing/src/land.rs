//! Landing with plain git (docs/adr/0060), for a program that has no runner
//! of its own: one process assigns its changes' candidates, gates each,
//! runs a round's slow gate once at its tip and lands the round as one
//! atomic fast-forward, on the chain the remote records, through the turn's
//! rules (`turn`) and the gate crate's plain gate runner
//! (`afkconvoy_gate::run`). It asks its program's forge what it must
//! (`Forge`).
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

use crate::chain::{Candidate, Chain, Decision, Evidence, Observed, Phase, Process};
use crate::forge::{Change, Forge};
use crate::turn::{self, Declared, Next};

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
    /// Its forge no longer has it asking to land.
    Gone,
    /// It is on the chain, valid, and not landed yet.
    Waiting,
}

/// A change assigned its candidate, by the candidate's id; or why not.
pub type Assigned<R> = Result<String, Outcome<R>>;
/// The changes a publication landed, each with what its forge said when told.
pub type Told<E> = Vec<(u64, Result<(), E>)>;
/// What became of each change, by its number.
pub type Outcomes<R> = Vec<(u64, Outcome<R>)>;

/// One program's landing on one remote, by plain git.
pub struct Lander<'a, F: Forge> {
    /// A checkout of the repo: the candidates are made in its object store,
    /// and its remote holds the target, the changes' sources, the chain and
    /// the gate's records.
    pub git: Git,
    pub forge: &'a F,
    /// The host name a gate run is recorded as having run on.
    pub host: String,
    /// This process, which owns every candidate it assigns.
    pub owner: Process,
    /// Where each candidate's worktree is made, one directory each.
    pub worktrees: PathBuf,
    /// What the commit named declares of its gate, read in the checkout
    /// named.
    pub declared: fn(&Path, &str) -> Result<Declared, String>,
    /// The time now, in epoch seconds.
    pub now: fn() -> u64,
}

/// The chain of one target as read: its record's commit, the target's tip,
/// and the chain.
struct Read<N> {
    sha: Option<String>,
    base: String,
    chain: Chain<N>,
}

/// One git call in `dir` that must succeed → its stdout, trimmed.
fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    git_in(dir, args, None)
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

impl<F> Lander<'_, F>
where
    F: Forge,
    F::Note: Clone + PartialEq + Serialize + DeserializeOwned,
{
    fn dir(&self) -> &Path {
        &self.git.worktree
    }

    fn record_ref(&self, target: &str) -> String {
        turn::record_ref(self.git.layout, target)
    }

    /// The sha the remote has at `name`, fetched into the checkout; `None`
    /// where it has none.
    fn fetched(&self, name: &str) -> Result<Option<String>, String> {
        let listed = git(self.dir(), &["ls-remote", &self.git.remote, name])?;
        let Some(sha) = listed
            .lines()
            .find_map(|line| line.split_once('\t').filter(|(_, n)| *n == name))
            .map(|(sha, _)| sha.to_string())
        else {
            return Ok(None);
        };
        if git(self.dir(), &["cat-file", "-e", &format!("{sha}^{{commit}}")]).is_err() {
            git(self.dir(), &["fetch", "--quiet", "--no-tags", &self.git.remote, name])?;
        }
        Ok(Some(sha))
    }

    fn read(&self, target: &str) -> Result<Read<F::Note>, String> {
        let base = self
            .fetched(&format!("refs/heads/{target}"))?
            .ok_or_else(|| format!("the remote has no branch {target:?}"))?;
        let sha = self.fetched(&self.record_ref(target))?;
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
    /// gone is not carried on.
    fn commit(&self, sha: Option<&str>, chain: &Chain<F::Note>) -> Result<String, String> {
        let mut chain = chain.clone();
        chain.prune(|c| c.stopped && !Path::new(&c.worktree).exists());
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

    /// Writes `chain` over the record `sha`, as a fast-forward of it alone
    /// → whether it took: another writer's record is not one this extends.
    fn write(
        &self,
        target: &str,
        sha: Option<&str>,
        chain: &Chain<F::Note>,
    ) -> Result<bool, String> {
        let next = self.commit(sha, chain)?;
        let name = self.record_ref(target);
        let pushed =
            git(self.dir(), &["push", "--quiet", &self.git.remote, &format!("{next}:{name}")]);
        match pushed {
            Ok(_) => Ok(true),
            Err(_) if self.fetched(&name)?.as_deref() != sha => Ok(false),
            Err(said) => Err(said),
        }
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
    /// commit → the candidate's id.
    pub fn assign(&self, change: &Change) -> Result<Assigned<F::Refusal>, Error<F::Error>> {
        let Some(change) = self.forge.current(change).map_err(Error::Forge)? else {
            return Ok(Err(Outcome::Gone));
        };
        if let Err(refusal) = self.forge.may_land(&change).map_err(Error::Forge)? {
            return Ok(Err(Outcome::Refused(refusal)));
        }
        let source = format!("refs/heads/{}", change.source);
        let original = self
            .fetched(&source)?
            .ok_or_else(|| format!("the remote has no branch {:?}", change.source))?;
        loop {
            let mut read = self.read(&change.target)?;
            read.chain.observe_base(&read.base);
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
            let commit = git(
                self.dir(),
                &["commit-tree", &tree, "-p", &predecessor, "-p", &original, "-m", &message],
            )?;
            let own = (self.declared)(self.dir(), &commit)?.own();
            let id = format!(
                "{}-{}-{}",
                change.number,
                (self.now)(),
                commit.get(..12).unwrap_or(&commit)
            );
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
                phase: Phase::Building,
                owner: self.owner.clone(),
                gate: None,
                descendants: vec![],
                worktree: worktree.to_string_lossy().into_owned(),
                barrier: false,
                cancelled_by: None,
                cause: None,
                stopped: false,
                abandoned: false,
                result: None,
                slow: own.slow,
                slowed: false,
            });
            if self.write(&change.target, read.sha.as_deref(), &read.chain)? {
                return Ok(Ok(id));
            }
        }
    }

    /// Runs `command` in the worktree of `c` under its limit of `seconds`,
    /// recording its run → its evidence.
    fn run(&self, c: &Candidate<F::Note>, command: &str, seconds: u64) -> Result<Evidence, String> {
        let checkout = Git { worktree: PathBuf::from(&c.worktree), ..self.git.clone() };
        let command = GateCommand::new(command);
        let limit = std::time::Duration::from_secs(seconds);
        let ran = run::gate(&checkout, &command, &self.host, (self.now)(), limit, &[])?;
        let (written, not_recorded) = match ran.settled {
            run::Settled::Written { not_recorded, .. } => (true, not_recorded),
            run::Settled::Dropped | run::Settled::Left => (false, None),
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
        }))
    }

    /// Runs the candidate `id`'s own gate on its commit, in a worktree of
    /// its own, and lets the chain decide it.
    pub fn gate(&self, target: &str, id: &str) -> Result<Chain<F::Note>, String> {
        let chain = self.change(target, |chain| {
            for c in
                chain.candidates.iter_mut().filter(|c| c.id == id && c.phase == Phase::Building)
            {
                c.phase = Phase::Validating;
            }
        })?;
        let c = chain.candidates.iter().find(|c| c.id == id).ok_or("no such candidate")?;
        if c.phase != Phase::Validating {
            return Ok(chain);
        }
        git(self.dir(), &["worktree", "add", "--quiet", "--detach", &c.worktree, &c.commit])?;
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
    /// once, on its commit, in the worktree its fast gate ran in; green, the
    /// whole gate is put on record for its tree from its two parts → whether
    /// it ran.
    pub fn round(&self, target: &str, id: &str) -> Result<bool, String> {
        let mut started = false;
        let chain = self.change(target, |chain| started = chain.start_round(id))?;
        if !started {
            return Ok(false);
        }
        let tip = chain.candidates.iter().find(|c| c.id == id).ok_or("no such candidate")?;
        let slow = tip.slow.clone().ok_or("a round needs a slow gate")?;
        let mut evidence = match self.run(tip, &slow.command, slow.seconds) {
            Ok(evidence) => evidence,
            Err(said) => {
                self.change(target, |chain| chain.invalidate(id, "gate_interrupted"))?;
                return Err(said);
            }
        };
        if evidence.green() {
            let parts = (
                &GateCommand::new(&slow.whole),
                &GateCommand::new(&tip.command),
                &GateCommand::new(&slow.command),
            );
            let tree = Tree::named(&tip.tree);
            let why = run::record_whole(&self.git, &tree, &tip.commit, parts, &[], (self.now)())?;
            if let Some(why) = why {
                evidence.recorded = false;
                evidence.not_recorded = Some(why);
            }
        }
        let chain = self.change(target, |chain| {
            chain.finish(id, evidence.clone());
            stop(chain, id);
        })?;
        self.remove_ended(&chain);
        Ok(true)
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
        let read = self.read(target)?;
        if !read.chain.ripe(id, &read.base) {
            return Ok(vec![]);
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
                return Ok(vec![]);
            }
        }
        let last =
            members.last().ok_or_else(|| "a landing takes at least one candidate".to_string())?;
        let whole = GateCommand::new(turn::whole(last));
        let recorded =
            run::recorded(&self.git, &Tree::named(&last.tree), &whole, (self.now)())?.is_some();
        let mut heads = true;
        for m in &members {
            heads &= self.fetched(&format!("refs/heads/{}", m.branch))?.as_deref()
                == Some(m.original_head.as_str());
        }
        let parent = |commit: &str| git(self.dir(), &["rev-parse", &format!("{commit}^1")]);
        let ordered = turn::fast_forwards(&members, &read.base, parent)?;
        let Some(_publication) =
            read.chain.publication(id, &read.base, recorded, heads).filter(|_| ordered)
        else {
            self.change(target, |chain| chain.invalidate(id, "publication_evidence_changed"))?;
            return Ok(vec![]);
        };
        let landed: Vec<Candidate<F::Note>> = members.iter().map(|c| (*c).clone()).collect();
        let mut next = read.chain.clone();
        next.land(id);
        let state = self.commit(read.sha.as_deref(), &next)?;
        let record = self.record_ref(target);
        let old = read.sha.clone().unwrap_or_default();
        let mut refspecs = vec![];
        for update in turn::updates(&members, (&record, &old, &state), target) {
            // A source kept at its head was read so just before; plain git
            // has no condition on a ref it does not change.
            if update.old != update.new {
                refspecs.push(format!("{}:{}", update.new, update.name));
            }
        }
        let mut args = vec!["push", "--quiet", "--atomic", self.git.remote.as_str()];
        args.extend(refspecs.iter().map(String::as_str));
        if git(self.dir(), &args).is_err() {
            return Ok(vec![]);
        }
        let commit = last.commit.clone();
        Ok(landed.iter().map(|c| (c.change, self.forge.landed(&asked(c), &commit))).collect())
    }

    /// Removes the worktree of each candidate that has ended and stopped.
    fn remove_ended(&self, chain: &Chain<F::Note>) {
        for c in &chain.candidates {
            if c.stopped && !c.valid() && Path::new(&c.worktree).exists() {
                let _ = git(self.dir(), &["worktree", "remove", "--force", &c.worktree]);
            }
        }
    }

    /// Lands `changes`, which ask to land on one target, as this process
    /// alone: assigns each a candidate in the order given, gates each, runs
    /// the round at the chain's tip where their commits split their gate,
    /// and lands what is ripe, in order → what became of each.
    pub fn land(&self, changes: &[Change]) -> Result<Outcomes<F::Refusal>, Error<F::Error>> {
        let Some(target) = changes.first().map(|c| c.target.clone()) else {
            return Ok(vec![]);
        };
        let mut outcomes: Outcomes<F::Refusal> = vec![];
        let mut ids: Vec<(u64, String)> = vec![];
        for change in changes {
            match self.assign(change)? {
                Ok(id) => ids.push((change.number, id)),
                Err(outcome) => outcomes.push((change.number, outcome)),
            }
        }
        for (_, id) in &ids {
            self.gate(&target, id)?;
        }
        loop {
            let read = self.read(&target)?;
            let mine: Vec<&str> = ids.iter().map(|(_, id)| id.as_str()).collect();
            let rounds: Vec<&str> = mine
                .iter()
                .copied()
                .filter(|id| read.chain.next(id) == Some(Next::Round))
                .collect();
            if let Some(tip) = rounds.first() {
                self.round(&target, tip)?;
                continue;
            }
            let ripe = mine.iter().rev().find(|id| read.chain.ripe(id, &read.base));
            let Some(ripe) = ripe else { break };
            let landed = self.publish(&target, ripe, changes)?;
            if landed.is_empty() && read.chain.next(ripe) == Some(Next::Waiting) {
                break;
            }
        }
        let chain = self.change(&target, |_| {})?;
        self.remove_ended(&chain);
        for (number, id) in ids {
            let c = chain.candidates.iter().find(|c| c.id == id);
            let outcome = match c {
                Some(c) if c.phase == Phase::Landed => Outcome::Landed(c.commit.clone()),
                Some(c) => c.decision().map_or(Outcome::Waiting, Outcome::Decided),
                None => Outcome::Waiting,
            };
            outcomes.push((number, outcome));
        }
        Ok(outcomes)
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
