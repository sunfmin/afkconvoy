//! The landing crate as another program uses it (docs/adr/0060): from
//! outside, through its public API alone, with a fake forge and no fleet
//! beside it. Changes land on a local bare repo in a round through its
//! pairing tree (docs/adr/0083) — each pair fast-gated, the pairs of a level
//! at once, the root merged onto the target tip and gated whole once, the
//! target fast-forwarded through one candidate per change in order, and the
//! forge told each landed. A red pair sends its later-asked change back
//! while the rest lands; a red root lands nothing and is not bisected. A
//! change asked while another run's round runs its whole gate stacks on
//! that round's root.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use afkconvoy_gate::declared;
use afkconvoy_gate::gate::GateCommand;
use afkconvoy_gate::layout::Layout;
use afkconvoy_gate::run::{self, Git, Settled};
use afkconvoy_landing::chain::{Candidate, Chain, Decision, Evidence, Phase, Plain, Process};
use afkconvoy_landing::forge::{Change, Forge};
use afkconvoy_landing::land::{Lander, Outcome};
use afkconvoy_landing::turn::{self, Declared};

/// A forge that lets every change land and remembers what it was told.
#[derive(Default)]
struct Fake {
    landed: RefCell<Vec<(u64, String)>>,
    sent_back: RefCell<Vec<(u64, Option<Evidence>)>>,
}

impl Forge for Fake {
    type Error = String;
    type Note = Plain;
    type Refusal = String;

    fn current(&self, change: &Change) -> Result<Option<Change>, String> {
        Ok(Some(change.clone()))
    }

    fn note(&self, _: &Change) -> Plain {
        Plain::default()
    }

    fn message(&self, change: &Change) -> String {
        format!("{} (!{})", change.title, change.number)
    }

    fn may_land(&self, _: &Change) -> Result<Result<(), String>, String> {
        Ok(Ok(()))
    }

    fn landed(&self, change: &Change, commit: &str) -> Result<(), String> {
        self.landed.borrow_mut().push((change.number, commit.to_string()));
        Ok(())
    }

    fn sent_back(&self, change: &Change, run: Option<&Evidence>) -> Result<(), String> {
        self.sent_back.borrow_mut().push((change.number, run.cloned()));
        Ok(())
    }
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git").current_dir(dir).args(args).output().unwrap();
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// The repo's gate, its fast gate and its slow one.
const DECLARED: &str = "# The repo's gate\n\n```yaml gates\nfast:\n  command: ./gate.sh fast\n  seconds: 60\n  budget_seconds: 30\nslow:\n  command: ./gate.sh slow\n  seconds: 120\n  budget_seconds: 60\n```\n";

/// A scratch repo whose gate notes each run in `ran` — which gate, on which
/// commit — whose slow gate is red on a tree that holds `breaks` — or any
/// file named `breaks…` — and whose fast gate is red on one that holds both
/// `red-x` and `red-y`, and notes in `order` when each fast run starts and
/// ends, a second apart, where `concurrent` is there; whose slow gate, where
/// `hold` is there, says so in `slowing` and runs on only once `release` is;
/// committed and pushed to a bare remote beside it.
fn scratch(root: &Path) -> (PathBuf, PathBuf) {
    let remote = root.join("remote.git");
    let work = root.join("work");
    let ran = root.join("ran");
    std::fs::create_dir_all(&work).unwrap();
    git(root, &["init", "--quiet", "--bare", remote.to_str().unwrap()]);
    git(&work, &["init", "--quiet", "--initial-branch=main"]);
    for (key, value) in [("user.name", "someone"), ("user.email", "someone@example.com")] {
        git(&work, &["config", key, value]);
    }
    std::fs::write(work.join("gate.md"), DECLARED).unwrap();
    // Each run also notes how many worktrees the checkout lists as it runs.
    let script = format!(
        "#!/bin/sh\necho \"$1 $(git rev-parse HEAD)\" >> {ran}\necho \"$1 $(git worktree list --porcelain | grep -c '^worktree ')\" >> {listed}\nif [ \"$1\" = fast ] && [ -e {root}/concurrent ]; then echo start >> {root}/order; sleep 1; echo end >> {root}/order; fi\nif [ \"$1\" = fast ] && [ -e red-x ] && [ -e red-y ]; then echo FAIL; exit 1; fi\nif [ \"$1\" = slow ] && [ -e {root}/hold ]; then touch {root}/slowing; while [ ! -e {root}/release ]; do sleep 0.1; done; fi\nif [ \"$1\" = slow ] && ls | grep -q '^breaks'; then echo FAIL; exit 1; fi\necho ok\n",
        ran = ran.display(),
        listed = root.join("listed").display(),
        root = root.display(),
    );
    std::fs::write(work.join("gate.sh"), script).unwrap();
    let executable = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(work.join("gate.sh"), executable).unwrap();
    git(&work, &["add", "gate.md", "gate.sh"]);
    git(&work, &["commit", "--quiet", "-m", "a repo with a gate"]);
    git(&work, &["remote", "add", "origin", remote.to_str().unwrap()]);
    git(&work, &["push", "--quiet", "origin", "main"]);
    (work, ran)
}

/// A change on the branch `source`, adding `file`, pushed: one that asks to
/// land on `main`.
fn change(work: &Path, number: u64, source: &str, file: &str) -> Change {
    git(work, &["checkout", "--quiet", "-b", source, "main"]);
    std::fs::write(work.join(file), source).unwrap();
    git(work, &["add", file]);
    git(work, &["commit", "--quiet", "-m", &format!("add {file}")]);
    git(work, &["push", "--quiet", "origin", source]);
    git(work, &["checkout", "--quiet", "main"]);
    Change { number, source: source.into(), target: "main".into(), title: format!("Add {file}") }
}

/// What a candidate's commit declares of its gate, read off it.
fn declared(work: &Path, commit: &str) -> Result<Declared, String> {
    let text = git(work, &["show", &format!("{commit}:gate.md")]);
    let gates = declared::gates(&text, 180, None).map_err(|fault| fault.to_string())?;
    Ok(Declared { whole: "./gate.sh".into(), seconds: 180, gates })
}

/// The kind of machine every gate here runs on.
const MACHINE: &str = "scratch-kind";

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn lander<'a>(work: &Path, worktrees: &Path, forge: &'a Fake) -> Lander<'a, Fake> {
    lander_as(work, worktrees, forge, "s")
}

/// A process of this host is running where it is this test's own, under
/// the start time it is named with here: a run named otherwise has ended.
fn running(p: &Process) -> bool {
    p.pid == std::process::id() && p.started == "s"
}

/// A lander run as the process this test's pid names under `started`: one
/// of a run that has ended, unless `started` is this test's own.
fn lander_as<'a>(
    work: &Path,
    worktrees: &Path,
    forge: &'a Fake,
    started: &str,
) -> Lander<'a, Fake> {
    Lander {
        git: Git { worktree: work.into(), remote: "origin".into(), layout: Layout::Hidden },
        forge,
        host: "scratch-host".into(),
        machine: MACHINE.into(),
        owner: Process {
            host: "scratch-host".into(),
            pid: std::process::id(),
            started: started.into(),
        },
        running,
        worktrees: worktrees.into(),
        declared,
        now,
    }
}

/// The chain of `main` as the remote records it.
fn recorded(work: &Path) -> Chain {
    let name = turn::record_ref(Layout::Hidden, "main");
    git(work, &["fetch", "--quiet", "origin", &format!("+{name}:{name}")]);
    serde_json::from_str(&git(work, &["show", "-s", "--format=%B", &name])).unwrap()
}

/// Rewrites the chain of `main` on the remote by `edit`: as a run that
/// ended mid-way left it.
fn rewrite(work: &Path, edit: impl FnOnce(&mut Chain)) {
    let name = turn::record_ref(Layout::Hidden, "main");
    let mut chain = recorded(work);
    edit(&mut chain);
    let tree = git(work, &["hash-object", "-t", "tree", "/dev/null"]);
    let message = serde_json::to_string(&chain).unwrap();
    let mut args = vec!["commit-tree", &tree, "-m", &message, "-p", &name];
    let commits: Vec<String> = chain.candidates.iter().map(|c| c.commit.clone()).collect();
    for commit in &commits {
        args.extend(["-p", commit]);
    }
    let next = git(work, &args);
    git(work, &["push", "--quiet", "origin", &format!("{next}:{name}")]);
}

/// The chain holds no two valid candidates of one change.
fn one_each(chain: &Chain) {
    let mut valid: Vec<u64> =
        chain.chained().into_iter().filter(|c| c.valid()).map(|c| c.change).collect();
    let all = valid.len();
    valid.sort_unstable();
    valid.dedup();
    assert_eq!(valid.len(), all, "two valid candidates of one change: {chain:?}");
}

/// Which gate ran on which commit, in the order they ran.
fn runs(ran: &Path) -> Vec<(String, String)> {
    let text = std::fs::read_to_string(ran).unwrap_or_default();
    text.lines()
        .filter_map(|line| line.split_once(' '))
        .map(|(gate, commit)| (gate.to_string(), commit.to_string()))
        .collect()
}

/// The candidates of the chain, without the pairs beside it.
fn chained(chain: &Chain) -> Vec<Candidate> {
    chain.chained().into_iter().cloned().collect()
}

/// The pairs of the round's pairing trees the chain records.
fn paired(chain: &Chain) -> Vec<Candidate> {
    chain.candidates.iter().filter(|c| c.is_pair()).cloned().collect()
}

#[test]
fn two_changes_land_in_one_round_through_one_pair_and_one_whole_gate() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    let base = git(&work, &["rev-parse", "main"]);
    let a = change(&work, 1, "a", "a.txt");
    let b = change(&work, 2, "b", "b.txt");
    let heads = (git(&work, &["rev-parse", "a"]), git(&work, &["rev-parse", "b"]));
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let lander = lander(&work, &worktrees, &forge);

    let outcomes = lander.land(&[a, b]).unwrap();

    let chain = recorded(&work);
    let [first, second] = &chained(&chain)[..] else { panic!("{chain:?}") };
    let [pair] = &paired(&chain)[..] else { panic!("{chain:?}") };
    assert_eq!(
        outcomes,
        [(1, Outcome::Landed(first.commit.clone())), (2, Outcome::Landed(second.commit.clone()))]
    );
    assert!(chained(&chain).iter().all(|c| c.phase == Phase::Landed && c.stopped && c.batch));
    // The pair joins the two heads, and its fast gate ran; the root merged
    // onto the tip holds the pair's tree, so the whole gate's fast part is
    // the pair's run, reused (docs/adr/0069), and the slow part runs once.
    let parents = |commit: &str| git(&work, &["show", "-s", "--format=%P", commit]);
    assert_eq!(parents(&pair.commit), format!("{} {}", heads.0, heads.1));
    assert_eq!(pair.change, 2, "the later-asked PR's turn gates it");
    assert_eq!(pair.tree, second.tree);
    assert_eq!(
        runs(&ran),
        [("fast".to_string(), pair.commit.clone()), ("slow".to_string(), second.commit.clone())]
    );
    assert!(!first.slowed && second.slowed);
    // The target fast-forwarded through both candidates, in order: each a
    // merge of its change's original head onto the one before it.
    git(&work, &["fetch", "--quiet", "origin"]);
    let tip = git(&work, &["rev-parse", "origin/main"]);
    assert_eq!(tip, second.commit);
    assert_eq!(parents(&second.commit), format!("{} {}", first.commit, heads.1));
    assert_eq!(parents(&first.commit), format!("{base} {}", heads.0));
    assert_eq!(git(&work, &["show", "-s", "--format=%s", &first.commit]), "Add a.txt (!1)");
    // The forge was told each landed, in order; no source head moved.
    assert_eq!(*forge.landed.borrow(), [(1, tip.clone()), (2, tip)]);
    assert_eq!(
        (git(&work, &["rev-parse", "origin/a"]), git(&work, &["rev-parse", "origin/b"])),
        heads
    );
    assert!(chain.candidates.iter().all(|c| !worktrees.join(&c.id).exists()));
    assert_eq!(chain.round, None, "the round closed");
    assert!(paired(&chain).iter().all(|p| !p.valid()), "its pair ended with it");
}

#[test]
fn a_candidate_s_worktree_goes_as_its_gate_run_ends() {
    let root = tempfile::tempdir().unwrap();
    let (work, _) = scratch(root.path());
    let a = change(&work, 1, "a", "a.txt");
    let b = change(&work, 2, "b", "b.txt");
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");

    lander(&work, &worktrees, &forge).land(&[a, b]).unwrap();

    // Every run, the pair's fast gate and the root's slow gate, saw the
    // checkout and its own worktree alone (sunfmin/control#1063).
    let listed = std::fs::read_to_string(root.path().join("listed")).unwrap();
    assert_eq!(listed, "fast 2\nslow 2\n");
    let chain = recorded(&work);
    assert!(chain.candidates.iter().all(|c| !Path::new(&c.worktree).exists()));
    let all = git(&work, &["worktree", "list", "--porcelain"]);
    assert_eq!(all.lines().filter(|line| line.starts_with("worktree ")).count(), 1, "{all}");
}

#[test]
fn a_root_whose_tree_is_green_on_record_runs_no_second_fast_gate() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    // The change's head already holds the target's tip, and its fast gate
    // ran green on it: the root merged onto the tip has that head's tree.
    let a = change(&work, 1, "a", "a.txt");
    let head = git(&work, &["rev-parse", "a"]);
    git(&work, &["checkout", "--quiet", "a"]);
    let checkout = Git { worktree: work.clone(), remote: "origin".into(), layout: Layout::Hidden };
    let fast = GateCommand::new("./gate.sh fast");
    let limit = std::time::Duration::from_secs(60);
    let worker = run::gate(&checkout, &fast, ("worker-host", MACHINE), now(), limit, &[]).unwrap();
    assert!(matches!(worker.settled, Settled::Written { not_recorded: None, .. }), "{worker:?}");
    git(&work, &["checkout", "--quiet", "main"]);
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");

    let outcomes = lander(&work, &worktrees, &forge).land(std::slice::from_ref(&a)).unwrap();

    let chain = recorded(&work);
    let [only] = &chained(&chain)[..] else { panic!("{chain:?}") };
    assert!(paired(&chain).is_empty(), "a round of one is its own root");
    assert_eq!(outcomes, [(1, Outcome::Landed(only.commit.clone()))]);
    assert_eq!(only.tree, git(&work, &["rev-parse", &format!("{head}^{{tree}}")]));
    // Its fast gate is the worker's run, reused: only the slow gate ran.
    assert_eq!(runs(&ran), [("fast".to_string(), head), ("slow".to_string(), only.commit.clone())]);
    let evidence = only.result.as_ref().expect("its evidence");
    assert!(evidence.green(), "{evidence:?}");

    // Asked again with its whole gate on record, nothing runs at all.
    let b = change(&work, 2, "b", "b.txt");
    git(&work, &["checkout", "--quiet", "b"]);
    git(&work, &["merge", "--quiet", "--no-edit", "origin/main"]);
    git(&work, &["push", "--quiet", "origin", "b"]);
    git(&work, &["checkout", "--quiet", "--detach", "b"]);
    let whole = GateCommand::new("./gate.sh");
    for gate in [fast, GateCommand::new("./gate.sh slow")] {
        run::gate(&checkout, &gate, ("worker-host", MACHINE), now(), limit, &[]).unwrap();
    }
    let tree = afkconvoy_gate::gate::Tree::named(&git(&work, &["rev-parse", "HEAD^{tree}"]));
    let parts = (&whole, &GateCommand::new("./gate.sh fast"), &GateCommand::new("./gate.sh slow"));
    let b_head = git(&work, &["rev-parse", "HEAD"]);
    assert_eq!(run::record_whole(&checkout, &tree, &b_head, parts, &[], now()).unwrap(), None);
    git(&work, &["checkout", "--quiet", "main"]);
    let before = runs(&ran).len();

    let outcomes = lander(&work, &worktrees, &forge).land(std::slice::from_ref(&b)).unwrap();

    assert!(matches!(outcomes[..], [(2, Outcome::Landed(_))]), "{outcomes:?}");
    assert_eq!(runs(&ran).len(), before, "the whole gate's run on record answered");
    let chain = recorded(&work);
    let last = chained(&chain).pop().unwrap();
    let reused = last.result.as_ref().and_then(|e| e.reused.as_ref()).expect("reused");
    assert_eq!(reused.host.as_deref(), Some("worker-host"), "it names the run it stands on");
    assert_eq!(reused.machine.as_deref(), Some(MACHINE));
}

#[test]
fn a_red_root_lands_nothing_and_is_not_bisected() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    let base = git(&work, &["rev-parse", "main"]);
    let breaking = change(&work, 1, "a", "breaks");
    let other = change(&work, 2, "b", "b.txt");
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let lander = lander(&work, &worktrees, &forge);

    let outcomes = lander.land(&[breaking, other]).unwrap();

    assert_eq!(
        outcomes,
        [(1, Outcome::Decided(Decision::RoundRed)), (2, Outcome::Decided(Decision::Red))],
        "the round is not split: its last-asked PR keeps the failure"
    );
    let chain = recorded(&work);
    assert!(chain.candidates.iter().all(|c| !c.valid()), "{chain:?}");
    let [first, last] = &chained(&chain)[..] else { panic!("{chain:?}") };
    assert_eq!(first.cancelled_by.as_ref(), Some(&last.id));
    let gates: Vec<String> = runs(&ran).into_iter().map(|(gate, _)| gate).collect();
    assert_eq!(gates, ["fast", "slow"], "the pair's fast gate, the root's slow gate once");
    git(&work, &["fetch", "--quiet", "origin"]);
    assert_eq!(git(&work, &["rev-parse", "origin/main"]), base, "nothing landed");
    assert!(forge.landed.borrow().is_empty());
}

/// The changes numbered 1 to `n`, each on a branch of its own adding a file
/// of its own; the ones in `reds` break the slow gate.
fn changes(work: &Path, n: u64, reds: &[u64]) -> Vec<Change> {
    (1..=n)
        .map(|number| {
            let file = if reds.contains(&number) {
                format!("breaks-{number}")
            } else {
                format!("file-{number}.txt")
            };
            change(work, number, &format!("change-{number}"), &file)
        })
        .collect()
}

/// The files on the remote's `main`.
fn landed_files(work: &Path) -> Vec<String> {
    git(work, &["fetch", "--quiet", "origin"]);
    let listed = git(work, &["ls-tree", "--name-only", "origin/main"]);
    listed
        .lines()
        .filter(|f| f.starts_with("file-") || f.starts_with("breaks") || f.starts_with("red-"))
        .map(String::from)
        .collect()
}

/// What became of each change, with what landed told by its number alone.
fn ended(outcomes: Vec<(u64, Outcome<String>)>) -> Vec<(u64, Option<Decision>)> {
    outcomes
        .into_iter()
        .map(|(number, outcome)| match outcome {
            Outcome::Landed(_) => (number, Some(Decision::Landed)),
            Outcome::Decided(decision) => (number, Some(decision)),
            Outcome::Conflict => (number, None),
            other => panic!("change {number}: {other:?}"),
        })
        .collect()
}

/// Four ready, mutually clean changes: two pairs, fast-gated at once, then
/// their pair, then the root merged onto the tip, gated whole once; all four
/// land in one fast-forward, and the forge is told each landed.
#[test]
fn four_clean_changes_pair_twice_gate_their_pair_and_land_in_one_fast_forward() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    let base = git(&work, &["rev-parse", "main"]);
    let asked = changes(&work, 4, &[]);
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    std::fs::write(root.path().join("concurrent"), "").unwrap();

    let outcomes = lander(&work, &worktrees, &forge).land(&asked).unwrap();

    assert_eq!(ended(outcomes), (1..=4).map(|n| (n, Some(Decision::Landed))).collect::<Vec<_>>());
    let chain = recorded(&work);
    let pairs = paired(&chain);
    let levels: Vec<(u32, u64)> =
        pairs.iter().map(|p| (p.pair.as_ref().unwrap().level, p.change)).collect();
    assert_eq!(levels, [(0, 2), (0, 4), (1, 4)], "two pairs, then their pair");
    // The two pairs of level 0 gated at once: both started before either ended.
    let order = std::fs::read_to_string(root.path().join("order")).unwrap();
    assert!(order.starts_with("start\nstart\nend\nend\n"), "{order}");
    let chained = chained(&chain);
    let tip = chained.last().unwrap();
    assert_eq!(chained.len(), 4);
    assert!(chained.iter().all(|c| c.phase == Phase::Landed && c.batch));
    let gates: Vec<(String, String)> = runs(&ran);
    let slow: Vec<&(String, String)> = gates.iter().filter(|(g, _)| g == "slow").collect();
    assert_eq!(slow, [&("slow".to_string(), tip.commit.clone())], "gated whole once");
    assert_eq!(gates.len(), 4, "three pairs' fast gates, the root's reused, one slow: {gates:?}");
    git(&work, &["fetch", "--quiet", "origin"]);
    assert_eq!(git(&work, &["rev-parse", "origin/main"]), tip.commit, "one fast-forward");
    let history = git(&work, &["rev-list", "--first-parent", &format!("{base}..origin/main")]);
    assert_eq!(history.lines().count(), 4, "one merge commit for each change");
    let told: Vec<u64> = forge.landed.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(told, [1, 2, 3, 4]);
    assert!(forge.sent_back.borrow().is_empty());
}

/// A pair red on its fast gate stops no other pair: its later-asked change
/// is sent back with the red run, its earlier one goes up unpaired, and the
/// rest of the round lands.
#[test]
fn a_red_pair_sends_its_later_change_back_and_the_rest_of_the_round_lands() {
    let root = tempfile::tempdir().unwrap();
    let (work, _) = scratch(root.path());
    // Each of the first two passes alone; together they break the fast gate.
    let asked = vec![
        change(&work, 1, "change-1", "red-x"),
        change(&work, 2, "change-2", "red-y"),
        change(&work, 3, "change-3", "file-3.txt"),
        change(&work, 4, "change-4", "file-4.txt"),
    ];
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");

    let outcomes = lander(&work, &worktrees, &forge).land(&asked).unwrap();

    assert_eq!(
        ended(outcomes),
        [
            (1, Some(Decision::Landed)),
            (2, Some(Decision::Red)),
            (3, Some(Decision::Landed)),
            (4, Some(Decision::Landed)),
        ]
    );
    let sent = forge.sent_back.borrow();
    let [(2, Some(run))] = &sent[..] else { panic!("{sent:?}") };
    assert!(run.excerpt.iter().any(|line| line == "FAIL"), "sent back with its red run");
    assert_eq!(landed_files(&work), ["file-3.txt", "file-4.txt", "red-x"]);
    let chain = recorded(&work);
    let levels: Vec<(u32, Vec<u64>, Vec<u64>)> = paired(&chain)
        .iter()
        .map(|p| {
            let q = p.pair.as_ref().unwrap();
            (q.level, q.first.clone(), q.second.clone())
        })
        .collect();
    assert_eq!(
        levels,
        [(0, vec![1], vec![2]), (0, vec![3], vec![4]), (1, vec![1], vec![3, 4])],
        "1 went up unpaired, and paired with 3 and 4"
    );
}

/// An odd change goes up a level unpaired; one that merges cleanly with no
/// node of its level leaves the round, and the next round finds it
/// conflicting with the target.
#[test]
fn an_odd_change_goes_up_unpaired_and_one_clean_with_none_leaves_for_the_next_round() {
    let root = tempfile::tempdir().unwrap();
    let (work, _) = scratch(root.path());
    let asked = changes(&work, 3, &[]);
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");

    let outcomes = lander(&work, &worktrees, &forge).land(&asked).unwrap();

    assert_eq!(ended(outcomes), (1..=3).map(|n| (n, Some(Decision::Landed))).collect::<Vec<_>>());
    let chain = recorded(&work);
    let levels: Vec<(u32, Vec<u64>, Vec<u64>)> = paired(&chain)
        .iter()
        .map(|p| {
            let q = p.pair.as_ref().unwrap();
            (q.level, q.first.clone(), q.second.clone())
        })
        .collect();
    assert_eq!(levels, [(0, vec![1], vec![2]), (1, vec![1, 2], vec![3])]);

    // 6 conflicts with 4: it pairs with none, and leaves the round.
    let asked = vec![
        change(&work, 4, "change-4", "file-4.txt"),
        change(&work, 5, "change-5", "file-5.txt"),
        change(&work, 6, "change-6", "file-4.txt"),
    ];
    let outcomes = lander(&work, &worktrees, &forge).land(&asked).unwrap();

    assert_eq!(
        ended(outcomes),
        [(4, Some(Decision::Landed)), (5, Some(Decision::Landed)), (6, None)],
        "6 conflicts with the target in the next round"
    );
}

/// What the crate depends on: Flux's attributes, serde and the gate crate —
/// never afkconvoy-core, whose fleet concepts its API is to stay free of.
#[test]
fn the_crate_depends_on_nothing_of_the_fleet() {
    let manifest = include_str!("../Cargo.toml");
    let section = manifest.split("[dependencies]").nth(1).unwrap().split("\n[").next().unwrap();
    let mut names: Vec<&str> = section
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once(['=', '.']).map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(
        names,
        ["afkconvoy-gate", "flux-alloc", "flux-core", "flux-rs", "serde", "serde_json"]
    );
}

/// The crate's public API names no fleet concept: none of its public items
/// and fields is named for a team, role, stage, issue, claim, patrol,
/// intake, tracker, GitHub or a pull request.
#[test]
fn its_public_api_names_no_fleet_concept() {
    const FLEET: [&str; 11] = [
        "team", "role", "stage", "issue", "claim", "patrol", "intake", "tracker", "github", "pr",
        "pull",
    ];
    let sources = [
        include_str!("../src/lib.rs"),
        include_str!("../src/chain.rs"),
        include_str!("../src/forge.rs"),
        include_str!("../src/land.rs"),
        include_str!("../src/lease.rs"),
        include_str!("../src/pairing.rs"),
        include_str!("../src/turn.rs"),
    ];
    for source in sources {
        let public = source.split("#[cfg(test)]").next().unwrap();
        for line in public.lines().map(str::trim).filter(|line| line.starts_with("pub ")) {
            let lower = line.to_lowercase();
            let words: Vec<&str> = lower
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            for concept in FLEET {
                let named = |word: &&str| match concept {
                    // `pr` alone, not `predecessor` or `process`.
                    "pr" => *word == "pr" || word.starts_with("prs"),
                    _ => word.starts_with(concept),
                };
                assert!(!words.iter().any(named), "`{line}` names a fleet concept: {concept}");
            }
        }
    }
}

/// A run assigns a candidate, gates it green — its fast gate and the round
/// at it — and ends before it publishes (sunfmin/control#900). A reader of
/// the chain names it wedged once it stood past its whole gate's seconds;
/// the next run lands it, gates nothing again, and assigns no second
/// candidate of the change.
#[test]
fn a_green_candidate_a_run_left_unpublished_lands_at_the_next_run() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    let a = change(&work, 1, "a", "file-1.txt");
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let ended = lander_as(&work, &worktrees, &forge, "an ended run");
    let Ok(id) = ended.assign(&a).unwrap() else { panic!("assigned") };
    ended.gate("main", &id).unwrap();
    assert!(ended.round("main", &id).unwrap());

    let chain = recorded(&work);
    let [left] = &chain.candidates[..] else { panic!("{chain:?}") };
    assert!(left.whole() && left.stopped, "{left:?}");
    let assigned = left.assigned.expect("it says when it was assigned");
    assert!(chain.wedged(assigned + left.limit()).is_empty(), "within its gate's seconds");
    let wedged: Vec<&str> =
        chain.wedged(assigned + left.limit() + 1).iter().map(|c| c.id.as_str()).collect();
    assert_eq!(wedged, [id.as_str()], "past them, unpublished: wedged");

    let outcomes = lander(&work, &worktrees, &forge).land(&[a]).unwrap();

    let chain = recorded(&work);
    let [landed] = &chain.candidates[..] else { panic!("one candidate: {chain:?}") };
    assert_eq!(landed.id, id, "the candidate it left, not a second");
    assert_eq!(outcomes, [(1, Outcome::Landed(landed.commit.clone()))]);
    assert_eq!(landed.phase, Phase::Landed);
    let gates: Vec<String> = runs(&ran).into_iter().map(|(gate, _)| gate).collect();
    assert_eq!(gates, ["fast", "slow"], "nothing gated again");
    git(&work, &["fetch", "--quiet", "origin"]);
    assert_eq!(git(&work, &["rev-parse", "origin/main"]), landed.commit);
    assert_eq!(*forge.landed.borrow(), [(1, landed.commit.clone())]);
    assert!(chain.wedged(u64::MAX / 2).is_empty(), "a landed candidate is not wedged");
}

/// A run ends while its candidate is gating, on this host, which lives on:
/// the next run takes it over — `owner_died`, with what stands on it — and
/// the changes land on candidates of its own (sunfmin/control#900).
#[test]
fn a_candidate_left_gating_by_a_run_that_ended_is_taken_over_and_the_changes_land() {
    let root = tempfile::tempdir().unwrap();
    let (work, _) = scratch(root.path());
    let a = change(&work, 1, "a", "file-1.txt");
    let b = change(&work, 2, "b", "file-2.txt");
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let ended = lander_as(&work, &worktrees, &forge, "an ended run");
    let Ok(left) = ended.assign(&a).unwrap() else { panic!("assigned") };
    let Ok(behind) = ended.assign(&b).unwrap() else { panic!("assigned") };
    rewrite(&work, |chain| chain.candidates[0].phase = Phase::Validating);

    let outcomes = lander(&work, &worktrees, &forge).land(&[a, b]).unwrap();

    let chain = recorded(&work);
    one_each(&chain);
    let by = |id: &str| chain.candidates.iter().find(|c| c.id == id).unwrap();
    let (left, behind) = (by(&left), by(&behind));
    assert_eq!(
        (left.phase, left.cause.as_deref(), left.abandoned),
        (Phase::Cancelled, Some("owner_died"), true)
    );
    assert_eq!(behind.cancelled_by.as_deref(), Some(left.id.as_str()));
    let landed: Vec<&str> = chain
        .candidates
        .iter()
        .filter(|c| c.phase == Phase::Landed)
        .map(|c| c.id.as_str())
        .collect();
    assert_eq!(landed.len(), 2, "{chain:?}");
    assert!(outcomes.iter().all(|(_, o)| matches!(o, Outcome::Landed(_))), "{outcomes:?}");
    assert_eq!(landed_files(&work), ["file-1.txt", "file-2.txt"]);
}

/// Whatever runs come and go, the chain holds no two valid candidates of
/// one change (sunfmin/control#900): a run asking again takes up the
/// candidate the change has; a head that moved invalidates it first.
#[test]
fn the_chain_never_holds_two_valid_candidates_of_one_change() {
    let root = tempfile::tempdir().unwrap();
    let (work, _) = scratch(root.path());
    let a = change(&work, 1, "a", "file-1.txt");
    let b = change(&work, 2, "b", "file-2.txt");
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let first = lander_as(&work, &worktrees, &forge, "a first run");
    let assigned = |lander: &Lander<Fake>, change: &Change| {
        let Ok(id) = lander.assign(change).unwrap() else { panic!("assigned") };
        one_each(&recorded(&work));
        id
    };
    let (a1, b1) = (assigned(&first, &a), assigned(&first, &b));
    assert_eq!(assigned(&first, &a), a1, "asked again, the same candidate");
    // The first run gates both fast-green and ends.
    first.gate("main", &a1).unwrap();
    first.gate("main", &b1).unwrap();
    let second = lander_as(&work, &worktrees, &forge, "a second run");
    assert_eq!(assigned(&second, &b), b1, "another run takes up the one it has");
    assert_eq!(assigned(&second, &a), a1);
    let live = lander(&work, &worktrees, &forge);

    // The change's head moves: its candidate goes, with what stands on it.
    git(&work, &["checkout", "--quiet", "a"]);
    std::fs::write(work.join("file-1.txt"), "moved").unwrap();
    git(&work, &["commit", "--quiet", "-am", "move a"]);
    git(&work, &["push", "--quiet", "origin", "a"]);
    git(&work, &["checkout", "--quiet", "main"]);
    let a2 = assigned(&live, &a);
    assert_ne!(a2, a1);
    let chain = recorded(&work);
    let old = chain.candidates.iter().find(|c| c.id == a1).unwrap();
    assert_eq!(old.cause.as_deref(), Some("head_changed"));
    assert!(!chain.candidates.iter().find(|c| c.id == b1).unwrap().valid());
    let b2 = assigned(&live, &b);
    assert_ne!(b2, b1);

    let outcomes = live.land(&[a, b]).unwrap();

    let chain = recorded(&work);
    one_each(&chain);
    let landed = |n: u64| {
        let c = chain.chained().into_iter().find(|c| c.change == n && c.phase == Phase::Landed);
        let c = c.unwrap_or_else(|| panic!("{outcomes:?} {chain:#?}"));
        c.commit.clone()
    };
    assert_eq!(outcomes, [(1, Outcome::Landed(landed(1))), (2, Outcome::Landed(landed(2)))]);
    assert_eq!(landed_files(&work), ["file-1.txt", "file-2.txt"]);
}

/// A change with no branch of its own, its head at the full ref `name`
/// (`refs/pull/7/head`, as a forge gives a pull pushed by AGit flow), adding
/// `file`, pushed there: one that asks to land on `main`.
fn change_at_ref(work: &Path, number: u64, name: &str, file: &str) -> Change {
    git(work, &["checkout", "--quiet", "--detach", "main"]);
    std::fs::write(work.join(file), name).unwrap();
    git(work, &["add", file]);
    git(work, &["commit", "--quiet", "-m", &format!("add {file}")]);
    git(work, &["push", "--quiet", "origin", &format!("HEAD:{name}")]);
    git(work, &["checkout", "--quiet", "main"]);
    Change { number, source: name.into(), target: "main".into(), title: format!("Add {file}") }
}

/// Moves the head at the full ref `name` on by one commit, pushed.
fn move_ref(work: &Path, name: &str) {
    git(work, &["fetch", "--quiet", "origin", name]);
    git(work, &["checkout", "--quiet", "--detach", "FETCH_HEAD"]);
    std::fs::write(work.join("moved.txt"), "moved").unwrap();
    git(work, &["add", "moved.txt"]);
    git(work, &["commit", "--quiet", "-m", "move"]);
    git(work, &["push", "--quiet", "--force", "origin", &format!("HEAD:{name}")]);
    git(work, &["checkout", "--quiet", "main"]);
}

/// A change whose source is a full ref, not under `refs/heads/`, lands as a
/// branch's does beside one whose source is a bare branch name
/// (sunfmin/control#955); its ref is not moved.
#[test]
fn a_change_whose_source_is_a_full_ref_lands_as_a_branch_s_does() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    let a = change_at_ref(&work, 7, "refs/pull/7/head", "file-1.txt");
    let b = change(&work, 8, "b", "file-2.txt");
    let head = git(&work, &["ls-remote", "origin", "refs/pull/7/head"]);
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");

    let outcomes = lander(&work, &worktrees, &forge).land(&[a, b]).unwrap();

    let chain = recorded(&work);
    let [first, second] = &chained(&chain)[..] else { panic!("{chain:?}") };
    assert_eq!(first.branch, "refs/pull/7/head");
    assert_eq!(second.branch, "b", "a bare source is still a branch");
    assert_eq!(
        outcomes,
        [(7, Outcome::Landed(first.commit.clone())), (8, Outcome::Landed(second.commit.clone()))]
    );
    assert_eq!(runs(&ran).len(), 2, "their pair fast-gated, one slow gate at the root");
    assert_eq!(landed_files(&work), ["file-1.txt", "file-2.txt"]);
    assert_eq!(git(&work, &["ls-remote", "origin", "refs/pull/7/head"]), head);
    assert!(
        git(&work, &["ls-remote", "origin", "refs/heads/refs/pull/7/head"]).is_empty(),
        "no branch made of the ref"
    );
}

/// The full ref a candidate merged moves before it lands: the publication's
/// evidence changed, and it is invalidated, as a branch's candidate is.
#[test]
fn a_candidate_of_a_full_ref_is_invalidated_when_that_ref_moves() {
    let root = tempfile::tempdir().unwrap();
    let (work, _) = scratch(root.path());
    let a = change_at_ref(&work, 7, "refs/pull/7/head", "file-1.txt");
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let lander = lander(&work, &worktrees, &forge);
    let Ok(id) = lander.assign(&a).unwrap() else { panic!("assigned") };
    lander.gate("main", &id).unwrap();
    assert!(lander.round("main", &id).unwrap());

    move_ref(&work, "refs/pull/7/head");
    let landed = lander.publish("main", &id, std::slice::from_ref(&a)).unwrap();

    assert!(landed.is_empty(), "{landed:?}");
    let chain = recorded(&work);
    let c = chain.candidates.iter().find(|c| c.id == id).unwrap();
    assert_eq!(c.cause.as_deref(), Some("publication_evidence_changed"), "{c:?}");
    assert!(forge.landed.borrow().is_empty());
}

/// A change whose source the remote has no ref at is that change's outcome
/// alone, `Gone`: the round lands the others (sunfmin/control#955).
#[test]
fn a_change_whose_source_the_remote_has_not_is_gone_and_the_rest_lands() {
    let root = tempfile::tempdir().unwrap();
    let (work, _) = scratch(root.path());
    let missing = Change {
        number: 2,
        source: "refs/pull/2/head".into(),
        target: "main".into(),
        title: "Gone".into(),
    };
    let b = change(&work, 3, "b", "file-2.txt");
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");

    let outcomes = lander(&work, &worktrees, &forge).land(&[missing, b]).unwrap();

    let chain = recorded(&work);
    let [landed] = &chain.candidates[..] else { panic!("{chain:?}") };
    assert_eq!(outcomes, [(2, Outcome::Gone), (3, Outcome::Landed(landed.commit.clone()))]);
    assert_eq!(landed_files(&work), ["file-2.txt"]);
}

/// Makes the remote refuse every push to its `main` with `reason`, or, where
/// `None`, take them again: a pre-receive hook.
fn refuse_main(work: &Path, reason: Option<&str>) {
    let hooks = PathBuf::from(git(work, &["remote", "get-url", "origin"])).join("hooks");
    let hook = hooks.join("pre-receive");
    let Some(reason) = reason else {
        std::fs::remove_file(hook).unwrap();
        return;
    };
    let script = format!(
        "#!/bin/sh\nwhile read old new name; do\n  if [ \"$name\" = refs/heads/main ]; then echo '{reason}' >&2; exit 1; fi\ndone\n"
    );
    std::fs::create_dir_all(&hooks).unwrap();
    std::fs::write(&hook, script).unwrap();
    let executable = std::os::unix::fs::PermissionsExt::from_mode(0o755);
    std::fs::set_permissions(&hook, executable).unwrap();
}

/// A run's push that would land a green round is refused by the remote
/// (sunfmin/control#998): each change it would have landed is unpublished,
/// with what git said, never waiting; its candidate stays green on the
/// chain, and the next run whose push takes lands it, gating nothing again.
#[test]
fn a_publication_whose_push_is_refused_is_unpublished_with_git_s_error_and_lands_at_the_next_run() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    let asked = changes(&work, 2, &[]);
    let base = git(&work, &["rev-parse", "main"]);
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let lander = lander(&work, &worktrees, &forge);
    refuse_main(&work, Some("main is not yours to push"));

    let outcomes = lander.land(&asked).unwrap();

    assert_eq!(outcomes.len(), 2, "{outcomes:?}");
    for (_, outcome) in &outcomes {
        let Outcome::Unpublished(said) = outcome else { panic!("{outcomes:?}") };
        assert!(said.contains("main is not yours to push"), "{said}");
    }
    let chain = recorded(&work);
    assert!(chained(&chain).iter().all(|c| c.valid()), "{chain:?}");
    let tip = chained(&chain).last().unwrap().clone();
    assert!(chain.ripe(&tip.id, &base), "still ripe: {chain:?}");
    git(&work, &["fetch", "--quiet", "origin"]);
    assert_eq!(git(&work, &["rev-parse", "origin/main"]), base, "nothing landed");
    assert!(forge.landed.borrow().is_empty());
    let gated = runs(&ran);

    refuse_main(&work, None);
    let outcomes = lander.land(&asked).unwrap();

    let chain = recorded(&work);
    let commits: Vec<String> = chained(&chain).iter().map(|c| c.commit.clone()).collect();
    assert_eq!(commits.len(), 2, "no second candidate: {chain:?}");
    assert_eq!(
        outcomes,
        [(1, Outcome::Landed(commits[0].clone())), (2, Outcome::Landed(commits[1].clone()))]
    );
    assert_eq!(runs(&ran), gated, "nothing gated again");
    git(&work, &["fetch", "--quiet", "origin"]);
    assert_eq!(git(&work, &["rev-parse", "origin/main"]), tip.commit);
    let told: Vec<u64> = forge.landed.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(told, [1, 2]);
}

/// Another checkout of the scratch repo's remote, as another program's
/// process landing on it has.
fn another_checkout(root: &Path, name: &str) -> PathBuf {
    let remote = root.join("remote.git");
    let work = root.join(name);
    git(root, &["clone", "--quiet", remote.to_str().unwrap(), work.to_str().unwrap()]);
    for (key, value) in [("user.name", "someone"), ("user.email", "someone@example.com")] {
        git(&work, &["config", key, value]);
    }
    work
}

/// The chain of `main` as the remote holds it, read on the remote itself.
fn on_remote(root: &Path) -> Chain {
    let name = turn::record_ref(Layout::Hidden, "main");
    let text = git(&root.join("remote.git"), &["show", "-s", "--format=%B", &name]);
    serde_json::from_str(&text).unwrap()
}

/// Waits until `done`, at most a minute.
fn wait_for(what: &str, done: impl Fn() -> bool) {
    let until = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while !done() {
        assert!(std::time::Instant::now() < until, "waited a minute for {what}");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
}

/// The stacked merge of the chain `main` has, once its fast gate decided it.
fn stacked_merge(chain: &Chain) -> Option<Candidate> {
    let merges = chain.candidates.iter().filter(|c| c.pair.as_ref().is_some_and(|p| p.stacked));
    merges.into_iter().find(|c| c.phase != Phase::Building && c.phase != Phase::Validating).cloned()
}

/// Lands `first` in one run whose round's slow gate holds, and, once it
/// runs, `second` in another run, from a checkout of its own; lets the slow
/// gate go once the second run's stacked merge is gated → what became of
/// each, and the second run's stacked merge.
fn stacked_on(
    root: &Path,
    work: &Path,
    (first, second): (Change, Change),
) -> (
    Vec<(u64, Outcome<String>)>,
    Vec<(u64, Outcome<String>)>,
    Candidate,
    Vec<(u64, Option<Evidence>)>,
) {
    std::fs::write(root.join("hold"), "").unwrap();
    let other = another_checkout(root, "other");
    let worktrees = root.join("candidates");
    std::thread::scope(|scope| {
        let running = scope.spawn(|| {
            let forge = Fake::default();
            lander(work, &worktrees, &forge).land(&[first]).unwrap()
        });
        wait_for("the round's slow gate", || root.join("slowing").exists());
        let stacking = scope.spawn(|| {
            let forge = Fake::default();
            let outcomes = lander(&other, &worktrees, &forge).land(&[second]).unwrap();
            (outcomes, forge.sent_back.take())
        });
        wait_for("the stacked merge's fast gate", || stacked_merge(&on_remote(root)).is_some());
        let merge = stacked_merge(&on_remote(root)).unwrap();
        let chain = on_remote(root);
        assert!(chain.whole_running().is_some(), "the round's whole gate still runs: {chain:?}");
        std::fs::write(root.join("release"), "").unwrap();
        let first = running.join().unwrap();
        let (second, sent_back) = stacking.join().unwrap();
        (first, second, merge, sent_back)
    })
}

/// A change asked while another run's round runs its whole gate stacks on
/// that round's root: its merge onto the root-onto-tip commit is fast-gated
/// at once, the running gate left as it is; that round landed, the change's
/// own round starts from that merge's tree, its fast gate the stacked
/// merge's run, reused, and runs only its slow gate.
#[test]
fn a_change_asked_while_a_round_s_whole_gate_runs_stacks_on_its_root_and_runs_only_its_slow_gate() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    let a = change(&work, 1, "a", "a.txt");
    let b = change(&work, 2, "b", "b.txt");

    let (first, second, merge, sent_back) = stacked_on(root.path(), &work, (a, b));

    assert!(matches!(first[..], [(1, Outcome::Landed(_))]), "{first:?}");
    assert!(matches!(second[..], [(2, Outcome::Landed(_))]), "{second:?}");
    assert!(sent_back.is_empty());
    let chain = recorded(&work);
    let [ca, cb] = &chained(&chain)[..] else { panic!("{chain:?}") };
    assert_eq!(merge.predecessor, ca.commit, "stacked on the round's root-onto-tip commit");
    assert_eq!(merge.phase, Phase::Green);
    assert_eq!(cb.predecessor, ca.commit, "its round started from that commit as the tip");
    assert_eq!(cb.tree, merge.tree, "its root has the stacked merge's tree");
    assert_eq!(
        runs(&ran),
        [
            ("fast".to_string(), ca.commit.clone()),
            ("slow".to_string(), ca.commit.clone()),
            ("fast".to_string(), merge.commit.clone()),
            ("slow".to_string(), cb.commit.clone()),
        ],
        "no second fast run of the stacked merge's tree"
    );
    let reused = cb.result.as_ref().and_then(|e| e.reused.as_ref());
    assert!(reused.is_none(), "the slow gate ran: {cb:?}");
    git(&work, &["fetch", "--quiet", "origin"]);
    assert_eq!(git(&work, &["rev-parse", "origin/main"]), cb.commit);
    assert_eq!((chain.round.as_ref(), chain.stacked.as_ref()), (None, None));
}

/// The round a change stacked on ends red: the stacked merge is discarded,
/// nothing is charged to the change, and it pairs in the next round and
/// lands.
#[test]
fn a_change_stacked_on_a_round_that_ends_red_is_charged_nothing_and_lands_in_the_next_round() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    let a = change(&work, 1, "a", "breaks-a");
    let b = change(&work, 2, "b", "b.txt");
    let base = git(&work, &["rev-parse", "main"]);

    let (first, second, merge, sent_back) = stacked_on(root.path(), &work, (a, b));

    assert_eq!(ended(first), [(1, Some(Decision::Red))]);
    assert!(matches!(second[..], [(2, Outcome::Landed(_))]), "{second:?}");
    assert!(sent_back.is_empty(), "the red is not the stacked change's");
    let chain = recorded(&work);
    let merge = chain.candidates.iter().find(|c| c.id == merge.id).cloned().unwrap_or(merge);
    assert!(!merge.valid());
    let landed = chained(&chain).into_iter().find(|c| c.change == 2).unwrap();
    assert_eq!(landed.predecessor, base, "it paired anew on the target tip");
    let gates = runs(&ran);
    assert_eq!(gates.last(), Some(&("slow".to_string(), landed.commit.clone())));
    git(&work, &["fetch", "--quiet", "origin"]);
    assert_eq!(git(&work, &["rev-parse", "origin/main"]), landed.commit);
}
