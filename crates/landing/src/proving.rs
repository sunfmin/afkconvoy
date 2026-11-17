//! The proving branch (docs/adr/0090): a round lands on it once its fast
//! gate is green, the fast gate its whole gate there, and the base branch
//! moves only by a promotion — the slow gate on the proving branch's tip,
//! joined with the fast run on record of the same tree, then a plain
//! fast-forward of the base branch to it.
//!
//! The landing is `Lander`'s, with its `proving` branch named
//! (`Declared::on`). The promotion is one procedure, `promote`, which every
//! runner of one follows: what differs between runners — how a remote is
//! read and pushed, in which lane the gate runs and how — it asks through
//! `Proving`. afkconvoy's runner implements it with its host's lanes and
//! gate runner, in the cli; `Promoter` is the plain-git one, for a program
//! with none of its own, and tells its program's forge how a promotion
//! ended (`Forge::promoted`).

use std::path::{Path, PathBuf};

use afkconvoy_gate::declared::{Gate, Gates};
use afkconvoy_gate::gate::{GateCommand, Judged, Red, Tree, Unsettled};
use afkconvoy_gate::run::{self, Git, Records};

use crate::forge::Forge;
use crate::land::{self, Error};
use crate::turn::Declared;

/// Whether a landing on `target` is one on the proving branch `proving`,
/// which its fast gate alone gates.
pub fn proves_on(proving: Option<&str>, target: &str) -> bool {
    proving == Some(target)
}

/// The gate a landing on the proving branch runs as its whole gate: the
/// fast gate `gates` declares, where it has time to run; `None`: the whole
/// gate, as on any other branch.
pub fn landing_gate(gates: Option<&Gates>) -> Option<&Gate> {
    gates.map(|gates| &gates.fast).filter(|fast| fast.seconds > 0)
}

impl Declared {
    /// What this declaration gates a landing on `target` by, where the
    /// proving branch is `proving`: on it, its fast gate is its whole gate,
    /// with its limit, and it splits nothing (`landing_gate`); elsewhere,
    /// as it is.
    pub fn on(self, proving: Option<&str>, target: &str) -> Declared {
        if !proves_on(proving, target) {
            return self;
        }
        match landing_gate(self.gates.as_ref()).cloned() {
            Some(fast) => Declared { whole: fast.command, seconds: fast.seconds, gates: None },
            None => self,
        }
    }
}

/// The base branch's tip and the proving branch's, as one read of the
/// remote saw them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tips {
    pub base: String,
    pub proving: String,
}

/// Which gate a promotion ran on the proving tip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ran {
    /// None: the whole gate's green run of its tree was on record.
    Recorded,
    /// The slow gate alone, joined with the fast run on record into a
    /// record of the whole gate.
    Slow,
    /// The whole gate: no fast run of its tree was on record.
    Whole,
}

impl Ran {
    /// Its word in an answer: `recorded`, `slow` or `whole`.
    pub fn word(self) -> &'static str {
        match self {
            Ran::Recorded => "recorded",
            Ran::Slow => "slow",
            Ran::Whole => "whole",
        }
    }
}

/// How a promotion ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    /// The base branch is at the proving branch's tip: nothing to do.
    Current,
    /// The base branch, at `base`, is not an ancestor of the proving tip: a
    /// promotion never moves it to what is not a fast-forward of it.
    Diverged { base: String },
    /// Another promotion of the repo runs, and answers for it.
    Promoting,
    /// Green, and the base branch fast-forwarded from `from` to the tip.
    Promoted { from: String },
    /// Green, and the base branch, at `base`, held the tip already: another
    /// promotion took it there, and nothing was pushed.
    AlreadyPromoted { base: String },
    /// The tip's gate ran red: the base branch stays where it was.
    GateRed(Red),
    /// The tip's gate ran red only on TIMEOUTs it cannot classify on an
    /// overloaded host: nothing is recorded, the base branch stays.
    GateUnsettled(Unsettled),
    /// The tip's gate passed, but the whole gate is not on record for its
    /// tree, for the reason given: nothing moved.
    Unrecorded(String),
    /// The remote refused the fast-forward each time the base branch was
    /// read again, last at `base`: it moved each time.
    TargetMoved { base: String },
}

impl Outcome {
    /// Its word in an answer.
    pub fn word(&self) -> &'static str {
        match self {
            Outcome::Current => "current",
            Outcome::Diverged { .. } => "diverged",
            Outcome::Promoting => "promoting",
            Outcome::Promoted { .. } => "promoted",
            Outcome::AlreadyPromoted { .. } => "already_promoted",
            Outcome::GateRed(_) => "gate_red",
            Outcome::GateUnsettled(_) => "gate_unsettled",
            Outcome::Unrecorded(_) => "unrecorded",
            Outcome::TargetMoved { .. } => "target_moved",
        }
    }
}

/// What a promotion came to: the tips as it read them last, which gate it
/// ran on the proving tip once it ran one, and how it ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Promotion {
    pub tips: Tips,
    pub ran: Option<Ran>,
    pub outcome: Outcome,
}

/// What a promotion asks of its runner: the remote read and pushed, the
/// gate's lane, the gate run, and the gate's records (`records`).
pub trait Proving {
    /// What reaching the remote, or running the gate, fails with.
    type Error;
    /// Where the gate's records are read and written.
    type Records: Records<Error = Self::Error>;
    /// What holds the gate's lane while it is held; let go as it drops.
    type Lane;

    fn records(&self) -> &Self::Records;

    /// Both tips as the remote has them now, their objects brought here.
    fn tips(&self) -> Result<Tips, Self::Error>;

    /// The base branch's tip as the remote has it now, its objects here.
    fn base(&self) -> Result<String, Self::Error>;

    /// Whether `head` holds `other`: `other` is `head` or an ancestor of it.
    fn holds(&self, head: &str, other: &str) -> Result<bool, Self::Error>;

    /// The tree of `commit`.
    fn tree(&self, commit: &str) -> Result<String, Self::Error>;

    /// What `tip` declares of its gate, whole, as the base branch reads it.
    fn declared(&self, tip: &str) -> Result<Declared, Self::Error>;

    /// The lane the gate `declared` runs in, held until it drops: asked for
    /// before the tips are read the last time, so that every landing while
    /// it waited is in what is gated → `None` while another promotion of
    /// the repo runs.
    fn lane(&self, declared: &Declared) -> Result<Option<Self::Lane>, Self::Error>;

    /// Runs `command` on `tip`, in a worktree of its own detached at it,
    /// under its limit of `seconds`, recording its run → what it judged. A
    /// green run on record of `command` on its tree may answer for it.
    fn gate(&self, tip: &str, command: &GateCommand, seconds: u64)
    -> Result<Judged, Self::Error>;

    /// Pushes `tip` onto the base branch by a push the remote refuses
    /// unless the base branch is still at `base` → whether it took.
    fn push(&self, tip: &str, base: &str) -> Result<bool, Self::Error>;
}

/// How many times a green promotion reads the base branch again after the
/// remote refused its push: each refusal is another promotion, which moves
/// it the one way it moves.
pub const PUBLISH_TRIES: u32 = 3;

/// Promotes the proving branch (docs/adr/0090): reads both tips; where the
/// base branch is behind the proving tip, takes the gate's lane and reads
/// them again — the tip gated is every landing up to then, in one run — and
/// gates that tip: a green run of the whole gate on record of its tree
/// answers; else, where the fast gate's run of it is on record, the slow
/// gate alone runs and the two are joined into a record of the whole gate;
/// else the whole gate runs. Green, the base branch is read again: where it
/// holds the tip already nothing is pushed; where it is still an ancestor
/// of it, the tip is pushed onto it, requiring the value read, and a
/// refusal is read again, `PUBLISH_TRIES` times at most. The base branch
/// never takes a merge.
pub fn promote<P: Proving>(proving: &P, now: u64) -> Result<Promotion, P::Error> {
    let asked = proving.tips()?;
    if let Some(outcome) = settled(proving, &asked)? {
        return Ok(Promotion { tips: asked, ran: None, outcome });
    }
    let declared = proving.declared(&asked.proving)?;
    let Some(_lane) = proving.lane(&declared)? else {
        return Ok(Promotion { tips: asked, ran: None, outcome: Outcome::Promoting });
    };
    let tips = proving.tips()?;
    if let Some(outcome) = settled(proving, &tips)? {
        return Ok(Promotion { tips, ran: None, outcome });
    }
    let tip = tips.proving.clone();
    let declared = proving.declared(&tip)?;
    let tree = Tree::named(&proving.tree(&tip)?);
    let whole = GateCommand::new(&declared.whole);
    let (judged, ran) = gate(proving, &declared, &tip, &tree, now)?;
    let outcome = match judged {
        Err(why) => Outcome::Unrecorded(why),
        Ok(Judged::Red(red)) => Outcome::GateRed(red),
        Ok(Judged::Unsettled(unsettled)) => Outcome::GateUnsettled(unsettled),
        Ok(Judged::Green) => match run::recorded(proving.records(), &tree, &whole, now)? {
            None => Outcome::Unrecorded(format!(
                "the whole gate's record of {tip} does not stand ({})",
                whole.text()
            )),
            Some(_) => publish(proving, &tip)?,
        },
    };
    Ok(Promotion { tips, ran: Some(ran), outcome })
}

/// The outcome of a promotion `tips` leave nothing to gate for: the base
/// branch at the proving tip, or no ancestor of it; `None`: the tip is to
/// be gated.
fn settled<P: Proving>(proving: &P, tips: &Tips) -> Result<Option<Outcome>, P::Error> {
    if tips.base == tips.proving {
        return Ok(Some(Outcome::Current));
    }
    if !proving.holds(&tips.proving, &tips.base)? {
        return Ok(Some(Outcome::Diverged { base: tips.base.clone() }));
    }
    Ok(None)
}

/// Gates the proving tip `tip`, of `tree`, as `declared` says its whole
/// gate → what it judged, or why its green slow run is not joined into a
/// record of the whole gate; and which gate ran.
fn gate<P: Proving>(
    proving: &P,
    declared: &Declared,
    tip: &str,
    tree: &Tree,
    now: u64,
) -> Result<(Result<Judged, String>, Ran), P::Error> {
    let records = proving.records();
    let whole = GateCommand::new(&declared.whole);
    if run::recorded(records, tree, &whole, now)?.is_some() {
        return Ok((Ok(Judged::Green), Ran::Recorded));
    }
    let split = declared.gates.as_ref().map(|gates| {
        (GateCommand::new(&gates.fast.command), GateCommand::new(&gates.slow.command), gates)
    });
    let joined = match &split {
        Some((fast, _, _)) => run::recorded(records, tree, fast, now)?.is_some(),
        None => false,
    };
    let Some((fast, slow, gates)) = split.as_ref().filter(|_| joined) else {
        let judged = proving.gate(tip, &whole, declared.seconds)?;
        return Ok((Ok(judged), Ran::Whole));
    };
    let judged = proving.gate(tip, slow, gates.slow.seconds)?;
    if judged == Judged::Green
        && let Some(why) = run::record_whole(records, tree, tip, (&whole, fast, slow), &[], now)?
    {
        return Ok((Err(why), Ran::Slow));
    }
    Ok((Ok(judged), Ran::Slow))
}

/// Moves the base branch to the gated `tip`, only ever by a fast-forward
/// from the value it was read at → how it ended.
fn publish<P: Proving>(proving: &P, tip: &str) -> Result<Outcome, P::Error> {
    let mut base = String::new();
    for _ in 0..PUBLISH_TRIES {
        base = proving.base()?;
        if proving.holds(&base, tip)? {
            return Ok(Outcome::AlreadyPromoted { base });
        }
        if !proving.holds(tip, &base)? {
            return Ok(Outcome::Diverged { base });
        }
        if proving.push(tip, &base)? {
            return Ok(Outcome::Promoted { from: base });
        }
    }
    Ok(Outcome::TargetMoved { base })
}

/// A promotion by plain git, for a program that has no runner of its own:
/// its gate runs to its end in this process, in a worktree of its own, with
/// no lane, and nothing keeps two promotions of one repo apart but the
/// remote, which takes only a fast-forward of the base branch from the
/// value read.
pub struct Promoter<'a, F: Forge> {
    /// A checkout of the repo: its remote holds both branches and the
    /// gate's records.
    pub git: Git,
    pub forge: &'a F,
    /// The branch a promotion moves.
    pub base_branch: String,
    /// The branch rounds land on fast-green, whose tip it moves it to.
    pub proving_branch: String,
    /// The host name a gate run is recorded as having run on.
    pub host: String,
    /// The kind of machine it is (`afkconvoy_gate::gate::machine`).
    pub machine: String,
    /// Where the gate's worktree is made.
    pub worktrees: PathBuf,
    /// What the commit named declares of its gate, read in the checkout
    /// named.
    pub declared: fn(&Path, &str) -> Result<Declared, String>,
    /// The time now, in epoch seconds.
    pub now: fn() -> u64,
}

impl<F: Forge> Promoter<'_, F> {
    /// Promotes the proving branch (`promote`), then tells the forge how it
    /// ended (`Forge::promoted`) → what it came to.
    pub fn promote(&self) -> Result<Promotion, Error<F::Error>> {
        let promotion = promote(self, (self.now)())?;
        self.forge.promoted(&promotion).map_err(Error::Forge)?;
        Ok(promotion)
    }

    fn dir(&self) -> &Path {
        &self.git.worktree
    }

    /// The tip of `branch` on the remote, its objects here.
    fn tip(&self, branch: &str) -> Result<String, String> {
        land::fetched(&self.git, &format!("refs/heads/{branch}"))?
            .ok_or_else(|| format!("the remote has no branch {branch:?}"))
    }
}

impl<F: Forge> Proving for Promoter<'_, F> {
    type Error = String;
    type Records = Git;
    type Lane = ();

    fn records(&self) -> &Git {
        &self.git
    }

    fn tips(&self) -> Result<Tips, String> {
        Ok(Tips { base: self.tip(&self.base_branch)?, proving: self.tip(&self.proving_branch)? })
    }

    fn base(&self) -> Result<String, String> {
        self.tip(&self.base_branch)
    }

    fn holds(&self, head: &str, other: &str) -> Result<bool, String> {
        land::contains(self.dir(), head, other)
    }

    fn tree(&self, commit: &str) -> Result<String, String> {
        land::git(self.dir(), &["rev-parse", &format!("{commit}^{{tree}}")])
    }

    fn declared(&self, tip: &str) -> Result<Declared, String> {
        (self.declared)(self.dir(), tip)
    }

    fn lane(&self, _: &Declared) -> Result<Option<()>, String> {
        Ok(Some(()))
    }

    fn gate(&self, tip: &str, command: &GateCommand, seconds: u64) -> Result<Judged, String> {
        let short = tip.get(..12).unwrap_or(tip);
        let worktree = self.worktrees.join(format!("promotion-{}-{short}", (self.now)()));
        let path = worktree.to_string_lossy().into_owned();
        land::git(self.dir(), &["worktree", "add", "--quiet", "--detach", &path, tip])?;
        let checkout = Git { worktree, ..self.git.clone() };
        let on = (self.host.as_str(), self.machine.as_str());
        let limit = std::time::Duration::from_secs(seconds);
        let ran = run::gate(&checkout, command, on, (self.now)(), limit, &[]);
        let _ = land::git(self.dir(), &["worktree", "remove", "--force", &path]);
        Ok(ran?.judged)
    }

    fn push(&self, tip: &str, base: &str) -> Result<bool, String> {
        let name = format!("refs/heads/{}", self.base_branch);
        let lease = format!("--force-with-lease={name}:{base}");
        let refspec = format!("{tip}:{name}");
        match land::git(self.dir(), &["push", "--quiet", &lease, &self.git.remote, &refspec]) {
            Ok(_) => Ok(true),
            Err(_) if self.tip(&self.base_branch)? != base => Ok(false),
            Err(said) => Err(said),
        }
    }
}
