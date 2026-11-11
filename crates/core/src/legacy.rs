//! Names and ancestry of the read-only line migrated from an older release.
use crate::layout::Layout;
use crate::worker::is_worktree_named;
pub struct TrainRefs {
    /// The train itself: its tip.
    pub line: String,
    /// The commit of the line the train worker last gated red.
    pub red: String,
}

pub fn train_refs(layout: Layout, base_branch: &str) -> TrainRefs {
    TrainRefs {
        line: layout.under(&format!("train/line/{base_branch}")),
        red: layout.under(&format!("train/red/{base_branch}")),
    }
}

/// The train's line as the remote has it.
pub struct Line<'a> {
    pub tip: &'a str,
    /// The base branch already holds the tip: the train landed.
    pub on_base: bool,
}

/// What the next PR joins onto. A line the base branch already holds — the
/// train landed, or nothing ever joined — is no train: the next PR joins onto
/// the base branch's tip, and pushing that commit fast-forwards the line all
/// the same.
pub fn joins_onto<'a>(line: Option<Line<'a>>, base: &'a str) -> &'a str {
    match line {
        Some(line) if !line.on_base => line.tip,
        _ => base,
    }
}

pub const TRAIN_WORKTREE: &str = "convoy-train";
pub fn is_train_worktree(directory: &str) -> bool {
    is_worktree_named(directory, TRAIN_WORKTREE)
}

/// Names retained only to retire proposal runs written by older releases.
/// They confer no authority on the decision or its author.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Proposal {
    pub issue: u64,
    pub comment: u64,
}

impl Proposal {
    pub fn claimed(key: &str) -> Option<Self> {
        Self::tagged(key.strip_prefix("precedent-")?)
    }

    pub fn branch_named(branch: &str) -> Option<Self> {
        Self::tagged(branch.strip_prefix("convoy-precedent-")?)
    }

    fn tagged(tag: &str) -> Option<Self> {
        let (issue, comment) = tag.split_once("-c")?;
        Some(Self {
            issue: crate::record::canonical_number(issue)?,
            comment: crate::record::canonical_number(comment)?,
        })
    }

    pub fn branch(&self) -> String {
        format!("convoy-precedent-{}-c{}", self.issue, self.comment)
    }
}
