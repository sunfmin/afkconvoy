use crate::common;

use std::path::{Path, PathBuf};

use common::{World, commit, git};
use serde_json::{Value, json};

const LINE: &str = "refs/convoy/train/line/main";

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&args).ok()
}

/// What a tick did, without the workers it asked after and left or nudged.
fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap();
    steps
        .iter()
        .filter(|step| step["do"] != "board")
        .filter(|step| {
            !((step["do"] == "judge" || step["do"] == "watch") && step["outcome"] != "done")
        })
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

fn worktree_of(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().find(|row| row["linkedIssue"] == issue).expect("a worktree of the issue");
    PathBuf::from(row["path"].as_str().unwrap())
}

fn join(world: &World, issue: u64, now: &str) -> String {
    let dir = worktree_of(world, issue);
    let answer = world.afk_in(&dir, &["land", "--issue", &issue.to_string(), "--now", now]).ok();
    assert_eq!(answer["outcome"], "joined", "{answer}");
    answer["commit"].as_str().unwrap().to_string()
}

/// A world whose workers on `issues`, dispatched at 1000, each opened a PR
/// (#`issue`0) and joined the train at 1050; the train worker was started,
/// and told of the train's tip, at 1090.
fn train(issues: &[u64]) -> World {
    let world = World::new();
    world.set_gate("true");
    std::fs::write(world.clone.join(".git/info/exclude"), "target/\n").unwrap();
    for &n in issues {
        world.issue(n, &["ready-for-agent"]);
    }
    assert_eq!(tick(&world, 1000)["errors"], json!([]));
    for &n in issues {
        let dir = worktree_of(&world, n);
        commit(&dir, &format!("f{n}.txt"), "x\n");
        git(&dir, &["push", "-q", "origin", "HEAD"]);
        world.pr(n * 10, n, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
        join(&world, n, "1050");
    }
    assert_eq!(did(&tick(&world, 1090)), ["tend_train done", "heartbeat done"]);
    world
}

fn train_worktree(world: &World) -> PathBuf {
    world.root.join("wt/convoy-train")
}

/// The terminals open in a worktree.
fn terminals(world: &World, dir: &Path) -> Vec<Value> {
    let orca = world.orca();
    let rows = orca["terminals"].as_array().unwrap();
    let here = |t: &&Value| t["open"] == true && t["worktreePath"] == dir.to_str().unwrap();
    rows.iter().filter(here).cloned().collect()
}

/// The last line typed at the worker in a worktree.
fn last_told(world: &World, dir: &Path) -> String {
    let open = terminals(world, dir);
    let sent = open[0]["sent"].as_array().unwrap();
    sent.last().unwrap()["text"].as_str().unwrap().to_string()
}

fn status(world: &World, issue: u64) -> String {
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", "1100"]).ok();
    let mine = plan["mine"].as_array().unwrap();
    let row = mine.iter().find(|row| row["number"] == issue).expect("a claim of mine");
    row["status"].as_str().unwrap().to_string()
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

/// The join records a PR carries.
fn records(world: &World, pr: u64) -> Vec<String> {
    world.comments(pr).into_iter().filter(|body| body.contains("convoy:join")).collect()
}

fn line(world: &World) -> Option<String> {
    let listed = world.remote(&["for-each-ref", "--format=%(objectname)", LINE]);
    Some(listed).filter(|sha| !sha.is_empty())
}

fn pr_state(world: &World, number: u64) -> String {
    let gh = world.gh();
    let prs = gh["prs"].as_array().unwrap();
    prs.iter().find(|pr| pr["number"] == number).unwrap()["state"].as_str().unwrap().to_string()
}

fn abandon(world: &World, now: &str) -> common::Answer {
    world.afk(&["abandon", "--instance", "alpha", "--host", "h1", "--now", now])
}

/// Historical nudge evidence survives upgrades; an operator may explicitly
/// abandon such a train. New ticks must diagnose silence instead.
fn historical_nudge(world: &World, at: u64) {
    let dir = train_worktree(world);
    let git_dir = PathBuf::from(git(&dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::write(git_dir.join("convoy-nudged"), at.to_string()).unwrap();
}
fn abandon_now(world: &World, at: u64) -> Value {
    historical_nudge(world, at - 300);
    abandon(world, &at.to_string()).ok()
}

#[test]
fn an_explicit_abandon_of_a_historically_nudged_train_keeps_prs_and_builds() {
    let world = train(&[7, 8]);
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let dir = train_worktree(&world);
    // what the silent worker left in the train's worktree
    commit(&dir, "half-a-fix.txt", "x\n");
    std::fs::write(dir.join("README.md"), "edited\n").unwrap();
    std::fs::write(dir.join("junk.txt"), "untracked\n").unwrap();
    std::fs::create_dir(dir.join("target")).unwrap();
    std::fs::write(dir.join("target/built.o"), "what a gate run built\n").unwrap();
    let heads: Vec<String> =
        [7, 8].iter().map(|n| git(&worktree_of(&world, *n), &["rev-parse", "HEAD"])).collect();

    // told moments ago: left; then nudged, once
    assert_eq!(did(&tick(&world, 1180)), ["tend_train nothing", "heartbeat done"]);
    assert_eq!(did(&tick(&world, 1390)), ["tend_train nothing", "heartbeat done"]);
    assert!(!last_told(&world, &dir).starts_with("You stopped with the landing train not landed"));
    assert_eq!(did(&tick(&world, 1480))[0], "tend_train nothing");
    assert!(line(&world).is_some());
    world.orca_calls();

    // silent again: the train is given up, with nothing landed
    let abandoned = abandon_now(&world, 1690);
    assert_eq!(abandoned["action"], "abandoned");
    assert_eq!(line(&world), None, "the train's refs are deleted");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base);

    // no PR failed: each reads `joining` again, and says on its marker comment why
    for (n, head) in [7, 8].iter().zip(&heads) {
        assert_eq!(status(&world, *n), "joining");
        assert_eq!(pr_state(&world, n * 10), "open");
        let records = records(&world, n * 10);
        assert_eq!(records.len(), 1);
        assert!(records[0].starts_with("<!--convoy:join abandoned=1690-->\n"), "{records:?}");
        // …and its worker is told to join the next train, its branch as it was
        let worktree = worktree_of(&world, *n);
        assert!(last_told(&world, &worktree).contains("join the next one now"));
        assert!(brief(&worktree).contains(&format!("PR #{} is open for it", n * 10)));
        assert_eq!(&git(&worktree, &["rev-parse", "HEAD"]), head);
    }
    assert_eq!(world.claims().len(), 2);

    // the train's worktree is cleared, and kept with what its gate runs built
    assert!(dir.is_dir());
    assert_eq!(terminals(&world, &dir).len(), 0, "its silent worker is closed");
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), base);
    assert_eq!(git(&dir, &["status", "--porcelain"]), "");
    assert!(!dir.join("junk.txt").exists());
    assert!(dir.join("target/built.o").exists());
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(!calls.contains(&"worktree rm".to_string()), "{calls:?}");

    // the next train: a PR joins again, and a train worker is started in the same worktree
    join(&world, 7, "1700");
    assert_eq!(status(&world, 7), "joined");
    assert_eq!(did(&tick(&world, 1780))[0], "tend_train done");
    assert_eq!(terminals(&world, &dir).len(), 1);
}

#[test]
fn an_inspection_s_pr_alone_on_a_train_given_up_is_joined_again_by_the_fleet() {
    let world = World::new();
    world.set_gate("true");
    // An inspection's change of the repo on the train, and nothing of the fleet's own.
    git(&world.clone, &["fetch", "-q", "origin"]);
    let checkout = world.root.join("wt/afkconvoy-inspection");
    let (branch, at) = ("tester/afkconvoy-inspection", checkout.to_str().unwrap());
    git(&world.clone, &["worktree", "add", "-q", "-b", branch, at, "origin/main"]);
    let head = commit(&checkout, "gate.sh", "afkconvoy exclusive -- cargo test\n");
    git(&checkout, &["push", "-q", "origin", "HEAD"]);
    world.with_gh(|gh| {
        let pr = json!({"number": 90, "title": "Queue the gates", "headRefName": branch,
            "headRefOid": "", "body": "Run the gate through the host's queue.\n", "state": "open"});
        gh["prs"].as_array_mut().unwrap().push(pr);
    });
    let land = ["land", "--inspection", "--host", "h1", "--now", "1050"];
    let joined = world.afk_in(&checkout, &land).ok();
    assert_eq!(joined["outcome"], "joined", "{joined}");

    // The fleet tends the train for it, and gives it up once its worker stays silent.
    assert_eq!(did(&tick(&world, 1090)), ["tend_train done"]);
    assert_eq!(did(&tick(&world, 1390)), ["tend_train nothing"]);
    let abandoned = abandon_now(&world, 1690);
    assert_eq!(abandoned["action"], "abandoned");

    // Nobody is left to join it again: the fleet did, onto the base branch.
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let tip = line(&world).expect("on the next train");
    assert_ne!(json!(tip), joined["commit"]);
    assert_eq!(world.remote(&["log", "-1", "--format=%P", &tip]), format!("{base} {head}"));
    let message = world.remote(&["log", "-1", "--format=%B", &tip]);
    assert_eq!(message, "Queue the gates (#90)\n\nBy an inspection on h1");
    assert_eq!(pr_state(&world, 90), "open");
    assert_eq!(did(&tick(&world, 1780)), ["tend_train done"], "a new train worker");
}

#[test]
fn what_the_train_already_landed_is_settled_from_the_base_branch_not_abandoned() {
    let world = train(&[7, 8]);
    // the train worker landed #7's joining, and went silent on the rest
    let joins = world.remote(&["log", "--reverse", "--merges", "--format=%H", LINE]);
    let landed = joins.lines().next().unwrap().to_string();
    world.remote(&["update-ref", "refs/heads/main", &landed]);
    assert_eq!(status(&world, 7), "landed");

    assert_eq!(did(&tick(&world, 1390)), ["settle 7 done", "tend_train nothing", "heartbeat done"]);
    assert_eq!(abandon_now(&world, 1690)["action"], "abandoned");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), landed, "what landed stays");
    assert_eq!(world.state(7), "closed");
    assert_eq!(records(&world, 70), [] as [String; 0], "a landed PR was taken off nothing");
    assert_eq!(status(&world, 8), "joining");
    assert_eq!(records(&world, 80).len(), 1);
    assert_eq!(pr_state(&world, 80), "open");
}

#[test]
fn a_pr_taken_off_an_abandoned_train_a_second_time_is_escalated_with_its_branch_kept() {
    let world = train(&[7]);
    tick(&world, 1390);
    assert_eq!(abandon_now(&world, 1690)["action"], "abandoned");
    // the PR joins the next train, whose worker goes silent too
    join(&world, 7, "1700");
    assert_eq!(did(&tick(&world, 1780))[0], "tend_train done");
    assert_eq!(did(&tick(&world, 2080))[0], "tend_train nothing", "a new worker has its own nudge");
    let worktree = worktree_of(&world, 7);
    let head = git(&worktree, &["rev-parse", "HEAD"]);
    let branch = git(&worktree, &["symbolic-ref", "--short", "HEAD"]);

    assert_eq!(abandon_now(&world, 2380)["action"], "abandoned");
    assert_eq!(line(&world), None);
    assert_eq!(world.claims(), []);
    assert_eq!(world.labels(7), ["ready-for-human"]);
    let said = world.comments(7);
    let escalation = said.iter().find(|body| body.contains("convoy:escalation")).unwrap();
    assert!(escalation.contains("PR #70") && escalation.contains("for the second time"));
    let records = records(&world, 70);
    assert_eq!(records.len(), 1, "one marker comment per PR");
    assert!(records[0].starts_with("<!--convoy:join abandoned=1690-->\n"), "{records:?}");
    assert_eq!(pr_state(&world, 70), "open");
    assert_eq!(world.state(7), "open");
    // everything in the worktree is on the pushed branch: only the branch is kept
    assert!(!worktree.is_dir());
    assert_eq!(world.remote(&["rev-parse", &format!("refs/heads/{branch}")]), head);
}

#[test]
fn the_abandon_and_the_restart_share_the_prs_one_marker_comment() {
    let world = train(&[7]);
    world.comment(70, "<!--convoy:join restarted=1040-->\nrestarted before it joined");
    tick(&world, 1390);
    assert_eq!(abandon_now(&world, 1690)["action"], "abandoned");
    let records = records(&world, 70);
    assert_eq!(records.len(), 1);
    assert!(records[0].starts_with("<!--convoy:join restarted=1040 abandoned=1690-->\n"));
    assert_eq!(world.claims().len(), 1, "a restart is not an abandon: the PR joins again");
}

#[test]
fn a_train_that_grew_since_the_nudge_gets_its_worker_told_and_a_nudge_of_its_own() {
    let world = World::new();
    world.set_gate("true");
    for n in [7, 8] {
        world.issue(n, &["ready-for-agent"]);
    }
    tick(&world, 1000);
    // both open a PR; #8 is slow to join
    for n in [7, 8] {
        let dir = worktree_of(&world, n);
        commit(&dir, &format!("f{n}.txt"), "x\n");
        git(&dir, &["push", "-q", "origin", "HEAD"]);
        world.pr(n * 10, n, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    }
    join(&world, 7, "1050");
    assert_eq!(did(&tick(&world, 1090))[0], "tend_train done");
    assert_eq!(did(&tick(&world, 1390))[0], "tend_train nothing");
    join(&world, 8, "1400");
    // not a silence past a nudge: something joined that it was not told about
    assert_eq!(did(&tick(&world, 1690))[0], "tend_train done");
    assert!(line(&world).is_some());
    assert_eq!(did(&tick(&world, 1780))[0], "tend_train nothing");
    assert_eq!(did(&tick(&world, 1990))[0], "tend_train nothing");
    assert_eq!(abandon_now(&world, 2290)["action"], "abandoned");
}

#[test]
fn an_abandon_by_hand_is_refused_unless_the_tick_would_abandon() {
    let world = train(&[7]);
    let refused = abandon(&world, "1180").error();
    assert!(refused.contains("sign of life within the grace period, or was not nudged"));
    assert!(abandon(&world, "1390").error().contains("was not nudged yet"));
    historical_nudge(&world, 1390);
    assert!(abandon(&world, "1480").error().contains("sign of life"));
    assert!(line(&world).is_some());

    // at work, it is the tick's to tend
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "working", "stateStartedAt": 1_600_000});
            terminal["lastOutputAt"] = json!(1_680_000);
        }
    });
    assert!(abandon(&world, "1690").error().contains("its worker is at work"));
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = Value::Null;
            terminal["lastOutputAt"] = Value::Null;
        }
    });

    let abandoned = abandon(&world, "1690").ok();
    assert_eq!(abandoned["action"], "abandoned");
    assert_eq!((&abandoned["issues"], &abandoned["escalated"]), (&json!([7]), &json!([])));
    assert_eq!(line(&world), None);
    assert!(abandon(&world, "1700").error().contains("nothing is on the landing train"));
}

#[test]
fn the_loop_says_when_it_nudged_the_train_worker_and_abandoned_the_train() {
    let world = train(&[7]);
    world.say_concurrency(&world.clone, 2);
    let run = |now: &str| {
        let args = ["run", "--foreground", "--host", "h1", "--second-ms", "0"];
        let more = ["--instance", "alpha", "--ticks", "1", "--now", now];
        world.afk_lines(&[&args[..], &more[..]].concat()).0
    };
    let stands = "holding 1/2 · frontier 0 · next in 90s";
    assert_eq!(run("1390"), [format!("tick 1: nothing to do · {stands}")]);
    assert_eq!(run("1690"), [format!("tick 1: nothing to do · {stands}")]);
    assert!(line(&world).is_some(), "unclassified silence never abandons a train");
}
