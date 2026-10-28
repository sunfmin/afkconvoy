//! What a claim leaves of its worktree as it ends without landing: removed
//! when it holds nothing the remote does not — a later dispatch recreates it
//! at the pushed tip — and kept, saying why, when it does.

use flux_rs::attrs::*;

/// What git read of one worktree against the remote: what it holds that
/// neither its pushed branch nor the base branch does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Held {
    /// Anything uncommitted or untracked.
    pub dirty: bool,
    /// Commits on its branch that are on neither its pushed branch nor the
    /// base branch; `None` when they could not be counted — unreadable is not
    /// zero.
    pub unpushed: Option<u64>,
}

/// The proof that a worktree holds nothing the remote does not: a clean tree,
/// and every commit of its branch pushed. `Left::of` is the only way to one,
/// so what asks for one — removing the worktree — never takes work that
/// exists only on this host.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(unpushed: int, dirty: bool)]
#[invariant(unpushed == 0 && !dirty)]
// The fields are what the invariant is stated on: nothing reads them back.
#[allow(dead_code)]
pub struct Spare {
    #[field(u64[unpushed])]
    unpushed: u64,
    #[field(bool[dirty])]
    dirty: bool,
}

/// Why a worktree is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptBecause {
    /// It holds something uncommitted.
    Dirty,
    /// Its branch has commits the remote does not.
    Unpushed,
    /// What it holds could not be read: never taken for nothing.
    Unread,
}

impl KeptBecause {
    /// The word a command's answer says it with (`kept_because`).
    pub fn key(self) -> &'static str {
        match self {
            KeptBecause::Dirty => "dirty",
            KeptBecause::Unpushed => "unpushed",
            KeptBecause::Unread => "unread",
        }
    }
}

/// A worktree that is kept, and why.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Kept {
    pub because: KeptBecause,
    /// How many commits only this host has; `None` when unread.
    pub unpushed: Option<u64>,
}

impl Kept {
    /// What the worktree holds that the remote does not, as a sentence that
    /// follows "it holds".
    pub fn holds(&self) -> String {
        let commits = match self.unpushed {
            Some(0) => None,
            Some(1) => Some("1 commit the remote does not have".to_string()),
            Some(count) => Some(format!("{count} commits the remote does not have")),
            None => Some("commits that could not be checked against the remote".to_string()),
        };
        match (self.because, commits) {
            (KeptBecause::Dirty, Some(commits)) => format!("uncommitted changes and {commits}"),
            (KeptBecause::Dirty, None) => "uncommitted changes".to_string(),
            (_, Some(commits)) => commits,
            (_, None) => "nothing the remote does not have".to_string(),
        }
    }

    /// The status board's line on it: the work exists only on that host.
    pub fn board_line(&self) -> String {
        format!(
            "Its worktree is kept on the host that worked it: it holds {}, found nowhere else.",
            self.holds()
        )
    }
}

/// What becomes of a worktree as its claim ends.
#[derive(Debug, PartialEq, Eq)]
pub enum Left {
    /// Removed: it holds nothing the remote does not.
    Removed(Spare),
    Kept(Kept),
}

impl Left {
    /// What becomes of a worktree holding `held`: kept for anything dirty
    /// first, then for any commit only it has, or any it could not count.
    pub fn of(held: &Held) -> Left {
        if held.dirty {
            return Left::Kept(Kept { because: KeptBecause::Dirty, unpushed: held.unpushed });
        }
        match held.unpushed {
            Some(0) => Left::Removed(Spare { unpushed: 0, dirty: false }),
            Some(count) => {
                Left::Kept(Kept { because: KeptBecause::Unpushed, unpushed: Some(count) })
            }
            None => Left::Kept(Kept { because: KeptBecause::Unread, unpushed: None }),
        }
    }

    /// The proof the worktree may be removed, when it may.
    pub fn spare(&self) -> Option<&Spare> {
        match self {
            Left::Removed(spare) => Some(spare),
            Left::Kept(_) => None,
        }
    }

    /// Why it is kept, when it is.
    pub fn kept(&self) -> Option<&Kept> {
        match self {
            Left::Removed(_) => None,
            Left::Kept(kept) => Some(kept),
        }
    }
}

/// Whether settling keeps this worktree. Parking keeps anything dirty.
/// Settling does not: a dirty tree whose every commit is on the remote is
/// the gate's leftovers, and the worktree goes. A commit the remote does
/// not have, or a tree that could not be read, stays.
pub fn settle_keeps(left: &Left) -> bool {
    match left.kept() {
        None => false,
        Some(kept) => match (kept.because, kept.unpushed) {
            (KeptBecause::Dirty, Some(0)) => false,
            (KeptBecause::Dirty, _) => true,
            (KeptBecause::Unpushed | KeptBecause::Unread, _) => true,
        },
    }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A worktree with something uncommitted, taken for one to remove.
    #[should_fail]
    fn a_dirty_tree_called_spare() -> Spare {
        Spare { unpushed: 0, dirty: true }
    }

    /// A worktree whose branch has a commit only it holds, taken for one to remove.
    #[should_fail]
    fn an_unpushed_commit_called_spare(unpushed: u64) -> Spare {
        Spare { unpushed: unpushed + 1, dirty: false }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_worktree_is_removed_only_when_it_holds_nothing_the_remote_does_not() {
        let left = |dirty, unpushed| Left::of(&Held { dirty, unpushed });
        assert!(left(false, Some(0)).spare().is_some());
        for (dirty, unpushed, because) in [
            (true, Some(0), KeptBecause::Dirty),
            (true, Some(2), KeptBecause::Dirty),
            (true, None, KeptBecause::Dirty),
            (false, Some(1), KeptBecause::Unpushed),
            (false, None, KeptBecause::Unread),
        ] {
            let left = left(dirty, unpushed);
            assert_eq!(left.kept(), Some(&Kept { because, unpushed }), "{dirty} {unpushed:?}");
            assert!(left.spare().is_none());
        }
    }

    #[test]
    fn a_settled_worktree_stays_only_for_a_commit_the_remote_does_not_have() {
        let kept = |because, unpushed| Left::Kept(Kept { because, unpushed });
        assert!(!settle_keeps(&Left::of(&Held { dirty: false, unpushed: Some(0) })));
        assert!(!settle_keeps(&kept(KeptBecause::Dirty, Some(0))));
        assert!(settle_keeps(&kept(KeptBecause::Dirty, Some(2))));
        assert!(settle_keeps(&kept(KeptBecause::Dirty, None)));
        assert!(settle_keeps(&kept(KeptBecause::Unpushed, Some(1))));
        assert!(settle_keeps(&kept(KeptBecause::Unread, None)));
    }

    #[test]
    fn a_kept_worktree_says_what_it_holds() {
        let holds = |because, unpushed| Kept { because, unpushed }.holds();
        assert_eq!(holds(KeptBecause::Dirty, Some(0)), "uncommitted changes");
        assert_eq!(
            holds(KeptBecause::Dirty, Some(1)),
            "uncommitted changes and 1 commit the remote does not have"
        );
        assert_eq!(holds(KeptBecause::Unpushed, Some(3)), "3 commits the remote does not have");
        assert!(holds(KeptBecause::Unread, None).contains("could not be checked"));
    }
}
