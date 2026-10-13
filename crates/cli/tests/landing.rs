mod common;

use std::path::{Path, PathBuf};

use common::{World, commit, git};
use serde_json::{Value, json};

const LINE: &str = "refs/convoy/train/line/main";
const RED: &str = "refs/convoy/train/red/main";

/// A world whose gate is red while the tree holds a file `broken`, and which
/// runs the script `during` beside it, once, when a test left one there.
fn world() -> World {
    let world = World::new();
    let root = world.root.display();
    world.set_gate(&format!(
        "echo gate-ran; if [ -f {root}/during ]; then mv {root}/during {root}/during.sh; \
         sh {root}/during.sh; fi; test ! -f broken"
    ));
    world
}

/// The train's own worktree.
fn train(world: &World) -> PathBuf {
    let dir = world.root.join("train");
    git(&world.clone, &["fetch", "-q", "origin"]);
    let at = dir.to_str().unwrap();
    git(&world.clone, &["worktree", "add", "-q", "-b", "convoy-train", at, "origin/main"]);
    dir
}

fn join(world: &World, dir: &Path, issue: u64) -> String {
    let answer = world.afk_in(dir, &["land", "--issue", &issue.to_string()]).ok();
    assert_eq!(answer["outcome"], "joined", "{answer}");
    answer["commit"].as_str().unwrap().to_string()
}

fn land_train(world: &World, dir: &Path) -> (Value, String) {
    let ran = world.afk_in(dir, &["land", "--train", "--merged-timeout", "5"]);
    let log = ran.stderr.clone();
    (ran.ok(), log)
}

fn base(world: &World) -> String {
    world.remote(&["rev-parse", "refs/heads/main"])
}

fn branches(world: &World) -> Vec<String> {
    let listed = world.remote(&["for-each-ref", "--format=%(refname:short)", "refs/heads"]);
    listed.lines().map(str::to_string).collect()
}

fn state(world: &World, issue: u64) -> String {
    let gh = world.gh();
    let issues = gh["issues"].as_array().unwrap();
    let row = issues.iter().find(|row| row["number"] == issue).unwrap();
    row["state"].as_str().unwrap().to_string()
}

#[test]
fn a_train_of_one_pr_lands_on_its_workers_recorded_run() {
    let world = world();
    let seven = world.worker(7, "seven.txt", "7\n");
    assert_eq!(world.afk_in(&seven, &["gate", "--train"]).ok()["recorded"], true);
    let commit = join(&world, &seven, 7);
    let train = train(&world);

    let (answer, log) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert!(!log.contains("gate-ran"), "no second run of the gate: {log}");
    assert_eq!(answer["gate"]["source"], "recorded");
    assert_eq!(answer["commit"], commit.as_str());
    assert_eq!(answer["prs"], json!([70]));
    assert_eq!(answer["landed"], json!([{"issue": 7, "pr": 70, "commit": commit}]));
    assert_eq!(answer["unmerged"], json!([]));
    assert_eq!(answer["more"], false);

    assert_eq!(base(&world), commit, "the commit gated is the commit on the base branch");
    assert_eq!(state(&world, 7), "closed");
    assert_eq!(branches(&world), ["main"], "the landed PR's branch is deleted");

    let (again, _) = land_train(&world, &train);
    assert_eq!(again["outcome"], "idle");
    assert_eq!(base(&world), commit);
}

/// The gate's records on the remote → (ref, commit, subject), one a line.
fn gate_records(world: &World) -> String {
    world.remote(&[
        "for-each-ref",
        "--format=%(refname) %(objectname) %(subject)",
        "refs/convoy/gate",
    ])
}

#[test]
fn a_landing_answered_by_a_run_on_record_says_that_runs_time_and_writes_no_record() {
    let world = world();
    let seven = world.worker(7, "seven.txt", "7\n");
    let gate = world.afk_in(&seven, &["gate", "--train", "--host", "workers-host"]).ok();
    assert_eq!(gate["recorded"], true, "{gate}");
    join(&world, &seven, 7);
    let train = train(&world);
    let before = gate_records(&world);
    assert!(
        before.ends_with(&format!(" seconds={} host=workers-host", gate["seconds"])),
        "{before}"
    );

    let args = ["land", "--train", "--merged-timeout", "5", "--host", "trains-host"];
    let answer = world.afk_in(&train, &args).ok();
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert_eq!(answer["gate"]["source"], "recorded");
    assert_eq!(answer["gate"]["seconds"], gate["seconds"]);
    assert_eq!(answer["gate"]["host"], "workers-host", "the run's host, not the landing's");
    assert_eq!(gate_records(&world), before, "no new record is written");
}

#[test]
fn a_landing_gated_by_a_run_made_now_puts_its_time_on_record() {
    let world = world();
    let seven = world.worker(7, "seven.txt", "7\n");
    join(&world, &seven, 7);
    let train = train(&world);

    let args = ["land", "--train", "--merged-timeout", "5", "--host", "trains-host"];
    let answer = world.afk_in(&train, &args).ok();
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert_eq!(answer["gate"]["source"], "run");
    assert_eq!(answer["gate"]["host"], "trains-host");
    let seconds = answer["gate"]["seconds"].as_u64().expect("the answer says the seconds");
    let records = gate_records(&world);
    assert!(records.ends_with(&format!(" seconds={seconds} host=trains-host")), "{records}");
}

#[test]
fn a_record_with_no_time_answers_a_landing_and_is_not_rewritten() {
    let world = world();
    let seven = world.worker(7, "seven.txt", "7\n");
    assert_eq!(world.afk_in(&seven, &["gate", "--train", "--host", "mini"]).ok()["recorded"], true);
    join(&world, &seven, 7);
    let train = train(&world);
    // the record as it was written before records kept a time
    let written = gate_records(&world);
    let (name, rest) = written.split_once(' ').unwrap();
    let subject = rest.split_once(' ').unwrap().1.split_once(" seconds=").unwrap().0;
    let tree = world.remote(&["rev-parse", &format!("{name}^{{tree}}")]);
    let old = git(&world.clone, &["commit-tree", &tree, "-m", subject]);
    git(&world.clone, &["push", "-q", "--force", "origin", &format!("{old}:{name}")]);
    let before = gate_records(&world);
    assert_eq!(before, format!("{name} {old} {subject}"));

    let (answer, log) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert!(!log.contains("gate-ran"), "it is the green run it was: {log}");
    let gate = answer["gate"].as_object().unwrap();
    assert_eq!(gate["source"], "recorded");
    assert!(!gate.contains_key("seconds") && !gate.contains_key("host"), "{answer}");
    assert_eq!(gate_records(&world), before, "it is not rewritten");
}

#[test]
fn a_train_of_several_prs_is_gated_once_and_landed_whole() {
    let world = world();
    let seven = world.worker(7, "seven.txt", "7\n");
    let eight = world.worker(8, "eight.txt", "8\n");
    join(&world, &seven, 7);
    let tip = join(&world, &eight, 8);
    let train = train(&world);

    let (answer, log) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert_eq!(log.matches("gate-ran").count(), 1);
    assert_eq!(answer["gate"]["source"], "run");
    assert_eq!(answer["prs"], json!([70, 80]));
    assert_eq!(base(&world), tip);
    assert_eq!((state(&world, 7), state(&world, 8)), ("closed".into(), "closed".into()));
    assert_eq!(branches(&world), ["main"]);
    // the run is on record for the tree that landed
    let tree = world.remote(&["rev-parse", &format!("{tip}^{{tree}}")]);
    let records = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/gate"]);
    assert!(records.contains(&tree), "{records}");
}

#[test]
fn a_red_gate_lands_nothing_and_the_train_is_fixed_with_one_more_commit() {
    let world = world();
    let eight = world.worker(8, "eight.txt", "8\n");
    assert_eq!(world.afk_in(&eight, &["gate", "--train"]).ok()["recorded"], true);
    let green = join(&world, &eight, 8);
    let seven = world.worker(7, "broken", "it is\n");
    let red = join(&world, &seven, 7);
    let train = train(&world);
    let started = base(&world);

    let (answer, _) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "gate_red", "{answer}");
    assert_eq!(answer["gate"]["exit_code"], 1);
    assert_eq!(answer["gate"]["excerpt"], json!(["gate-ran"]));
    assert_eq!(base(&world), started, "nothing landed");
    assert_eq!(world.remote(&["rev-parse", RED]), red);
    assert_eq!((state(&world, 7), state(&world, 8)), ("open".into(), "open".into()));
    for pr in [70, 80] {
        let comments = world.comments(pr);
        assert_eq!(comments.len(), 1, "every PR of the run is told");
        assert!(comments[0].contains("exited 1") && comments[0].contains("gate-ran"));
    }

    // a worker gating meanwhile takes in the train's last green commit, not the red tip
    let nine = world.worker(9, "nine.txt", "9\n");
    let gate = world.afk_in(&nine, &["gate", "--train"]).ok();
    assert_eq!(gate["merged"], green.as_str());
    assert_eq!(gate["status"], "green");
    assert!(!nine.join("broken").exists());

    // the train worker fixes the train: one more commit on top, here
    git(&train, &["rm", "-q", "broken"]);
    git(&train, &["commit", "-q", "-m", "fix the train"]);
    let fix = git(&train, &["rev-parse", "HEAD"]);
    let (answer, _) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert_eq!(answer["commit"], fix.as_str());
    assert_eq!(answer["prs"], json!([70, 80]));
    assert_eq!(base(&world), fix);
    assert_eq!(world.remote(&["rev-parse", &format!("{fix}^")]), red, "nothing was rewritten");
    assert_eq!((state(&world, 7), state(&world, 8)), ("closed".into(), "closed".into()));

    // the red commit is on the base branch now: the train is not under repair
    let ten = world.worker(10, "ten.txt", "10\n");
    assert_eq!(world.afk_in(&ten, &["gate", "--train"]).ok()["merged"], fix.as_str());
}

#[test]
fn a_train_whose_gate_reaches_the_configs_gate_limit_is_red_and_says_the_limit() {
    let world = World::new();
    world.set_gate_config("gate_command: sleep 30\ngate_seconds: 1");
    let seven = world.worker(7, "seven.txt", "7\n");
    join(&world, &seven, 7);
    let started = std::time::Instant::now();
    let (answer, _) = land_train(&world, &train(&world));
    assert!(started.elapsed().as_secs() < 15, "the run was killed");
    assert_eq!(answer["outcome"], "gate_red", "{answer}");
    assert_eq!(
        (&answer["gate"]["timed_out"], &answer["gate"]["exit_code"]),
        (&json!(true), &json!(124))
    );
    let detail = answer["detail"].as_str().unwrap();
    assert!(detail.contains("reached the gate limit of 1s"), "{detail}");
}

#[test]
fn under_a_red_train_with_no_green_commit_a_worker_gates_on_the_base_branch() {
    let world = world();
    let seven = world.worker(7, "broken", "it is\n");
    join(&world, &seven, 7);
    let train = train(&world);
    assert_eq!(land_train(&world, &train).0["outcome"], "gate_red");
    let nine = world.worker(9, "nine.txt", "9\n");
    let gate = world.afk_in(&nine, &["gate", "--train"]).ok();
    assert_eq!(gate["merged"], base(&world).as_str());
    assert_eq!(gate["status"], "green");
}

#[test]
fn a_base_branch_that_moved_during_the_gate_refuses_the_push() {
    let world = world();
    let seven = world.worker(7, "seven.txt", "7\n");
    let commit_7 = join(&world, &seven, 7);
    let train = train(&world);
    // someone lands on the base branch while the gate runs
    let other = world.root.join("other");
    git(&world.root, &["clone", "-q", world.bare.to_str().unwrap(), other.to_str().unwrap()]);
    let moved = commit(&other, "else.txt", "else\n");
    git(&other, &["push", "-q", "origin", "HEAD:refs/heads/side"]);
    let bare = world.bare.display();
    std::fs::write(
        world.root.join("during"),
        format!("git --git-dir={bare} update-ref refs/heads/main {moved}\n"),
    )
    .unwrap();

    let (answer, _) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "target_moved", "{answer}");
    assert_eq!(base(&world), moved, "nothing landed");
    assert_eq!(state(&world, 7), "open");

    // the next run merges the new tip in and gates that
    let (answer, log) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert!(log.contains("gate-ran"), "the new tip is another tree: it is gated");
    let landed = base(&world);
    assert_eq!(answer["commit"], landed.as_str());
    let parents = world.remote(&["log", "-1", "--format=%P", &landed]);
    assert_eq!(parents, format!("{commit_7} {moved}"), "a merge, never a rebase");
    assert_eq!(world.remote(&["rev-parse", LINE]), landed);
    assert_eq!(state(&world, 7), "closed");
}

#[test]
fn prs_that_joined_during_a_gate_run_land_behind_the_next_one() {
    let world = world();
    let seven = world.worker(7, "seven.txt", "7\n");
    let eight = world.worker(8, "eight.txt", "8\n");
    let first = join(&world, &seven, 7);
    let train = train(&world);
    std::fs::write(
        world.root.join("during"),
        format!("cd {} && {} land --issue 8\n", eight.display(), env!("CARGO_BIN_EXE_afkconvoy")),
    )
    .unwrap();

    let (answer, _) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert_eq!(answer["prs"], json!([70]));
    assert_eq!(answer["more"], true, "the train has grown since");
    assert_eq!(base(&world), first);
    assert_eq!((state(&world, 7), state(&world, 8)), ("closed".into(), "open".into()));

    let (answer, _) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert_eq!(answer["prs"], json!([80]));
    assert_eq!(answer["more"], false);
    assert_eq!(state(&world, 8), "closed");
}

#[test]
fn a_pr_github_still_shows_open_keeps_its_branch_and_is_reported() {
    let world = world();
    let seven = world.worker(7, "seven.txt", "7\n");
    join(&world, &seven, 7);
    world.with_gh(|gh| gh["slow_merge"] = json!([70]));
    let train = train(&world);
    let answer = world.afk_in(&train, &["land", "--train", "--merged-timeout", "0"]).ok();
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert_eq!(answer["unmerged"], json!([70]));
    assert_eq!(branches(&world), ["issue-7", "main"]);
    assert_eq!(state(&world, 7), "closed");
}

#[test]
fn a_pr_pushed_to_since_it_joined_is_not_finished_by_the_landing() {
    let world = world();
    let seven = world.worker(7, "seven.txt", "7\n");
    let commit_7 = join(&world, &seven, 7);
    commit(&seven, "more.txt", "more\n");
    git(&seven, &["push", "-q", "origin", "HEAD"]);
    let train = train(&world);

    let (answer, _) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert_eq!(base(&world), commit_7);
    assert_eq!(answer["landed"], json!([]), "its worker joins again with what it pushed");
    assert_eq!(state(&world, 7), "open");
    assert_eq!(branches(&world), ["issue-7", "main"]);
}

#[test]
fn a_base_branch_that_conflicts_with_the_train_is_resolved_in_the_trains_worktree() {
    let world = world();
    let seven = world.worker(7, "README.md", "seven\n");
    join(&world, &seven, 7);
    let moved = world.land_elsewhere("README.md");
    let train = train(&world);

    let (answer, log) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "conflict", "{answer}");
    assert_eq!(answer["files"], json!(["README.md"]));
    assert!(!log.contains("gate-ran"));
    assert_eq!(base(&world), moved);
    assert_eq!(land_train(&world, &train).0["outcome"], "conflict", "as it stands");

    std::fs::write(train.join("README.md"), "both\n").unwrap();
    git(&train, &["add", "."]);
    git(&train, &["commit", "-q", "--no-edit"]);
    let (answer, _) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "landed", "{answer}");
    assert_eq!(world.remote(&["show", "main:README.md"]), "both");
    assert_eq!(state(&world, 7), "closed");
}

#[test]
fn an_empty_train_is_idle_and_a_dirty_worktree_is_refused() {
    let world = world();
    let train = train(&world);
    let (answer, log) = land_train(&world, &train);
    assert_eq!(answer["outcome"], "idle");
    assert!(!log.contains("gate-ran"));

    let seven = world.worker(7, "seven.txt", "7\n");
    join(&world, &seven, 7);
    std::fs::write(train.join("README.md"), "edited\n").unwrap();
    let error = world.afk_in(&train, &["land", "--train"]).error();
    assert!(error.contains("uncommitted changes"), "{error}");
    assert_eq!(state(&world, 7), "open");

    // land takes exactly one of --issue and --train
    assert_eq!(world.afk_in(&train, &["land"]).code, 2);
    assert_eq!(world.afk_in(&train, &["land", "--train", "--issue", "7"]).code, 2);
}
