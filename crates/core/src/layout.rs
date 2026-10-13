//! Where on the remote the fleet keeps its refs, and what a start asks of a
//! repo before anything else: which layout the remote takes, and whether the
//! base branch can be landed on at all.

use flux_rs::attrs::*;

/// One way of naming every ref the fleet writes. A remote takes the first of
/// `Layout::ALL` it does not reject; there is no third.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// Refs no branch rule and no push-triggered CI sees.
    Hidden,
    /// Ordinary branches, for a remote that takes nothing else.
    Branches,
}

struct Row {
    name: &'static str,
    root: &'static str,
}

impl Layout {
    /// Every layout, in the order a start asks the remote about them.
    pub const ALL: [Layout; 2] = [Layout::Hidden, Layout::Branches];

    /// The table: everything that differs between the layouts.
    fn row(self) -> Row {
        match self {
            Layout::Hidden => Row { name: "hidden", root: "refs/convoy/" },
            Layout::Branches => Row { name: "branches", root: "refs/heads/convoy-" },
        }
    }

    /// What the layout is written down as.
    pub fn name(self) -> &'static str {
        self.row().name
    }

    /// The layout written down as `name`.
    pub fn named(name: &str) -> Option<Layout> {
        Layout::ALL.into_iter().find(|layout| layout.name() == name)
    }

    /// The ref the fleet keeps `rest` at: `claim/7`, `gate/<key>`, ….
    pub fn under(self, rest: &str) -> String {
        format!("{}{rest}", self.row().root)
    }

    /// The pattern every ref of `kind` matches (`gate/`), or with no kind
    /// every ref the fleet keeps.
    pub fn every(self, kind: &str) -> String {
        self.under(&format!("{kind}*"))
    }

    /// What `under` was given to make `refname`; `None` for a ref that is not
    /// the fleet's.
    pub fn kept(self, refname: &str) -> Option<&str> {
        refname.strip_prefix(self.row().root)
    }

    /// The ref one probe pushes to ask whether the remote takes refs of `kind`.
    /// `token` is the probe's own, so two starts probing at once never meet on
    /// one ref.
    pub fn probe(self, kind: &str, token: &str) -> String {
        self.under(&format!("{kind}{PROBE}{token}"))
    }

    /// Whether `refname` is a probe's: one left behind is litter, never a record.
    pub fn is_probe(self, refname: &str) -> bool {
        self.kept(refname).is_some_and(|kept| {
            kept.starts_with(PROBE) || kept.strip_prefix(GATE_KIND).is_some_and(probe_leaf)
        })
    }
}

/// How long a startup probe that passed stands on the host that made it, in
/// seconds: what it asked — the ref layout, whether gate runs can be recorded
/// — does not change from one minute to the next.
pub const PROBE_STANDS: u64 = 3600;

/// Whether a startup probe that passed at `made` still stands at `now`: it is
/// younger than `PROBE_STANDS`, and not from a clock ahead of this one.
#[spec(fn(made: u64, now: u64) -> bool[made <= now && now - made < PROBE_STANDS])]
pub fn probe_stands(made: u64, now: u64) -> bool {
    made <= now && now - made < PROBE_STANDS
}

const PROBE: &str = "probe-";
/// The kind gate records are kept under.
pub const GATE_KIND: &str = "gate/";

fn probe_leaf(leaf: &str) -> bool {
    leaf.starts_with(PROBE)
}

/// What the remote said to one probe push.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Answer {
    /// It took the ref.
    Taken,
    /// The server turned the ref down, in these words.
    Rejected(String),
    /// The push ended with no verdict from the server: it says nothing about
    /// which layout is allowed.
    Unreached(String),
}

/// Reads one probe push: whether it succeeded, and what git said if not —
/// kept as one line, whatever its layout was.
pub fn answer(pushed: bool, stderr: &str) -> Answer {
    let said = stderr.split_whitespace().collect::<Vec<_>>().join(" ");
    if pushed {
        Answer::Taken
    } else if said.contains("[remote rejected]") {
        Answer::Rejected(said)
    } else {
        Answer::Unreached(said)
    }
}

/// The layout a start settled on. The branch layout is only ever settled on
/// because the server rejected the hidden one: `choose` is the only way to one.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(branches: bool, rejected: bool)]
#[invariant(branches == rejected)]
pub struct Chosen {
    #[field(bool[branches])]
    branches: bool,
    #[field(bool[rejected])]
    rejected: bool,
    /// The server's words, when it rejected the hidden layout.
    rejection: String,
}

impl Chosen {
    pub fn layout(&self) -> Layout {
        if self.branches { Layout::Branches } else { Layout::Hidden }
    }

    /// What the human is told about the choice; `None` when there is nothing
    /// to tell.
    pub fn warning(&self) -> Option<String> {
        if !self.rejected {
            return None;
        }
        Some(format!(
            "the remote rejects refs under {} ({}): every claim, heartbeat, gate record and \
             the landing train are kept as branches under {} instead, so CI that runs on \
             every push will fire on each of them",
            Layout::Hidden.every(""),
            self.rejection,
            Layout::Branches.every("")
        ))
    }
}

/// Settles the layout by asking the remote: `ask` pushes one probe under a
/// layout and says what came of it. Only a rejection moves on to the branch
/// layout; a push that reached no verdict is an error, and so is a remote
/// that rejects both.
pub fn choose(mut ask: impl FnMut(Layout) -> Answer) -> Result<Chosen, String> {
    match ask(Layout::Hidden) {
        Answer::Taken => Ok(Chosen { branches: false, rejected: false, rejection: String::new() }),
        Answer::Unreached(why) => Err(unreached(Layout::Hidden, &why)),
        Answer::Rejected(rejection) => match ask(Layout::Branches) {
            Answer::Taken => Ok(Chosen { branches: true, rejected: true, rejection }),
            Answer::Unreached(why) => Err(unreached(Layout::Branches, &why)),
            Answer::Rejected(second) => Err(format!(
                "the remote rejects the fleet's refs under both {} ({rejection}) and {} \
                 ({second}): there is nowhere to keep a claim",
                Layout::Hidden.every(""),
                Layout::Branches.every("")
            )),
        },
    }
}

fn unreached(layout: Layout, why: &str) -> String {
    format!(
        "the probe push under {} never reached a verdict from the remote ({why}): that says \
         nothing about where the fleet's refs may be kept",
        layout.every("")
    )
}

/// The rules of a base branch's protection that stop the fleet working on it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[refined_by(reviews: bool, restricted: bool, locked: bool, checks: int)]
pub struct Protection {
    /// The status checks a merge must have passed.
    #[field(Vec<String>[checks])]
    pub required_checks: Vec<String>,
    /// A change must come by a reviewed pull request.
    #[field(bool[reviews])]
    pub requires_reviews: bool,
    /// Only some may push.
    #[field(bool[restricted])]
    pub restricts_pushes: bool,
    #[field(bool[locked])]
    pub locked: bool,
}

impl Protection {
    #[spec(fn(required_checks: Vec<String>, reviews: bool, restricted: bool, locked: bool)
        -> Protection{p: p.reviews == reviews && p.restricted == restricted && p.locked == locked})]
    pub fn new(
        required_checks: Vec<String>,
        reviews: bool,
        restricted: bool,
        locked: bool,
    ) -> Protection {
        Protection {
            required_checks,
            requires_reviews: reviews,
            restricts_pushes: restricted,
            locked,
        }
    }

    /// Whether any rule stops the train landing by a push.
    #[spec(fn(&Protection[@p]) -> bool[p.checks > 0 || p.reviews || p.restricted || p.locked])]
    pub fn bars(&self) -> bool {
        !self.required_checks.is_empty()
            || self.requires_reviews
            || self.restricts_pushes
            || self.locked
    }

    /// The first rule that stops the fleet, in words.
    fn rule(&self) -> String {
        if !self.required_checks.is_empty() {
            format!("requires status checks ({})", self.required_checks.join(", "))
        } else if self.requires_reviews {
            "requires pull request reviews".to_string()
        } else if self.restricts_pushes {
            "restricts who may push".to_string()
        } else {
            "is locked".to_string()
        }
    }
}

/// What reading a base branch's protection came to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Guard {
    Unprotected,
    Protected(Protection),
    /// It could not be read, for this reason.
    Unreadable(String),
}

/// Leave to land on a base branch: its protection was read and no rule of it
/// stops a push, or it has none. `base_check` is where one comes from.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(barred: bool)]
#[invariant(!barred)]
pub struct Landable {
    #[field(bool[barred])]
    // What the invariant is stated on: nothing reads it back.
    #[allow(dead_code)]
    barred: bool,
}

impl Landable {
    pub fn under(protection: &Protection) -> Option<Landable> {
        let barred = protection.bars();
        if barred { None } else { Some(Landable { barred }) }
    }
}

/// What a start makes of the base branch's protection.
#[derive(Debug, PartialEq, Eq)]
pub enum BaseCheck {
    Clear(Landable),
    /// Unknown: carry on, with this said.
    Warn(String),
    /// The fleet cannot work this branch: the error, naming the rule.
    Refuse(String),
}

/// Whether the fleet can land on `branch`, given its protection. The train
/// lands by pushing a commit the gate passed here, so a branch that wants
/// checks of its own, a reviewed pull request, a named pusher, or no push at
/// all refuses every landing — said at start, not at the first one.
pub fn base_check(branch: &str, guard: &Guard) -> BaseCheck {
    match guard {
        Guard::Unprotected => BaseCheck::Clear(Landable { barred: false }),
        Guard::Unreadable(why) => BaseCheck::Warn(format!(
            "the protection of the base branch {branch:?} could not be read ({why}): if it \
             requires status checks or reviews, restricts pushes or is locked, every landing \
             will be refused"
        )),
        Guard::Protected(protection) => match Landable::under(protection) {
            Some(landable) => BaseCheck::Clear(landable),
            None => BaseCheck::Refuse(format!(
                "the base branch {branch:?} {}: the landing train lands by pushing a commit \
                 the gate passed, and that push would be refused. Lift the rule, or pick \
                 another base branch.",
                protection.rule()
            )),
        },
    }
}

/// Why a start stops when the remote has no base branch.
pub fn no_base_branch(branch: &str) -> String {
    format!(
        "the remote has no branch {branch:?}: the base branch must exist before the fleet \
         starts — afkconvoy creates none"
    )
}

/// What the human is told when the remote takes no gate record.
pub fn no_gate_records(layout: Layout, why: &str) -> String {
    format!(
        "the remote refuses refs under {} ({why}): no gate run can be put on record, so \
         every landing runs the gate itself",
        layout.every(GATE_KIND)
    )
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// The branch layout settled on with no rejection of the hidden one: a
    /// push that never reached the server read as "refs are refused here".
    #[should_fail]
    fn a_fallback_nobody_was_rejected_into(why: String) -> Chosen {
        Chosen { branches: true, rejected: false, rejection: why }
    }

    /// A rejection that left the fleet on the layout that was rejected.
    #[should_fail]
    fn a_rejected_layout_kept(rejection: String) -> Chosen {
        Chosen { branches: false, rejected: true, rejection }
    }

    /// Leave to land taken without asking what the protection bars.
    #[should_fail]
    fn a_protection_nobody_read(protection: &Protection) -> Landable {
        Landable { barred: protection.bars() }
    }

    /// A locked branch landed on.
    #[should_fail]
    #[spec(fn(protection: &Protection{p: p.locked}) -> Landable)]
    fn a_locked_branch_landed_on(protection: &Protection) -> Landable {
        Landable { barred: protection.locked }
    }
}

/// What callers may rely on: each is checked by Flux, and stops passing when
/// the spec it reads is weakened.
#[cfg(flux)]
#[allow(dead_code)]
mod relied_on {
    use super::*;

    /// A branch with required reviews bars the train.
    #[spec(fn(protection: &Protection{p: p.reviews}) -> bool[true])]
    fn required_reviews_bar_the_train(protection: &Protection) -> bool {
        protection.bars()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_table_is_two_layouts_and_each_reads_back() {
        assert_eq!(Layout::Hidden.under("claim/7"), "refs/convoy/claim/7");
        assert_eq!(Layout::Branches.under("claim/7"), "refs/heads/convoy-claim/7");
        assert_eq!(Layout::Branches.every(""), "refs/heads/convoy-*");
        assert_eq!(Layout::Hidden.every(GATE_KIND), "refs/convoy/gate/*");
        for layout in Layout::ALL {
            assert_eq!(Layout::named(layout.name()), Some(layout));
            assert_eq!(layout.kept(&layout.under("train/line/main")), Some("train/line/main"));
        }
        assert_eq!(Layout::named("tags"), None);
        assert_eq!(Layout::Hidden.kept("refs/heads/convoy-claim/7"), None);
        assert_eq!(Layout::Branches.kept("refs/heads/main"), None);
    }

    #[test]
    fn a_probe_is_told_from_a_record() {
        for layout in Layout::ALL {
            assert!(layout.is_probe(&layout.probe("", "17-4")));
            assert!(layout.is_probe(&layout.probe(GATE_KIND, "17-4")));
            assert!(!layout.is_probe(&layout.under("claim/7")));
            // An instance may be named anything its grammar allows.
            assert!(!layout.is_probe(&layout.under("heartbeat/probe-1")));
            assert!(!layout.is_probe(&layout.under("gate/4b825dc-00ff")));
        }
        assert_ne!(Layout::Hidden.probe("", "1"), Layout::Hidden.probe("", "2"));
    }

    #[test]
    fn only_the_servers_no_is_a_rejection() {
        assert_eq!(answer(true, "anything"), Answer::Taken);
        let no = "remote: no such refs here   \n ! [remote rejected] abc -> x (hook declined)\n";
        let one_line = "remote: no such refs here ! [remote rejected] abc -> x (hook declined)";
        assert_eq!(answer(false, no), Answer::Rejected(one_line.to_string()));
        let lost = "fatal: unable to access 'https://x/': Could not resolve host: x\n";
        assert!(matches!(answer(false, lost), Answer::Unreached(_)));
        assert!(matches!(answer(false, ""), Answer::Unreached(_)));
    }

    fn asked(answers: &[Answer]) -> (Result<Chosen, String>, Vec<Layout>) {
        let mut asked = vec![];
        let chosen = choose(|layout| {
            asked.push(layout);
            answers[asked.len() - 1].clone()
        });
        (chosen, asked)
    }

    #[test]
    fn a_remote_that_takes_hidden_refs_is_asked_nothing_more() {
        let (chosen, asked) = asked(&[Answer::Taken]);
        let chosen = chosen.unwrap();
        assert_eq!(chosen.layout(), Layout::Hidden);
        assert_eq!(chosen.warning(), None);
        assert_eq!(asked, [Layout::Hidden]);
    }

    #[test]
    fn a_rejection_falls_back_to_branches_and_says_ci_will_fire() {
        let (chosen, asked) = asked(&[Answer::Rejected("hook declined".into()), Answer::Taken]);
        let chosen = chosen.unwrap();
        assert_eq!(chosen.layout(), Layout::Branches);
        let warning = chosen.warning().unwrap();
        assert!(warning.contains("hook declined") && warning.contains("CI"), "{warning}");
        assert!(warning.contains("refs/heads/convoy-*"), "{warning}");
        assert_eq!(asked, Layout::ALL);
    }

    #[test]
    fn a_push_with_no_verdict_is_an_error_not_a_fallback() {
        let (chosen, asked) = asked(&[Answer::Unreached("no route".into())]);
        let error = chosen.unwrap_err();
        assert!(error.contains("never reached a verdict") && error.contains("no route"), "{error}");
        assert_eq!(asked, [Layout::Hidden], "the branch layout was never tried");

        let (chosen, _) =
            asked_twice(Answer::Rejected("no".into()), Answer::Unreached("timeout".into()));
        assert!(chosen.unwrap_err().contains("refs/heads/convoy-*"));
        let (chosen, _) =
            asked_twice(Answer::Rejected("no".into()), Answer::Rejected("nor".into()));
        let error = chosen.unwrap_err();
        assert!(error.contains("both") && error.contains("nor"), "{error}");
    }

    fn asked_twice(first: Answer, second: Answer) -> (Result<Chosen, String>, Vec<Layout>) {
        asked(&[first, second])
    }

    fn refusal(protection: Protection) -> String {
        match base_check("main", &Guard::Protected(protection)) {
            BaseCheck::Refuse(why) => why,
            other => panic!("not refused: {other:?}"),
        }
    }

    #[test]
    fn each_rule_that_stops_a_push_is_named() {
        let checks =
            refusal(Protection::new(vec!["ci".into(), "lint".into()], false, false, false));
        assert!(checks.contains("requires status checks (ci, lint)"), "{checks}");
        assert!(checks.contains("\"main\""), "{checks}");
        let reviews = refusal(Protection::new(vec![], true, false, false));
        assert!(reviews.contains("requires pull request reviews"), "{reviews}");
        let pushes = refusal(Protection::new(vec![], false, true, false));
        assert!(pushes.contains("restricts who may push"), "{pushes}");
        let locked = refusal(Protection::new(vec![], false, false, true));
        assert!(locked.contains("is locked"), "{locked}");
    }

    #[test]
    fn a_branch_no_rule_bars_is_clear() {
        assert!(matches!(base_check("main", &Guard::Unprotected), BaseCheck::Clear(_)));
        // Protected, but by rules that stop no push: linear history, say.
        let mild = Protection::new(vec![], false, false, false);
        assert!(matches!(base_check("main", &Guard::Protected(mild)), BaseCheck::Clear(_)));
    }

    #[test]
    fn an_unreadable_protection_is_a_warning() {
        match base_check("main", &Guard::Unreadable("HTTP 403".into())) {
            BaseCheck::Warn(text) => assert!(text.contains("HTTP 403") && text.contains("main")),
            other => panic!("{other:?}"),
        }
    }
}
