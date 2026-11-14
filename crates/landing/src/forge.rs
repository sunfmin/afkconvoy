//! What a landing needs of the forge its changes are asked on: the program
//! that lands says it, by implementing `Forge` (docs/adr/0060). afkconvoy's
//! is GitHub's pull requests and the issues they close; another program's
//! may be Forgejo's, or its own tasks. Nothing here names either.

use crate::chain::Evidence;

/// One change that asks to land, as its forge says it: a pull request, a
/// merge request, a task's branch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    /// Its number on the forge, which the chain records its candidate by.
    pub number: u64,
    /// Where its head is on the remote, which landing never moves: a bare
    /// name is the branch of that name (`refs/heads/<source>`); a full ref,
    /// one that starts with `refs/` (`refs/pull/2/head`, a change that has
    /// no branch of its own), is that ref as it is (`source_ref`). The
    /// original head a candidate merges is this ref's.
    pub source: String,
    /// The branch it asks to land on.
    pub target: String,
    /// Its title, which its program's merge message may carry.
    pub title: String,
}

/// The ref the remote holds a change's `source` at (`Change::source`): a
/// full ref, one that starts with `refs/`, as it is; a bare name, the branch
/// of that name.
pub fn source_ref(source: &str) -> String {
    if source.starts_with("refs/") { source.to_string() } else { format!("refs/heads/{source}") }
}

/// The forge a program's changes are asked on, as a landing asks it.
pub trait Forge {
    /// What reaching the forge fails with.
    type Error;
    /// What the program notes of a candidate of a change, beside what the
    /// chain reads (`chain::Candidate::note`).
    type Note;
    /// Why a change may not land now, in the program's own terms.
    type Refusal;

    /// The change as the forge has it now: its source and target as they
    /// stand; `None` once it is no longer asking to land.
    fn current(&self, change: &Change) -> Result<Option<Change>, Self::Error>;

    /// What the program notes of a candidate of `change`.
    fn note(&self, change: &Change) -> Self::Note;

    /// The message of the commit that lands `change`: what reads back, from
    /// the target's history, that it landed.
    fn message(&self, change: &Change) -> String;

    /// Whether `change` may land now → why not, where it may not: asked as
    /// a turn begins and again just before it lands.
    fn may_land(&self, change: &Change) -> Result<Result<(), Self::Refusal>, Self::Error>;

    /// `change` landed on its target at `commit`: what follows of it on the
    /// forge, such as closing what it completes.
    fn landed(&self, change: &Change, commit: &str) -> Result<(), Self::Error>;

    /// `change` is sent back from a batch round: its candidate was the
    /// round's first red one, and `run` is the red run that found it
    /// (docs/adr/0068). What follows of it on the forge, such as asking its
    /// author to fix it; nothing, unless the program says.
    fn sent_back(&self, change: &Change, run: Option<&Evidence>) -> Result<(), Self::Error> {
        let _ = (change, run);
        Ok(())
    }
}
