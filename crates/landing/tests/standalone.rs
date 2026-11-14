//! The landing crate as another program uses it (docs/adr/0060): from
//! outside, through its public API alone, with a fake forge and no fleet
//! beside it. Two changes land on a local bare repo in one round — both
//! fast-green, one slow gate at the round's tip, the target fast-forwarded
//! through both candidates in order, and the forge told each landed. A red
//! round lands nothing and cancels the chain. In a batch round only the
//! chain's tip is gated, and a red one is bisected to its first red change,
//! which is sent back while the rest lands (docs/adr/0068).

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use afkconvoy_gate::declared;
use afkconvoy_gate::gate::GateCommand;
use afkconvoy_gate::layout::Layout;
use afkconvoy_gate::run::{self, Git, Settled};
use afkconvoy_landing::chain::{Chain, Decision, Evidence, Phase, Plain, Process};
use afkconvoy_landing::forge::{Change, Forge};
use afkconvoy_landing::land::{Lander, Outcome};
use afkconvoy_landing::turn::{self, Declared, Landing};

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
/// commit — and whose slow gate is red on a tree that holds `breaks`;
/// — or any file named `breaks…` — committed and pushed to a bare remote
/// beside it.
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
    let script = format!(
        "#!/bin/sh\necho \"$1 $(git rev-parse HEAD)\" >> {}\nif [ \"$1\" = slow ] && ls | grep -q '^breaks'; then echo FAIL; exit 1; fi\necho ok\n",
        ran.display()
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
    // This program's config: a line of its own says how its rounds land.
    let landing = if text.lines().any(|line| line == "landing: batch") {
        Landing::Batch
    } else {
        Landing::PerCandidate
    };
    Ok(Declared { whole: "./gate.sh".into(), seconds: 180, gates, landing })
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
        chain.candidates.iter().filter(|c| c.valid()).map(|c| c.change).collect();
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

#[test]
fn two_changes_land_in_one_round_with_one_slow_gate_at_its_tip() {
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
    let [first, second] = &chain.candidates[..] else { panic!("{chain:?}") };
    assert_eq!(
        outcomes,
        [(1, Outcome::Landed(first.commit.clone())), (2, Outcome::Landed(second.commit.clone()))]
    );
    assert!(chain.candidates.iter().all(|c| c.phase == Phase::Landed && c.stopped));
    // Both fast-green, and one slow gate, at the round's tip.
    assert_eq!(
        runs(&ran),
        [
            ("fast".to_string(), first.commit.clone()),
            ("fast".to_string(), second.commit.clone()),
            ("slow".to_string(), second.commit.clone()),
        ]
    );
    assert!(first.slow.is_some() && !first.slowed && second.slowed);
    // The target fast-forwarded through both candidates, in order: each a
    // merge of its change's original head onto the one before it.
    git(&work, &["fetch", "--quiet", "origin"]);
    let tip = git(&work, &["rev-parse", "origin/main"]);
    assert_eq!(tip, second.commit);
    let parents = |commit: &str| git(&work, &["show", "-s", "--format=%P", commit]);
    assert_eq!(parents(&second.commit), format!("{} {}", first.commit, heads.1));
    assert_eq!(parents(&first.commit), format!("{base} {}", heads.0));
    assert_eq!(git(&work, &["show", "-s", "--format=%s", &first.commit]), "Add a.txt (!1)");
    // The forge was told each landed, in order; no source head moved.
    assert_eq!(*forge.landed.borrow(), [(1, tip.clone()), (2, tip)]);
    assert_eq!(
        (git(&work, &["rev-parse", "origin/a"]), git(&work, &["rev-parse", "origin/b"])),
        heads
    );
    assert!(!worktrees.join(&first.id).exists() && !worktrees.join(&second.id).exists());
}

#[test]
fn a_candidate_whose_tree_is_green_on_record_lands_without_a_second_fast_gate() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    // The change's head already holds the target's tip, and its fast gate
    // ran green on it: its candidate's tree is that head's.
    let a = change(&work, 1, "a", "a.txt");
    let head = git(&work, &["rev-parse", "a"]);
    git(&work, &["checkout", "--quiet", "a"]);
    let checkout = Git { worktree: work.clone(), remote: "origin".into(), layout: Layout::Hidden };
    let fast = GateCommand::new("./gate.sh fast");
    let limit = std::time::Duration::from_secs(60);
    let worker = run::gate(&checkout, &fast, ("worker-host", MACHINE), now(), limit, &[]).unwrap();
    assert!(matches!(worker.settled, Settled::Written { not_recorded: None, .. }), "{worker:?}");
    git(&work, &["checkout", "--quiet", "main"]);
    let b = change(&work, 2, "b", "b.txt");
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");

    let outcomes = lander(&work, &worktrees, &forge).land(&[a, b]).unwrap();

    let chain = recorded(&work);
    let [first, second] = &chain.candidates[..] else { panic!("{chain:?}") };
    assert_eq!(
        outcomes,
        [(1, Outcome::Landed(first.commit.clone())), (2, Outcome::Landed(second.commit.clone()))]
    );
    assert_eq!(first.tree, git(&work, &["rev-parse", &format!("{head}^{{tree}}")]));
    // The first candidate's fast gate is the worker's run, reused: it did not run again.
    assert_eq!(
        runs(&ran),
        [
            ("fast".to_string(), head),
            ("fast".to_string(), second.commit.clone()),
            ("slow".to_string(), second.commit.clone()),
        ]
    );
    let evidence = first.result.as_ref().expect("its evidence");
    assert!(evidence.green(), "{evidence:?}");
    let reused = evidence.reused.as_ref().expect("its record says it reused the run");
    assert_eq!(reused.host.as_deref(), Some("worker-host"), "it names the run it stands on");
    assert_eq!(reused.machine.as_deref(), Some(MACHINE));
    assert_eq!(evidence.command.as_deref(), Some("./gate.sh fast"));
    assert_eq!(evidence.head.as_deref(), Some(first.commit.as_str()));
    assert!(second.result.as_ref().unwrap().reused.is_none(), "the slow gate ran");
}

#[test]
fn a_red_round_lands_nothing_and_cancels_the_chain() {
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
        "the round is not split: its tip keeps the failure"
    );
    let chain = recorded(&work);
    assert!(chain.candidates.iter().all(|c| !c.valid()), "{chain:?}");
    assert_eq!(chain.candidates[0].cancelled_by.as_ref(), Some(&chain.candidates[1].id));
    let gates: Vec<String> = runs(&ran).into_iter().map(|(gate, _)| gate).collect();
    assert_eq!(gates, ["fast", "fast", "slow"]);
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
        .filter(|f| f.starts_with("file-") || f.starts_with("breaks"))
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
            other => panic!("change {number}: {other:?}"),
        })
        .collect()
}

#[test]
fn a_green_batch_round_lands_on_one_fast_gate_and_one_slow_gate_at_its_tip() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    let asked = changes(&work, 4, &[]);
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let lander = lander(&work, &worktrees, &forge);

    // The repo declares no batch landing: its caller asks for one, for this
    // round, with no commit to the repo.
    let outcomes = lander.land_with(&asked, Some(Landing::Batch)).unwrap();

    let chain = recorded(&work);
    let tip = chain.candidates.last().unwrap();
    assert_eq!(chain.candidates.len(), 4);
    assert!(chain.candidates.iter().all(|c| c.phase == Phase::Landed && c.batch));
    assert_eq!(
        runs(&ran),
        [("fast".to_string(), tip.commit.clone()), ("slow".to_string(), tip.commit.clone())]
    );
    assert_eq!(ended(outcomes), (1..=4).map(|n| (n, Some(Decision::Landed))).collect::<Vec<_>>());
    git(&work, &["fetch", "--quiet", "origin"]);
    assert_eq!(git(&work, &["rev-parse", "origin/main"]), tip.commit, "one fast-forward");
    let told: Vec<u64> = forge.landed.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(told, [1, 2, 3, 4]);
    assert!(forge.sent_back.borrow().is_empty());
}

#[test]
fn a_red_batch_round_is_bisected_to_its_red_change_which_is_sent_back_while_the_rest_lands() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    let base = git(&work, &["rev-parse", "main"]);
    let asked = changes(&work, 4, &[2]);
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let lander = lander(&work, &worktrees, &forge);

    let outcomes = lander.land_with(&asked, Some(Landing::Batch)).unwrap();

    assert_eq!(
        ended(outcomes),
        [
            (1, Some(Decision::Landed)),
            (2, Some(Decision::Red)),
            (3, Some(Decision::Landed)),
            (4, Some(Decision::Landed)),
        ]
    );
    // The tip red, then the middle candidate red, then the one before it
    // green: three gates, ceil(log2 4)+1, find the red one; the rebuilt
    // round's tip is the fourth.
    let slow: Vec<String> =
        runs(&ran).into_iter().filter(|(gate, _)| gate == "slow").map(|(_, c)| c).collect();
    assert_eq!(slow.len(), 4, "{slow:?}");
    let sent = forge.sent_back.borrow();
    let [(2, Some(run))] = &sent[..] else { panic!("{sent:?}") };
    assert!(run.excerpt.iter().any(|line| line == "FAIL"), "sent back with its red run");
    assert_eq!(landed_files(&work), ["file-1.txt", "file-3.txt", "file-4.txt"]);
    // What landed is one fast-forward chain from the base, each candidate a
    // merge onto the one before it, the first green prefix landed first.
    let history = git(&work, &["rev-list", "--first-parent", &format!("{base}..origin/main")]);
    assert_eq!(history.lines().count(), 3);
    assert!(forge.landed.borrow().iter().map(|(n, _)| *n).eq([1, 3, 4]));
}

#[test]
fn a_batch_round_with_two_red_changes_sends_both_back_and_lands_the_rest() {
    let root = tempfile::tempdir().unwrap();
    let (work, _) = scratch(root.path());
    let asked = changes(&work, 5, &[2, 4]);
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let lander = lander(&work, &worktrees, &forge);

    let outcomes = lander.land_with(&asked, Some(Landing::Batch)).unwrap();

    let decided = ended(outcomes);
    assert_eq!(
        decided,
        [
            (1, Some(Decision::Landed)),
            (2, Some(Decision::Red)),
            (3, Some(Decision::Landed)),
            (4, Some(Decision::Red)),
            (5, Some(Decision::Landed)),
        ]
    );
    let sent: Vec<u64> = forge.sent_back.borrow().iter().map(|(n, _)| *n).collect();
    assert_eq!(sent, [2, 4]);
    assert_eq!(landed_files(&work), ["file-1.txt", "file-3.txt", "file-5.txt"]);
}

#[test]
fn a_repo_whose_config_says_batch_lands_its_rounds_as_batch_rounds() {
    let root = tempfile::tempdir().unwrap();
    let (work, ran) = scratch(root.path());
    std::fs::write(work.join("gate.md"), format!("{DECLARED}\nlanding: batch\n")).unwrap();
    git(&work, &["commit", "--quiet", "-am", "land in batch rounds"]);
    git(&work, &["push", "--quiet", "origin", "main"]);
    let asked = changes(&work, 3, &[]);
    let forge = Fake::default();
    let worktrees = root.path().join("candidates");
    let lander = lander(&work, &worktrees, &forge);

    lander.land(&asked).unwrap();

    let gates: Vec<String> = runs(&ran).into_iter().map(|(gate, _)| gate).collect();
    assert_eq!(gates, ["fast", "slow"], "the tip alone");
    assert_eq!(landed_files(&work), ["file-1.txt", "file-2.txt", "file-3.txt"]);
    // Asked per candidate for a round, the repo's batch landing gives way.
    let more = vec![
        change(&work, 4, "change-4", "file-4.txt"),
        change(&work, 5, "change-5", "file-5.txt"),
    ];
    std::fs::remove_file(&ran).unwrap();
    lander.land_with(&more, Some(Landing::PerCandidate)).unwrap();
    let gates: Vec<String> = runs(&ran).into_iter().map(|(gate, _)| gate).collect();
    assert_eq!(gates, ["fast", "fast", "slow"]);
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
    let landed = |id: &str| chain.candidates.iter().find(|c| c.id == id).unwrap().commit.clone();
    assert_eq!(outcomes, [(1, Outcome::Landed(landed(&a2))), (2, Outcome::Landed(landed(&b2)))]);
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
    let [first, second] = &chain.candidates[..] else { panic!("{chain:?}") };
    assert_eq!(first.branch, "refs/pull/7/head");
    assert_eq!(second.branch, "b", "a bare source is still a branch");
    assert_eq!(
        outcomes,
        [(7, Outcome::Landed(first.commit.clone())), (8, Outcome::Landed(second.commit.clone()))]
    );
    assert_eq!(runs(&ran).len(), 3, "both fast-gated, one slow gate at the tip");
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
