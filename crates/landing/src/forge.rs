//! What a landing needs of the forge its changes are asked on: the program
//! that lands says it, by implementing `Forge` (docs/adr/0058). afkconvoy's
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

    /// `change` leaves its round carrying others (docs/adr/0083): it is the
    /// later-asked change of a pair whose fast gate ran red, or it conflicts
    /// with every partner. What follows of it on the forge, such as asking
    /// its author to merge `sent.merge` into it and fix it; nothing, unless
    /// the program says.
    fn sent_back(&self, change: &Change, sent: &SentBack<'_>) -> Result<(), Self::Error> {
        let _ = (change, sent);
        Ok(())
    }

    /// `change`'s description as the forge has it now, which carrying
    /// rewrites (`together`).
    fn description(&self, change: &Change) -> Result<String, Self::Error>;

    /// Rewrites `change`'s description as `description`.
    fn describe(&self, change: &Change, description: &str) -> Result<(), Self::Error>;

    /// Closes `change` without landing it, commented `comment`.
    fn close(&self, change: &Change, comment: &str) -> Result<(), Self::Error>;

    /// Deletes `change`'s source branch on the remote, while it is still at
    /// `head`: one that moved since holds work nothing carried, and stays.
    fn delete_source(&self, change: &Change, head: &str) -> Result<(), Self::Error>;

    /// `carried` was closed, carried by `by` (`carry`): what else follows of
    /// it in the program, such as ending the worker that worked it and
    /// removing its worktree, and noting where its work went on; nothing,
    /// unless the program says.
    fn carried(&self, carried: &Change, by: &Change) -> Result<(), Self::Error> {
        let _ = (carried, by);
        Ok(())
    }

    /// The changes `change` carried, as the program recorded each when it
    /// was told of it (`carried`), whatever `change`'s description says now:
    /// its author may have rewritten it since. Each as `carry` was handed
    /// it, its source as it was; none, unless the program says.
    fn carries(&self, change: &Change) -> Result<Vec<Change>, Self::Error> {
        let _ = change;
        Ok(vec![])
    }

    /// A promotion of the proving branch ended as `promotion` says
    /// (`proving::Promoter`, docs/adr/0090): what follows of it on the
    /// forge, such as asking for a repair of a red proving tip, or closing
    /// one once a promotion is green; nothing, unless the program says.
    fn promoted(&self, promotion: &crate::proving::Promotion) -> Result<(), Self::Error> {
        let _ = promotion;
        Ok(())
    }
}

/// What a carrying change is handed as it leaves its round (`Forge::sent_back`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SentBack<'a> {
    /// The run of the pair whose fast gate ran red, where one did.
    pub run: Option<&'a Evidence>,
    /// The commits its author merges into its source, in order: the red
    /// pair's merge, or its own node and then the node it conflicts with.
    pub merge: &'a [String],
    /// The changes it carries, in the order they asked.
    pub carried: &'a [u64],
}

/// The marker that says a description carries `change` already.
fn carries_marker(change: u64) -> String {
    format!("<!--carries change={change}-->")
}

/// The changes `description` says it carries (`together`), in the order it
/// took them.
pub fn carried_in(description: &str) -> Vec<u64> {
    let mut found = vec![];
    for rest in description.split("<!--carries change=").skip(1) {
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if let Ok(number) = digits.parse() {
            found.push(number);
        }
    }
    found
}

/// `description` of a change that carries `carried` as well, whose own
/// description is `theirs`: both changes together, `theirs` under a heading
/// that names `carried`, once — what closes it closes with the carrier.
/// One that carries it already is left as it is.
pub fn together(description: &str, carried: &Change, theirs: &str) -> String {
    let marker = carries_marker(carried.number);
    if description.contains(&marker) {
        return description.to_string();
    }
    let mine = description.trim_end();
    let heading = format!("## Carries #{}: {}", carried.number, carried.title);
    format!("{mine}\n\n{marker}\n{heading}\n\n{}\n", theirs.trim())
}

/// `by` carries `carried`, whose source was at `head` when `by`'s source
/// was found to hold it (docs/adr/0083): `by`'s description is rewritten as
/// both together, `carried` is closed, commented "carried by #…", its
/// source is deleted, and its program is told (`Forge::carried`). In that
/// order: what `carried` completes is in `by`'s description before it
/// closes.
pub fn carry<F: Forge>(
    forge: &F,
    by: &Change,
    carried: &Change,
    head: &str,
) -> Result<(), F::Error> {
    let description = forge.description(by)?;
    let theirs = forge.description(carried)?;
    let both = together(&description, carried, &theirs);
    if both != description {
        forge.describe(by, &both)?;
    }
    forge.close(carried, &format!("carried by #{}", by.number))?;
    forge.delete_source(carried, head)?;
    forge.carried(carried, by)
}

/// Rewrites `by`'s description to carry again each change it carried
/// (`Forge::carries`) that it no longer says it does (`together`): asked
/// just before `by` lands, so that whatever closes with a carried change
/// closes with `by`, though its author rewrote its description after
/// `carry` (sunfmin/control#1248).
pub fn described<F: Forge>(forge: &F, by: &Change) -> Result<(), F::Error> {
    let carries = forge.carries(by)?;
    if carries.is_empty() {
        return Ok(());
    }
    let description = forge.description(by)?;
    let mut both = description.clone();
    for carried in &carries {
        if !carried_in(&both).contains(&carried.number) {
            both = together(&both, carried, &forge.description(carried)?);
        }
    }
    if both != description {
        forge.describe(by, &both)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn together_puts_the_carried_description_under_a_heading_once() {
        let carried =
            Change { number: 7, source: "b7".into(), target: "main".into(), title: "Seven".into() };
        let both = together("Mine.\n\nCloses o/t#1\n", &carried, "Theirs.\n\nCloses o/t#2");
        assert_eq!(
            both,
            "Mine.\n\nCloses o/t#1\n\n<!--carries change=7-->\n## Carries #7: Seven\n\nTheirs.\n\nCloses o/t#2\n"
        );
        assert_eq!(together(&both, &carried, "Theirs, edited."), both, "carried once");
        assert_eq!(carried_in(&both), [7]);
        assert_eq!(carried_in("Mine."), Vec::<u64>::new());
    }

    /// A forge of descriptions alone, which says what each change carried.
    struct Described {
        descriptions: std::cell::RefCell<std::collections::BTreeMap<u64, String>>,
        carries: Vec<Change>,
    }

    impl Forge for Described {
        type Error = String;
        type Note = ();
        type Refusal = ();
        fn current(&self, change: &Change) -> Result<Option<Change>, String> {
            Ok(Some(change.clone()))
        }
        fn note(&self, _: &Change) {}
        fn message(&self, change: &Change) -> String {
            change.title.clone()
        }
        fn may_land(&self, _: &Change) -> Result<Result<(), ()>, String> {
            Ok(Ok(()))
        }
        fn landed(&self, _: &Change, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn description(&self, change: &Change) -> Result<String, String> {
            Ok(self.descriptions.borrow().get(&change.number).cloned().unwrap_or_default())
        }
        fn describe(&self, change: &Change, description: &str) -> Result<(), String> {
            self.descriptions.borrow_mut().insert(change.number, description.to_string());
            Ok(())
        }
        fn close(&self, _: &Change, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn delete_source(&self, _: &Change, _: &str) -> Result<(), String> {
            Ok(())
        }
        fn carries(&self, _: &Change) -> Result<Vec<Change>, String> {
            Ok(self.carries.clone())
        }
    }

    /// A carrying description its author rewrote is written back to carry
    /// what it carried before it lands, once (sunfmin/control#1248).
    #[test]
    fn a_rewritten_carrying_description_carries_again_what_it_carried() {
        let change = |number: u64| Change {
            number,
            source: format!("b{number}"),
            target: "main".into(),
            title: format!("Change {number}"),
        };
        let forge = Described {
            descriptions: std::cell::RefCell::new(
                [(20, "Rewritten.\n\nCloses o/t#2\n".to_string()), (10, "Closes o/t#1".into())]
                    .into(),
            ),
            carries: vec![change(10)],
        };
        described(&forge, &change(20)).unwrap();
        let after = forge.description(&change(20)).unwrap();
        assert_eq!(
            after,
            "Rewritten.\n\nCloses o/t#2\n\n<!--carries change=10-->\n## Carries #10: Change 10\n\nCloses o/t#1\n"
        );
        described(&forge, &change(20)).unwrap();
        assert_eq!(forge.description(&change(20)).unwrap(), after, "carried once");
        let none = Described { carries: vec![], ..forge };
        described(&none, &change(30)).unwrap();
        assert!(!none.descriptions.borrow().contains_key(&30), "nothing carried, nothing written");
    }
}
