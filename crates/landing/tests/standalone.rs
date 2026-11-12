//! The landing crate as another program uses it (docs/adr/0060): from
//! outside, through its public API alone, with a fake forge and no fleet
//! beside it. Two changes land on a local bare repo in one round — both
//! fast-green, one slow gate at the round's tip, the target fast-forwarded
//! through both candidates in order, and the forge told each landed. A red
//! round lands nothing and cancels the chain.

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::process::Command;

use afkconvoy_gate::declared;
use afkconvoy_gate::gate::GateCommand;
use afkconvoy_gate::layout::Layout;
use afkconvoy_gate::run::{self, Git, Settled};
use afkconvoy_landing::chain::{Chain, Decision, Phase, Plain, Process};
use afkconvoy_landing::forge::{Change, Forge};
use afkconvoy_landing::land::{Lander, Outcome};
use afkconvoy_landing::turn::{self, Declared};

/// A forge that lets every change land and remembers what it was told.
#[derive(Default)]
struct Fake {
    landed: RefCell<Vec<(u64, String)>>,
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
    let script = format!(
        "#!/bin/sh\necho \"$1 $(git rev-parse HEAD)\" >> {}\nif [ \"$1\" = slow ] && [ -e breaks ]; then echo FAIL; exit 1; fi\necho ok\n",
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
    Ok(Declared { whole: "./gate.sh".into(), seconds: 180, gates })
}

/// The kind of machine every gate here runs on.
const MACHINE: &str = "scratch-kind";

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn lander<'a>(work: &Path, worktrees: &Path, forge: &'a Fake) -> Lander<'a, Fake> {
    Lander {
        git: Git { worktree: work.into(), remote: "origin".into(), layout: Layout::Hidden },
        forge,
        host: "scratch-host".into(),
        machine: MACHINE.into(),
        owner: Process {
            host: "scratch-host".into(),
            pid: std::process::id(),
            started: "s".into(),
        },
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
