use crate::common;

use afkconvoy::log::Entry;
use afkconvoy_core::{cut::CHECKOUT, worker::is_worktree_named};
use common::{BASE, CONFIG, World, git};
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};

/// The `publish` line that says, in the world, what it published with which
/// tag checked out.
fn says_published(world: &World) -> String {
    let published = world.root.join("published");
    format!(
        "printf '%s %s\\n' \"$VERSION\" \"$(git describe --tags --exact-match)\" >> '{}'",
        published.display()
    )
}

/// The cut steps of a repo that turns cutting on: `set` writes the version to
/// a file, and `publish` runs the lines `publish`.
fn cut_block(publish: &str) -> String {
    let publish: String = publish.lines().map(|line| format!("  {line}\n")).collect();
    format!(
        "```yaml cut\nset: |\n  printf '%s\\n' \"$VERSION\" > VERSION\npublish: |\n{publish}```\n"
    )
}

/// Commits `config` as the repo's config file, and pushes it to the base branch.
fn commit_config(world: &World, config: &str, subject: &str) {
    std::fs::write(world.clone.join("docs/agents/afkconvoy.md"), config).unwrap();
    git(&world.clone, &["commit", "-q", "-am", subject]);
    git(&world.clone, &["push", "-q", "origin", BASE]);
}

/// Lands a commit `subject` on the base branch, as the landing train would.
fn land(world: &World, subject: &str) {
    std::fs::write(world.clone.join("widget.txt"), subject).unwrap();
    git(&world.clone, &["add", "widget.txt"]);
    git(&world.clone, &["commit", "-q", "-m", subject]);
    git(&world.clone, &["push", "-q", "origin", BASE]);
}

/// A landing at a known time whose side-branch work was authored earlier.
fn land_at(world: &World, subject: &str, when: u64) {
    std::fs::write(world.clone.join("widget.txt"), subject).unwrap();
    git(&world.clone, &["add", "widget.txt"]);
    let out = world
        .command("git", &world.clone)
        .env("GIT_COMMITTER_DATE", format!("@{when} +0000"))
        .env("GIT_AUTHOR_DATE", "@100 +0000")
        .args(["commit", "-q", "-m", subject])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    git(&world.clone, &["push", "-q", "origin", BASE]);
}

/// Tags the base branch's tip `name`, and pushes the tag.
fn tag(world: &World, name: &str) {
    git(&world.clone, &["tag", "-a", name, "-m", name]);
    git(&world.clone, &["push", "-q", "origin", name]);
}

fn turn_cutting_on(world: &World, subject: &str) {
    turn_cutting_on_publishing(world, &says_published(world), subject);
}

fn turn_cutting_on_publishing(world: &World, publish: &str, subject: &str) {
    commit_config(world, &format!("{CONFIG}\n{}", cut_block(publish)), subject);
}

/// A repo that turns cutting on, tagged `v0.1.0` a commit before its base
/// branch's tip: a PR, #7, landed since.
fn cutting() -> World {
    cutting_publishing(says_published)
}

/// `cutting`, its `publish` lines what `publish` says in its world.
fn cutting_publishing(publish: impl Fn(&World) -> String) -> World {
    let world = World::new();
    tag(&world, "v0.1.0");
    turn_cutting_on_publishing(&world, &publish(&world), "Turn cutting on (#7)");
    world
}

fn planned(world: &World, instance: &str, now: u64) -> Value {
    world.afk(&["plan", "--instance", instance, "--now", &now.to_string()]).ok()
}

fn standing(world: &World, instance: &str, now: u64) -> String {
    let plan = planned(world, instance, now);
    plan["cut"]["standing"].as_str().unwrap_or_else(|| panic!("no cut: {plan}")).to_string()
}

fn tick_as(world: &World, instance: &str, now: u64) -> Value {
    let now = now.to_string();
    world.afk(&["tick", "--instance", instance, "--host", "h1", "--now", &now]).ok()
}

/// What a tick did about the cut, and the error of a step that failed.
fn cut(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    let steps = steps.filter(|step| ["cut", "end_cut"].contains(&step["do"].as_str().unwrap()));
    let errors = ticked["errors"].as_array().unwrap();
    let error = |step: &Value| errors.iter().find(|error| error["do"] == step["do"]).cloned();
    let said = |step: &Value| match error(step) {
        Some(error) => format!("{} failed: {}", step["do"], error["error"]),
        None => format!("{} {}", step["do"], step["outcome"]),
    };
    steps.map(|step| said(step).replace('"', "")).collect()
}

/// Every line of this world's tick log, as `run` would print it.
fn logged(world: &World) -> Vec<String> {
    let runs = world.cache().join("afkconvoy/runs");
    let files = std::fs::read_dir(&runs).unwrap().map(|entry| entry.unwrap().path());
    let files = files.filter(|file| file.extension().unwrap() == "jsonl");
    let text: String = files.map(|file| std::fs::read_to_string(file).unwrap()).collect();
    let entry = |line: &str| serde_json::from_str::<Entry>(line).unwrap().said();
    text.lines().map(entry).collect()
}

/// The cut makes its version's commit in `checkout`, on the snapshot, and
/// its record says so → that commit.
fn its_commit_is_made(checkout: &Path) -> String {
    common::commit(checkout, "VERSION", "0.1.1\n");
    let made = git(checkout, &["rev-parse", "HEAD"]).trim().to_string();
    let mut record = record(checkout);
    record["commit"] = json!(made);
    std::fs::write(record_file(checkout), record.to_string()).unwrap();
    made
}

/// Every branch a cut pushed on the remote.
fn cut_branches(world: &World) -> Vec<String> {
    let listed =
        world.remote(&["for-each-ref", "--format=%(refname:short)", "refs/heads/afkconvoy-cut"]);
    listed.lines().map(str::to_string).collect()
}

/// The checkout the cut was started in.
fn checkout(world: &World) -> PathBuf {
    let listed = world.worktrees().into_iter();
    let mut cuts: Vec<_> = listed
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| is_worktree_named(name, CHECKOUT))
        })
        .collect();
    cuts.pop().expect("the cut has a checkout")
}

/// Where the cut keeps its record: in its checkout's own git dir.
fn record_file(checkout: &Path) -> PathBuf {
    let dir = git(checkout, &["rev-parse", "--absolute-git-dir"]);
    Path::new(dir.trim()).join("convoy-cut.json")
}

fn record(checkout: &Path) -> Value {
    serde_json::from_str(&std::fs::read_to_string(record_file(checkout)).unwrap()).unwrap()
}

/// The fleet's claim on a cut, on the remote.
fn claimed(world: &World) -> bool {
    let listed = git(&world.bare, &["for-each-ref", "--format=%(refname)"]);
    listed.lines().any(|name| name.ends_with("/cut"))
}

/// Every open issue that says a cut failed: its number and labels.
fn failures(world: &World) -> Vec<(u64, Vec<String>)> {
    let issues = world.gh()["issues"].as_array().unwrap().clone();
    let failed = issues.iter().filter(|issue| {
        issue["state"].as_str().unwrap_or("open") == "open"
            && issue["title"].as_str().unwrap().starts_with("afkconvoy: the cut of ")
    });
    let labels = |issue: &Value| serde_json::from_value(issue["labels"].clone()).unwrap();
    failed.map(|issue| (issue["number"].as_u64().unwrap(), labels(issue))).collect()
}

fn workers_leave(world: &World) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });
}

#[test]
fn plan_says_where_a_cut_stands_and_nothing_of_one_where_cutting_is_off() {
    let world = World::new();
    tag(&world, "v0.1.0");
    land(&world, "Add a widget (#5)");
    let plan = planned(&world, "alpha", 10_000);
    assert!(plan.get("cut").is_none(), "{plan}");

    turn_cutting_on(&world, "Turn cutting on (#7)");
    let plan = planned(&world, "alpha", 10_000);
    assert_eq!(plan["cut"]["standing"], "due", "{plan}");
    assert_eq!(plan["cut"]["past"], 2, "{plan}");

    tag(&world, "v0.1.1");
    assert_eq!(standing(&world, "alpha", 10_000), "tagged");
}

#[test]
fn with_no_version_tag_there_is_nothing_to_cut_from() {
    let world = World::new();
    turn_cutting_on(&world, "Turn cutting on");
    assert_eq!(standing(&world, "alpha", 10_000), "untagged");
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), Vec::<String>::new());
}

#[test]
fn a_cut_keeps_its_claim_checkout_and_terminal_when_the_host_runner_moves() {
    use crate::host_runner::{as_built, installed, logged, upgraded};
    let world = cutting();
    let link = installed(&world);
    let first =
        world.afk_as(&link, &world.clone, &["run", "--host", "h1", "--second-ms", "100"]).ok();
    let checkout = checkout(&world);
    let before = record(&checkout);
    assert!(claimed(&world));
    let terminals = world.orca()["terminals"].clone();
    upgraded(&link, "999.0.0", &as_built());
    logged(&world, "host runner moved: ");
    let until = Instant::now() + Duration::from_secs(30);
    while !world
        .runner_log()
        .iter()
        .any(|line| line.contains("start: took") && line.contains("cut"))
    {
        assert!(Instant::now() < until, "{:#?}", world.runner_log());
        std::thread::sleep(Duration::from_millis(100));
    }
    assert_ne!(world.host_runner().map(u64::from), first["host_runner"]["pid"].as_u64());
    assert!(claimed(&world));
    assert_eq!(record(&checkout), before);
    assert_eq!(world.orca()["terminals"], terminals);
    let text = std::fs::read_to_string(first["tick_log"].as_str().unwrap()).unwrap();
    assert!(
        text.lines().any(|line| line.contains("\"moved\"") && line.contains("cut in ")),
        "{text}"
    );
}

/// An untagged repo declaring its first version. Its gate can be failed by
/// a file outside the checkout, so retry uses exactly the same committed tree.
fn initial(preset: bool) -> World {
    let world = World::new();
    if preset {
        common::commit(&world.clone, "VERSION", "1.2.3\n");
    }
    let gate = format!("test ! -f '{}'", world.root.join("red").display());
    let steps = cut_block(&says_published(&world))
        .replace("```yaml cut\n", "```yaml cut\ninitial: 1.2.3\n");
    commit_config(
        &world,
        &format!("{}\n{steps}", CONFIG.replace("./gate.sh", &gate)),
        "Turn cutting on (#7)",
    );
    // Labels on old landings do not override the explicitly declared first version.
    world.with_gh(|gh| {
        gh["prs"].as_array_mut().unwrap().push(json!({
            "number": 7, "title": "Turn cutting on", "headRefName": "seven",
            "headRefOid": "", "body": "", "state": "merged", "labels": ["release:minor"]
        }))
    });
    world
}

/// The real cut command takes every step: it gates its own commit, takes no
/// landing turn, and the base branch gains nothing from it.
fn finish(world: &World, checkout: &Path) -> Value {
    let base = world.remote(&["rev-parse", "refs/heads/main"]);
    let answer = world.afk_in(checkout, &["cut", "--host", "h1"]).ok();
    let commit = answer["commit"].as_str().unwrap_or_else(|| panic!("{answer}"));
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), base, "the base gained nothing");
    assert!(!ancestor(world, commit, &base), "the version's commit stays off the base");
    assert_eq!(
        world.remote(&[
            "rev-parse",
            &format!("v{}^{{commit}}", answer["version"].as_str().unwrap())
        ]),
        commit
    );
    let branch = answer["branch"].as_str().unwrap();
    assert!(branch.starts_with("afkconvoy-cut/v"), "{answer}");
    assert_eq!(world.remote(&["rev-parse", &format!("refs/heads/{branch}")]), commit);
    let tree = world.remote(&["rev-parse", &format!("{commit}^{{tree}}")]);
    assert!(
        !world
            .remote(&["for-each-ref", "--format=%(refname)", &format!("refs/convoy/gate/{tree}-*")])
            .is_empty(),
        "its gate run is on record"
    );
    no_cut_on_the_chain(world);
    assert!(
        world.gh()["prs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|pr| !pr["headRefName"].as_str().unwrap_or("").starts_with("afkconvoy-cut/")),
        "a cut opens no PR"
    );
    answer
}

/// Nothing on the remote's candidate chain names a cut: no candidate, no
/// barrier, no pending entry.
fn no_cut_on_the_chain(world: &World) {
    let chains = world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/candidates"]);
    for chain in chains.lines() {
        let record = world.remote(&["log", "-1", "--format=%B", chain]);
        // A cut on the chain would be a branch of its own. The record also
        // holds paths, which name a checkout called afkconvoy-cut whenever
        // the tests run in a cut's gate: those name no cut.
        assert!(!record.contains("\"branch\":\"afkconvoy-cut/"), "{record}");
    }
    let refs = world.remote(&["for-each-ref", "--format=%(refname)"]);
    assert!(
        !refs.lines().any(|name| name.contains("barrier") || name.contains("pending")),
        "{refs}"
    );
}

#[test]
fn a_watch_s_landed_fix_reaches_the_published_version_through_a_cut() {
    let world = initial(false);
    let checkout = world.root.join("afkconvoy-watch");
    git(
        &world.clone,
        &["worktree", "add", "-q", "-b", "watch-fix", checkout.to_str().unwrap(), "HEAD"],
    );
    common::commit(&checkout, "fixed.txt", "worktrees are trusted\n");
    git(&checkout, &["push", "-q", "origin", "HEAD"]);
    world.with_gh(|gh| {
        gh["prs"].as_array_mut().unwrap().push(json!({
            "number": 90, "title": "Trust every worktree", "headRefName": "watch-fix",
            "headRefOid": "", "body": "The watch's lasting fix.", "state": "open"
        }))
    });
    let joined = world.afk_in(&checkout, &["land", "--watch", "--host", "h1"]).ok();
    assert_eq!(joined["recorded"], true);
    assert_eq!(joined["outcome"], "merged");
    let watch_commit = joined["commit"].as_str().unwrap();
    assert_eq!(
        world.remote(&["rev-parse", &format!("{watch_commit}^@")]).lines().count(),
        2,
        "a watch still lands a merge commit"
    );
    assert!(!world.root.join("published").exists(), "joining has installed nothing");

    world.afk(&["wake"]).ok();
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let cut_checkout = self::checkout(&world);
    let released = finish(&world, &cut_checkout);
    assert_eq!(released["version"], "1.2.3");
    assert!(released["prs"].as_array().unwrap().contains(&json!(90)));
    let published = std::fs::read_to_string(world.root.join("published")).unwrap();
    assert_eq!(published, "1.2.3 v1.2.3\n");
    assert_eq!(world.remote(&["show", "v1.2.3:fixed.txt"]), "worktrees are trusted");
    assert!(failures(&world).is_empty(), "the cut asked for no human");
}

/// The parents of `commit` on the remote, oldest first as git lists them.
fn parents(world: &World, commit: &str) -> Vec<String> {
    world.remote(&["rev-parse", &format!("{commit}^@")]).lines().map(str::to_string).collect()
}

/// Whether `commit` is an ancestor of `tip` on the remote.
fn ancestor(world: &World, commit: &str, tip: &str) -> bool {
    world
        .command("git", &world.bare)
        .args(["merge-base", "--is-ancestor", commit, tip])
        .output()
        .unwrap()
        .status
        .success()
}

#[test]
fn a_cut_tags_a_commit_whose_one_parent_is_its_snapshot_and_leaves_the_base_alone() {
    let world = initial(false);
    let snapshot = world.remote(&["rev-parse", BASE]);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    std::fs::write(world.root.join("red"), "red").unwrap();
    let failed = world.afk_in(&checkout, &["cut", "--host", "h1"]).ok();
    assert_eq!(failed["failed"], "gate", "{failed}");
    let made = record(&checkout)["commit"].as_str().unwrap().to_string();
    std::fs::remove_file(world.root.join("red")).unwrap();
    land(&world, "Add a gadget (#9)");
    let tip = world.remote(&["rev-parse", BASE]);

    let answer = finish(&world, &checkout);
    let tagged = answer["commit"].as_str().unwrap();
    assert_eq!(tagged, made, "the commit it made is the one it gated and tagged");
    assert_eq!(parents(&world, tagged), vec![snapshot.clone()], "one parent, the snapshot");
    assert_eq!(
        world.remote(&["diff", "--name-only", &snapshot, tagged]),
        "VERSION",
        "its diff against the snapshot is what the set lines wrote"
    );
    assert_eq!(world.remote(&["show", &format!("{tagged}:VERSION")]), "1.2.3");
    assert_eq!(world.remote(&["rev-parse", BASE]), tip, "the base gained no commit");
    assert!(ancestor(&world, &snapshot, &tip), "the tag's parent is on the base");
    assert_eq!(
        world.remote(&["log", "-1", "--format=%s", tagged]),
        "remote.git 1.2.3",
        "titled `<repo> <version>`"
    );

    // The landing after the snapshot is past the tag; the next cut picks its
    // version from the tag.
    assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut done"]);
    let plan = planned(&world, "alpha", 10_200);
    assert_eq!(plan["cut"]["newest_tag"], "v1.2.3", "{plan}");
    assert_eq!(plan["cut"]["past"], 1, "{plan}");
    assert_eq!(cut(&tick_as(&world, "alpha", 10_200)), ["cut done"]);
    let next = record(&self::checkout(&world));
    assert_eq!(
        (&next["version"], &next["from"], &next["prs"]),
        (&json!("1.2.4"), &json!("v1.2.3"), &json!([9]))
    );
}

#[test]
fn a_cut_interrupted_after_tagging_goes_on_from_the_tag_and_pushes_no_second_branch() {
    let world = initial(false);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    let answer = finish(&world, &checkout);
    let tagged = answer["commit"].as_str().unwrap().to_string();
    let branches = cut_branches(&world);
    assert_eq!(branches.len(), 1, "{branches:?}");
    // Its record and its checkout's commit lost: it starts again from `set`.
    let snapshot = parents(&world, &tagged).pop().unwrap();
    git(&checkout, &["reset", "-q", "--hard", &snapshot]);
    let mut record = record(&checkout);
    for key in ["commit", "branch", "at"] {
        record[key] = Value::Null;
    }
    record["took"] = json!([]);
    std::fs::write(record_file(&checkout), record.to_string()).unwrap();

    let again = world.afk_in(&checkout, &["cut", "--host", "h1"]).ok();
    assert_eq!(again["commit"], tagged, "{again}");
    assert_eq!(world.remote(&["rev-parse", "v1.2.3^{commit}"]), tagged);
    assert_eq!(cut_branches(&world), branches, "no second branch");
}

/// A cut whose checkout is gone from its host while it holds the claim —
/// how the FelixMBP14 cut of v0.5.64 started again four times — opens the
/// one failure issue, and no cut of that version starts on its own. Once a
/// human closes it, the next cut takes up the branch the first pushed.
#[test]
fn a_cut_whose_checkout_is_gone_opens_its_issue_and_the_next_takes_up_its_branch() {
    let world = initial(false);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let first = checkout(&world);
    std::fs::write(world.root.join("red"), "red").unwrap();
    let failed = world.afk_in(&first, &["cut", "--host", "h1"]).ok();
    assert_eq!(failed["failed"], "gate", "{failed}");
    std::fs::remove_file(world.root.join("red")).unwrap();
    let pushed = cut_branches(&world);
    assert_eq!(pushed.len(), 1, "{pushed:?}");
    let made = world.remote(&["rev-parse", &format!("refs/heads/{}", pushed[0])]);
    // Taken from outside before the tick read it.
    std::fs::remove_dir_all(&first).unwrap();
    git(&world.clone, &["worktree", "prune"]);

    assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut done"]);
    let failed = failures(&world);
    assert_eq!(failed.len(), 1, "{failed:?}");
    let issues = world.gh()["issues"].as_array().unwrap().clone();
    let opened = issues.iter().find(|issue| issue["number"] == failed[0].0).unwrap();
    assert_eq!(opened["title"], "afkconvoy: the cut of v1.2.3 failed", "{opened}");
    assert!(opened["body"].as_str().unwrap().contains("its checkout is gone"), "{opened}");
    assert!(!claimed(&world));
    for now in [10_200, 10_300 + 1800 + 60] {
        assert_eq!(standing(&world, "alpha", now), "failed");
        assert_eq!(cut(&tick_as(&world, "alpha", now)), Vec::<String>::new(), "no restart");
    }
    assert_eq!(failures(&world).len(), 1, "one issue");

    world.close_issue(failed[0].0);
    assert_eq!(cut(&tick_as(&world, "alpha", 20_000)), ["cut done"]);
    let answer = finish(&world, &self::checkout(&world));
    assert_eq!(answer["commit"], made, "{answer}");
    assert_eq!(cut_branches(&world), pushed, "no second branch for the same version");
}

/// A cut gated red pushed its branch, untagged; a fix lands and its issue
/// is closed, so the next cut starts again from the start. That cut makes a
/// new commit on the base as it stands now, on a branch of its own, and
/// gates what its record says it holds — not the stale branch's base, as
/// the v0.5.74 cut did twice (control#639).
#[test]
fn a_cut_started_again_after_the_base_moved_makes_a_new_commit_on_the_new_base() {
    let world = initial(false);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let first = checkout(&world);
    std::fs::write(world.root.join("red"), "red").unwrap();
    let failed = world.afk_in(&first, &["cut", "--host", "h1"]).ok();
    assert_eq!(failed["failed"], "gate", "{failed}");
    std::fs::remove_file(world.root.join("red")).unwrap();
    let pushed = cut_branches(&world);
    assert_eq!(pushed.len(), 1, "{pushed:?}");
    let stale = world.remote(&["rev-parse", &format!("refs/heads/{}", pushed[0])]);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut done"]);
    let failed = failures(&world);
    assert_eq!(failed.len(), 1, "{failed:?}");

    land(&world, "Fix the gate (#9)");
    let tip = world.remote(&["rev-parse", BASE]);
    world.close_issue(failed[0].0);
    assert_eq!(cut(&tick_as(&world, "alpha", 20_000)), ["cut done"]);
    let again = self::checkout(&world);
    let held = record(&again);
    assert_eq!((&held["version"], &held["prs"]), (&json!("1.2.3"), &json!([7, 9])), "{held}");

    let answer = finish(&world, &again);
    let made = answer["commit"].as_str().unwrap();
    assert_ne!(made, stale, "{answer}");
    assert_eq!(answer["prs"], json!([7, 9]), "{answer}");
    assert_eq!(parents(&world, made), vec![tip.clone()], "one parent, the base as it is now");
    assert!(ancestor(&world, &tip, made), "the commit it gated holds #9");
    assert_eq!(world.remote(&["show", &format!("{made}:VERSION")]), "1.2.3");
    let branches = cut_branches(&world);
    assert_eq!(branches.len(), 2, "a branch of its own: {branches:?}");
    assert_ne!(answer["branch"], json!(pushed[0]), "{answer}");
}

/// A peer's cut whose host went silent is a failed cut too: the tick that
/// unclaims it opens the one issue, named by the version its branch says,
/// and no cut of that version starts on its own.
#[test]
fn a_silent_peer_s_cut_opens_its_issue_and_starts_no_cut_on_its_own() {
    let world = cutting();
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    git(&checkout, &["push", "-q", "origin", "HEAD:refs/heads/afkconvoy-cut/v0.1.1-10000"]);
    let late = 10_000 + 1801 + 60;
    assert_eq!(standing(&world, "beta", late), "stale_run");
    assert_eq!(cut(&tick_as(&world, "beta", late)), ["end_cut done"]);
    let failed = failures(&world);
    assert_eq!(failed.len(), 1, "{failed:?}");
    let issues = world.gh()["issues"].as_array().unwrap().clone();
    let opened = issues.iter().find(|issue| issue["number"] == failed[0].0).unwrap();
    assert_eq!(opened["title"], "afkconvoy: the cut of v0.1.1 failed", "{opened}");
    assert!(opened["body"].as_str().unwrap().contains("h1, went silent"), "{opened}");
    assert!(!claimed(&world));
    assert_eq!(standing(&world, "beta", late + 10), "failed");
    assert_eq!(cut(&tick_as(&world, "beta", late + 10)), Vec::<String>::new());
}

/// A cut takes no turn: while its gate hangs, a turn on the same repo lands.
#[test]
fn a_turn_lands_while_a_cut_s_gate_hangs() {
    let world = World::new();
    let go = world.root.join("go");
    let started = world.root.join("cut-gate-started");
    // The cut's commit alone holds VERSION: only its gate waits. A heavy
    // gate, as this repo's own is, in the host's worker lanes.
    let script = world.root.join("gate-or-wait.sh");
    std::fs::write(
        &script,
        format!(
            "if test -f VERSION; then touch '{}'; while test ! -f '{}'; do sleep 0.05; done; fi\n",
            started.display(),
            go.display()
        ),
    )
    .unwrap();
    let gate = format!("afkconvoy exclusive -- sh '{}'", script.display());
    // A lane for the cut's gate and one for the turn: capacity is the
    // host's, and what this shows is that the cut holds no barrier and
    // shares its repo as a candidate does.
    world.say_host_settings(json!({"gate_lanes": 2}));
    tag(&world, "v0.1.0");
    commit_config(
        &world,
        &format!("{}\n{}", CONFIG.replace("./gate.sh", &gate), cut_block(&says_published(&world))),
        "Turn cutting on (#7)",
    );
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    // The base tip is on record: no certification waits on the cut's lane.
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true);
    let cutting = checkout(&world);
    let mut child = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &cutting)
        .args(["cut", "--host", "h1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(60);
    while !started.exists() {
        assert!(child.try_wait().unwrap().is_none(), "the cut ended before its gate");
        assert!(Instant::now() < until, "the cut's gate never started");
        std::thread::sleep(Duration::from_millis(50));
    }

    // An issue's turn: a candidate, as the five PRs held behind the
    // FelixMBP14 cut's barrier were.
    let fix = world.worker(41, "fixed.txt", "landed meanwhile");
    let said = world.root.join("landing-said");
    let mut landing = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &fix)
        .args(["land", "--issue", "41", "--host", "h1"])
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&said).unwrap())
        .spawn()
        .unwrap();
    let until = Instant::now() + Duration::from_secs(60);
    while landing.try_wait().unwrap().is_none() {
        if Instant::now() >= until {
            let _ = landing.kill();
            let _ = child.kill();
            panic!("the turn waited on the cut: {}", std::fs::read_to_string(&said).unwrap());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let joined: Value =
        serde_json::from_slice(&landing.wait_with_output().unwrap().stdout).unwrap();
    assert_eq!(joined["outcome"], "merged", "{joined}");
    let landed = joined["commit"].as_str().unwrap().to_string();
    assert_eq!(world.remote(&["rev-parse", BASE]), landed);
    assert!(child.try_wait().unwrap().is_none(), "the cut's gate still hangs");
    no_cut_on_the_chain(&world);

    std::fs::write(&go, "go").unwrap();
    let output = child.wait_with_output().unwrap();
    let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["tag"], "v0.1.1", "{answer}");
    assert_eq!(world.remote(&["rev-parse", BASE]), landed, "the cut added nothing to the base");
    let tagged = answer["commit"].as_str().unwrap();
    assert!(!ancestor(&world, tagged, &landed));
    let snapshot = parents(&world, tagged).pop().unwrap();
    let since = world.remote(&["rev-list", "--count", &format!("{snapshot}..{landed}")]);
    let past = planned(&world, "alpha", 10_100)["cut"]["past"].clone();
    assert_eq!(past.to_string(), since, "past the tag is past its parent, the snapshot");
    assert_eq!(since, "2", "the turn's merge commit and its PR's commit");
}

/// The first version takes every release step, whether setting it changes
/// a file or it was already set. Later versions are picked from its tag.
fn first_cut(preset: bool) {
    let world = initial(preset);
    let config = world.afk(&["config"]).ok();
    assert_eq!(config["cut"]["initial"], "1.2.3");
    let plan = planned(&world, "alpha", 10_000);
    assert_eq!(plan["cut"]["standing"], "due", "{plan}");
    assert_eq!(plan["cut"]["initial"], "1.2.3");
    assert_eq!(plan["cut"]["past"], 0);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    let first = record(&checkout);
    assert_eq!(first["version"], "1.2.3");
    assert!(first["from"].is_null());
    assert_eq!(first["prs"], json!([7]));
    assert_eq!(standing(&world, "beta", 10_010), "peer_running");
    assert_eq!(cut(&tick_as(&world, "beta", 10_010)), Vec::<String>::new());
    assert_eq!(world.worktrees().iter().filter(|p| p.ends_with("afkconvoy-cut")).count(), 1);

    // Failure at gate leaves one PR and a record that resumes at gate. A
    // peer still starts no cut, and the retry closes the reported failure.
    std::fs::write(world.root.join("red"), "red").unwrap();
    let failed = world.afk_in(&checkout, &["cut", "--host", "h1"]);
    let screen = failed.stderr.clone();
    let failed = failed.ok();
    assert_eq!(failed["failed"], "gate", "{failed}");
    assert!(screen.lines().next().unwrap().contains("v1.2.3 (the initial version"), "{screen}");
    assert!(screen.contains("[afkconvoy] cut: gate started\n"), "{screen}");
    let ended = screen.lines().last().unwrap();
    assert!(ended.starts_with("[afkconvoy] cut: gate failed after "), "{screen}");
    assert!(ended.contains("s: "), "{screen}");
    assert!(!screen.contains("[afkconvoy] cut: tag started"), "{screen}");
    assert_ne!(git(&checkout, &["rev-parse", "HEAD"]), git(&world.clone, &["rev-parse", "HEAD"]));
    let same_tree = git(&checkout, &["rev-parse", "HEAD^{tree}"])
        == git(&world.clone, &["rev-parse", "HEAD^{tree}"]);
    assert_eq!(same_tree, preset, "a version already set still gets a commit");
    assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut done"]);
    assert_eq!(standing(&world, "beta", 10_110), "failed");
    assert_eq!(cut(&tick_as(&world, "beta", 10_110)), Vec::<String>::new());
    std::fs::remove_file(world.root.join("red")).unwrap();

    let answer = finish(&world, &checkout);
    assert_eq!(answer["tag"], "v1.2.3", "{answer}");
    assert_eq!(answer["prs"], json!([7]));
    assert_eq!(answer["closed"].as_array().unwrap().len(), 1);
    let steps: Vec<_> = answer["took"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["step"].as_str().unwrap())
        .collect();
    assert_eq!(steps, ["set", "push", "gate", "tag", "publish"]);
    let tagged = world.remote(&["rev-parse", "v1.2.3^{commit}"]);
    assert_eq!(answer["commit"], tagged);
    assert_eq!(parents(&world, &tagged), vec![world.remote(&["rev-parse", BASE])]);
    assert_eq!(std::fs::read_to_string(world.root.join("published")).unwrap(), "1.2.3 v1.2.3\n");
    assert_eq!(world.gh()["prs"].as_array().unwrap().len(), 1, "the cut opened no PR");
    assert!(failures(&world).is_empty());
    // The failed run already gave its claim back; the manual continuation
    // closed its failure and left no run for the tick to end again.
    assert_eq!(cut(&tick_as(&world, "alpha", 10_200)), Vec::<String>::new());
    assert_eq!(standing(&world, "alpha", 10_210), "tagged");
    assert!(!claimed(&world));

    // The initial declaration stays in config, but only landings past the
    // new tag choose the next patch or minor; old release:minor labels do not.
    git(&world.clone, &["fetch", "-q", "origin"]);
    git(&world.clone, &["merge", "--ff-only", "origin/main"]);
    land(&world, "Add a gadget (#9)");
    world.with_gh(|gh| {
        gh["prs"].as_array_mut().unwrap().push(json!({
        "number": 9, "title": "Add a gadget", "headRefName": "nine", "headRefOid": "",
        "body": "", "state": "merged", "labels": if preset { vec!["release:minor"] } else { vec![] }
    }))
    });
    assert!(planned(&world, "alpha", 10_300)["cut"].get("initial").is_none());
    assert_eq!(cut(&tick_as(&world, "alpha", 10_300)), ["cut done"]);
    let next = record(&self::checkout(&world));
    assert_eq!(next["from"], "v1.2.3");
    assert_eq!(next["version"], if preset { "1.3.0" } else { "1.2.4" });
    assert_eq!(next["prs"], json!([9]));
}

#[test]
fn an_untagged_repo_cuts_its_declared_initial_version_and_then_the_next_patch() {
    first_cut(false);
}

#[test]
fn an_initial_version_already_set_is_gated_tagged_and_cut_then_the_next_minor() {
    first_cut(true);
}

#[test]
fn the_initial_cut_obeys_admission_and_a_fleet_lock() {
    let world = initial(false);
    world.busy("1000");
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), Vec::<String>::new());
    assert!(!claimed(&world));
    world.busy("0");
    world.issue(9, &["fleet-lock"]);
    assert_eq!(standing(&world, "alpha", 10_100), "locked");
    assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), Vec::<String>::new());
    assert!(!claimed(&world));
    world.close_issue(9);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_200)), ["cut done"]);
}

#[test]
fn a_busy_fleet_cuts_at_the_age_bound_with_claims_and_a_frontier_left() {
    let world = cutting();
    let landed: u64 = git(&world.clone, &["log", "-1", "--format=%ct"]).trim().parse().unwrap();
    for issue in 1..=4 {
        world.issue(issue, &["ready-for-agent"]);
    }
    let before = tick_as(&world, "alpha", landed + 17_999);
    assert!(cut(&before).is_empty(), "{before}");
    assert!(!claimed(&world));
    let plan = planned(&world, "alpha", landed + 17_999);
    assert_eq!(plan["cut"]["oldest_landing_age_seconds"], 17_999, "{plan}");
    assert_eq!(plan["cut"]["busy_in_seconds"], 1, "{plan}");
    let at_bound = tick_as(&world, "alpha", landed + 18_000);
    assert_eq!(cut(&at_bound), ["cut done"], "{at_bound}");
    assert_eq!(at_bound["held"], 3, "the cut takes no slot: {at_bound}");
    assert_eq!(at_bound["frontier_remaining"], 1, "{at_bound}");
    assert!(logged(&world).iter().any(|line| line.contains("fleet busy")));
    assert!(claimed(&world));
    assert_eq!(standing(&world, "beta", landed + 18_001), "peer_running");
    assert!(cut(&tick_as(&world, "beta", landed + 18_001)).is_empty());
}

#[test]
fn a_repo_can_shorten_the_busy_bound_and_admission_does_not_hold_an_overdue_cut() {
    let world = World::new();
    commit_config(
        &world,
        &format!(
            "{CONFIG}\n{}\n```yaml cut\nbusy_after_seconds: 120\n```\n",
            cut_block(&says_published(&world))
        ),
        "Turn cutting on",
    );
    tag(&world, "v0.1.0");
    land_at(&world, "Add a widget (#9)", 10_000);
    land_at(&world, "Add a gadget (#10)", 10_100);
    world.busy("1000");
    let plan = planned(&world, "alpha", 10_119);
    assert_eq!(plan["cut"]["oldest_landing_age_seconds"], 119, "{plan}");
    assert_eq!(plan["cut"]["busy_in_seconds"], 1, "{plan}");
    assert!(cut(&tick_as(&world, "alpha", 10_119)).is_empty());
    assert_eq!(cut(&tick_as(&world, "alpha", 10_120)), ["cut done"]);
}

#[test]
fn an_idle_tick_starts_one_cut_of_the_next_patch_and_a_peer_starts_none() {
    let world = cutting();
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    let record = record(&checkout);
    assert_eq!(record["version"], "0.1.1", "{record}");
    assert_eq!(record["prs"], json!([7]), "{record}");
    assert!(claimed(&world));
    assert!(logged(&world).iter().any(|line| line.contains("fleet idle")));
    // The cut is this command, in a terminal of its checkout.
    let created =
        world.orca_calls().into_iter().filter(|call| common::asked(call) == "terminal create");
    let created: Vec<String> = created.map(|call| call.join(" ")).collect();
    assert_eq!(created.len(), 1, "{created:?}");
    assert!(created[0].contains(" cut; exit"), "{created:?}");
    assert!(created[0].contains("--title afkconvoy cut v0.1.1"), "{created:?}");

    // Mine is running; a peer finds it claimed and starts none.
    assert_eq!(standing(&world, "alpha", 10_010), "running");
    assert_eq!(standing(&world, "beta", 10_010), "peer_running");
    assert_eq!(cut(&tick_as(&world, "beta", 10_010)), Vec::<String>::new());
    assert_eq!(world.worktrees().iter().filter(|p| p.ends_with("afkconvoy-cut")).count(), 1);
}

#[test]
fn a_cut_shows_its_plan_and_each_step_around_the_commands_output() {
    let world = cutting_publishing(|world| {
        format!(
            "printf 'publish stdout\\n'\nprintf 'publish stderr\\n' >&2\n\
             while test ! -f '{}'; do sleep 0.01; done\n{}\nprintf 'no final newline'",
            world.root.join("finish-publish").display(),
            says_published(world)
        )
    });
    let file = world.clone.join("docs/agents/afkconvoy.md");
    let config = std::fs::read_to_string(file)
        .unwrap()
        .replace("gate_command: ./gate.sh", "gate_command: sh ./gate.sh")
        .replacen(
            "set: |\n",
            "set: |\n  printf 'set stdout\\n'\n  printf 'set stderr\\n' >&2\n",
            1,
        );
    std::fs::write(
        world.clone.join("gate.sh"),
        "#!/bin/sh\nprintf 'gate stdout\\n'\nprintf 'gate stderr\\n' >&2\n",
    )
    .unwrap();
    git(&world.clone, &["add", "gate.sh"]);
    commit_config(&world, &config, "Make the steps visible");
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    let screen_file = world.root.join("cut-screen");
    let mut child = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &checkout)
        .args(["cut", "--host", "h1"])
        .stdout(Stdio::piped())
        .stderr(std::fs::File::create(&screen_file).unwrap())
        .spawn()
        .unwrap();
    // Publish cannot end until this test has seen its output: progress
    // buffered until the cut exits would fail this handshake.
    let until = Instant::now() + Duration::from_secs(30);
    let during = loop {
        let screen = std::fs::read_to_string(&screen_file).unwrap();
        if screen.contains("publish stderr\n")
            || child.try_wait().unwrap().is_some()
            || Instant::now() >= until
        {
            break screen;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    std::fs::write(world.root.join("finish-publish"), "go\n").unwrap();
    let output = child.wait_with_output().unwrap();
    let screen = std::fs::read_to_string(screen_file).unwrap();
    assert!(
        during.contains("[afkconvoy] cut: publish started\npublish stdout\npublish stderr\n"),
        "{screen}"
    );
    assert!(!during.contains("[afkconvoy] cut: publish passed"), "{during}");
    assert!(output.status.success(), "{screen}");
    let answer: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(answer["tag"], "v0.1.1", "{answer}");
    assert_eq!(answer["took"].as_array().unwrap().len(), 5, "{answer}");
    let first = screen.lines().next().unwrap();
    assert!(first.contains("v0.1.1 (past v0.1.0, holding #7)"), "{first}");
    assert!(first.contains("steps: set, push, gate, tag, publish"), "{first}");
    assert!(first.contains(&format!("record: {}", record_file(&checkout).display())), "{first}");
    let log_file = record_file(&checkout).with_file_name("convoy-cut.log");
    assert!(first.contains(&format!("log: {}", log_file.display())), "{first}");
    let mut rest = screen.as_str();
    for step in answer["took"].as_array().unwrap() {
        let name = step["step"].as_str().unwrap();
        let seconds = step["seconds"].as_u64().unwrap();
        let start = format!("[afkconvoy] cut: {name} started\n");
        rest = rest.split_once(&start).unwrap_or_else(|| panic!("{screen}")).1;
        let end = format!("[afkconvoy] cut: {name} passed after {seconds}s\n");
        let (output, after) = rest.split_once(&end).unwrap_or_else(|| panic!("{screen}"));
        if ["set", "gate", "publish"].contains(&name) {
            assert!(output.contains(&format!("{name} stdout\n{name} stderr\n")), "{screen}");
        }
        if name == "publish" {
            assert!(output.ends_with("no final newline\n"), "{screen}");
        }
        rest = after;
    }
    let log = std::fs::read_to_string(log_file).unwrap();
    for name in ["set", "publish"] {
        assert!(log.contains(&format!("{name} stdout\n{name} stderr\n")), "{log}");
    }
    assert!(log.ends_with("no final newline"), "{log}");
}

#[test]
fn a_pr_labelled_for_a_minor_version_raises_the_minor_one() {
    let world = cutting();
    world.with_gh(|gh| {
        let pr = json!({"number": 7, "title": "Turn cutting on", "headRefName": "seven",
            "headRefOid": "", "body": "", "state": "merged", "labels": ["release:minor"]});
        gh["prs"].as_array_mut().unwrap().push(pr);
    });
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let record = record(&checkout(&world));
    assert_eq!((&record["version"], &record["minor"]), (&json!("0.2.0"), &json!(true)));
}

#[test]
fn a_cut_whose_terminal_is_gone_opens_one_issue_and_no_cut_starts_while_it_is_open() {
    let world = cutting();
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    workers_leave(&world);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut done"]);
    let escalate = world.afk(&["config"]).ok()["escalate_label"].as_str().unwrap().to_string();
    let failed = failures(&world);
    assert_eq!(failed.len(), 1, "{failed:?}");
    assert!(failed[0].1.contains(&escalate), "{failed:?}");
    assert!(!claimed(&world));

    assert_eq!(standing(&world, "alpha", 10_200), "failed");
    let issue = failed[0].0;
    let comment = world.comments(issue).pop().unwrap();
    let url = world.comment_url(issue, &comment);
    world.comment(issue, "A later investigation update.");
    for command in ["plan", "doctor"] {
        let view = world.afk(&[command, "--now", "10200"]).json;
        let waiting = &view["waiting_on_human"][0];
        assert_eq!(waiting["issue"], issue, "{view}");
        assert_eq!(
            waiting["why"],
            "says a cut failed: the step `set` failed: it was ended from outside: its terminal is gone, and it said nothing"
        );
        assert_eq!(waiting["url"], url);
    }
    world.with_gh(|gh| gh["comments"][issue.to_string()] = json!([]));
    let old = world.afk(&["plan", "--now", "10200"]).ok();
    let waiting = &old["waiting_on_human"][0];
    assert!(waiting["why"].as_str().unwrap().contains("its terminal is gone"), "{waiting}");
    assert_eq!(waiting["url"], format!("https://gh/i/{issue}"));
    assert_eq!(cut(&tick_as(&world, "alpha", 10_200)), Vec::<String>::new());
    assert_eq!(failures(&world).len(), 1);

    world.close_issue(failed[0].0);
    assert_eq!(standing(&world, "alpha", 10_300), "due");
}

#[test]
fn a_cut_tags_its_commit_and_publishes_with_the_tag_checked_out() {
    let world = cutting();
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    let base = world.remote(&["rev-parse", BASE]);
    let landed = its_commit_is_made(&checkout);

    let cut_answer = world.afk_in(&checkout, &["cut", "--from", "tag", "--host", "h1"]);
    let screen = cut_answer.stderr.clone();
    let cut_answer = cut_answer.ok();
    assert_eq!(cut_answer["tag"], "v0.1.1", "{cut_answer}");
    let first = screen.lines().next().unwrap();
    assert!(first.contains("v0.1.1 (past v0.1.0, holding #7)"), "{screen}");
    assert!(first.contains("steps: tag, publish;"), "{screen}");
    assert!(!screen.contains("[afkconvoy] cut: set started"), "{screen}");
    for step in ["tag", "publish"] {
        assert!(screen.contains(&format!("[afkconvoy] cut: {step} started\n")), "{screen}");
        assert!(screen.contains(&format!("[afkconvoy] cut: {step} passed after ")), "{screen}");
    }
    let tagged = world.remote(&["rev-parse", "v0.1.1^{commit}"]);
    assert_eq!(tagged.trim(), landed);
    let published = std::fs::read_to_string(world.root.join("published")).unwrap();
    assert_eq!(published, "0.1.1 v0.1.1\n");
    assert_eq!(git(&world.bare, &["rev-parse", BASE]).trim(), base, "pushed nothing to the base");

    // The next tick finds it over: says so, and takes its checkout down.
    let ticked = tick_as(&world, "alpha", 10_100);
    assert_eq!(cut(&ticked), ["end_cut done"]);
    let logged = logged(&world);
    let said: Vec<_> = logged.iter().filter(|line| line.starts_with("cut: ")).collect();
    assert_eq!(said.len(), 2, "{logged:#?}");
    assert!(said[0].starts_with("cut: started the cut of v0.1.1"), "{said:#?}");
    assert!(said[1].ends_with("tagged, published and installed"), "{said:#?}");
    assert!(!claimed(&world));
    assert!(!checkout.exists());
    assert_eq!(standing(&world, "alpha", 10_200), "tagged");
}

#[test]
fn an_end_of_a_cut_that_fails_after_it_said_the_cut_ended_still_says_so() {
    let world = cutting();
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    its_commit_is_made(&checkout);
    world.afk_in(&checkout, &["cut", "--from", "tag", "--host", "h1"]).ok();

    // The cut is over, and its checkout cannot be removed: the step fails
    // after it said the cut ended, and the tick says both.
    world.with_orca(|orca| orca["rm_fails"] = json!(true));
    let ticked = tick_as(&world, "alpha", 10_100);
    let said = cut(&ticked);
    assert!(said.len() == 1 && said[0].starts_with("end_cut failed: "), "{ticked}");
    let logged = logged(&world);
    let ended: Vec<_> = logged.iter().filter(|line| line.starts_with("cut: ")).collect();
    assert_eq!(ended.len(), 2, "{logged:#?}");
    assert!(ended[1].ends_with("tagged, published and installed"), "{ended:#?}");
    assert!(claimed(&world), "the claim is held for the next tick to end it");
}

#[test]
fn a_minor_landing_before_or_after_the_cuts_own_commit_supersedes_the_patch() {
    for minor_after_cut in [false, true] {
        let world = cutting();
        assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
        let checkout = checkout(&world);
        // A minor may land on either side of the cut's own landing.
        if minor_after_cut {
            its_commit_is_made(&checkout);
        }
        land(&world, "Add a gadget (#9)");
        world.with_gh(|gh| {
            let pr = json!({"number": 9, "title": "Add a gadget", "headRefName": "nine",
                "headRefOid": "", "body": "", "state": "merged", "labels": ["release:minor"]});
            gh["prs"].as_array_mut().unwrap().push(pr);
        });
        if !minor_after_cut {
            its_commit_is_made(&checkout);
        }

        let stopped = world.afk_in(&checkout, &["cut", "--from", "tag", "--host", "h1"]).ok();
        assert_eq!(stopped["stopped"], "tag", "{stopped}");
        assert!(stopped.get("failed").is_none(), "{stopped}");
        assert!(
            stopped["why"].as_str().unwrap().contains("#9, labelled `release:minor`"),
            "{stopped}"
        );
        assert!(world.remote(&["tag", "--list", "v0.1.1"]).trim().is_empty(), "tagged nothing");
        assert!(!world.root.join("published").exists());
        assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut superseded", "cut done"]);
        assert!(failures(&world).is_empty(), "no human is needed");
        assert_eq!(record(&self::checkout(&world))["version"], "0.2.0");
        let said = logged(&world);
        assert!(
            said.iter().any(|line| line.contains("stopped at `tag`, without tagging")),
            "{said:?}"
        );
    }
}

/// A line of a step that fails fails the step there, and the lines after it
/// are not run: a publish that fails is not passed by the upgrade after it,
/// which finds nothing newer and succeeds. Lines that begin `set -ex` run as
/// they did.
#[test]
fn a_line_that_fails_fails_its_step_and_the_lines_after_it_are_not_run() {
    for first in ["", "set -ex\n"] {
        let world = cutting_publishing(|world| {
            format!("{first}./packaging/release.sh publish \"$VERSION\"\n{}", says_published(world))
        });
        assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
        let checkout = checkout(&world);
        its_commit_is_made(&checkout);

        let failed = world.afk_in(&checkout, &["cut", "--from", "tag", "--host", "h1"]).ok();
        assert_eq!(failed["failed"], "publish", "{first:?}: {failed}");
        assert!(failed["why"].as_str().unwrap().starts_with("its lines exited "), "{failed}");
        assert!(!world.root.join("published").exists(), "{first:?}: the line after it ran");
        let log = std::fs::read_to_string(failed["log"].as_str().unwrap()).unwrap();
        assert!(log.contains("packaging/release.sh"), "{first:?}: {log}");

        // The next tick ends it, and says so: the issue it opens names the step.
        assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut done"]);
        let said = logged(&world);
        assert!(said.iter().any(|line| line.contains("FAILED at its step `publish`")), "{said:#?}");
        let issues = world.gh()["issues"].as_array().unwrap().clone();
        let opened = issues.iter().find(|issue| issue["number"] == failures(&world)[0].0).unwrap();
        let body = opened["body"].as_str().unwrap();
        assert!(body.contains("failed at its step `publish`: its lines exited "), "{body}");
        assert!(body.contains("afkconvoy cut --from 'publish'"), "{body}");
    }
}

/// A start that fails once it holds the claim gives it back: the checkout a
/// failed cut left is not read as the new one's.
#[test]
fn a_start_that_fails_after_its_claim_gives_it_back() {
    let world = cutting();
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    workers_leave(&world);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut done"]);
    let failed = failures(&world);
    world.close_issue(failed[0].0);

    // The checkout the failed cut left cannot be removed.
    world.with_orca(|orca| orca["rm_fails"] = json!(true));
    let ticked = cut(&tick_as(&world, "alpha", 10_200));
    assert!(ticked[0].starts_with("cut failed: "), "{ticked:?}");
    assert!(!claimed(&world), "the claim was given back");
    assert_eq!(standing(&world, "alpha", 10_300), "due");
    assert!(failures(&world).is_empty(), "the failed cut is not said again");

    world.with_orca(|orca| orca["rm_fails"] = json!(false));
    assert_eq!(cut(&tick_as(&world, "alpha", 10_300)), ["cut done"]);
    assert!(failures(&world).is_empty());
}

#[test]
fn cut_is_refused_outside_a_cut_s_checkout() {
    let world = cutting();
    let refused = world.afk(&["cut"]).error();
    assert!(refused.contains("no cut's checkout"), "{refused}");
}

#[test]
fn cut_needs_leave_the_claim_for_a_host_that_can_cut_without_disclosing_output() {
    let world = cutting();
    let permitted = world.root.join("cut-permitted");
    let config = format!(
        "{CONFIG}\n{}\n```yaml cut-needs\nchecks:\n  - name: publish-access\n    command: echo secret-marker; echo private-detail >&2; test -f '{}'\n    seconds: 5\n```\n",
        cut_block(&says_published(&world)),
        permitted.display()
    );
    commit_config(&world, &config, "Declare cut needs");
    let plan = planned(&world, "alpha", 10_000);
    assert_eq!(
        plan["cut"]["not_cutting_here"], "not cutting here: publish-access failed",
        "{plan}"
    );
    let tick = tick_as(&world, "alpha", 10_000);
    assert!(!claimed(&world), "{tick}");
    assert!(tick.to_string().contains("not cutting here: publish-access failed"), "{tick}");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "10000"]).ok();
    assert!(doctor.to_string().contains("not cutting here: publish-access failed"), "{doctor}");
    assert!(!format!("{plan}{tick}{doctor}{}", logged(&world).join(" ")).contains("secret-marker"));
    std::fs::write(&permitted, "yes").unwrap();
    let passing = world.afk(&["tick", "--instance", "beta", "--host", "h2", "--now", "10010"]).ok();
    assert_eq!(cut(&passing), ["cut done"]);
    assert!(claimed(&world));
    assert!(!record(&checkout(&world)).to_string().contains("secret-marker"));
}

#[test]
fn a_cut_need_installs_before_the_cut_is_claimed() {
    let world = cutting();
    let permitted = world.root.join("cut-permitted");
    let installs = world.root.join("cut-installs");
    let config = format!(
        "{CONFIG}\n{}\n```yaml cut-needs\nchecks:\n  - name: publish-access\n    command: test -f '{}'\n    seconds: 5\n    install: echo ran >> '{}'; touch '{}'\n    install_seconds: 10\n```\n",
        cut_block(&says_published(&world)),
        permitted.display(),
        installs.display(),
        permitted.display()
    );
    commit_config(&world, &config, "Declare a cut need with its install");
    let tick = tick_as(&world, "alpha", 10_000);
    assert!(claimed(&world), "{tick}");
    assert_eq!(std::fs::read_to_string(&installs).unwrap(), "ran\n");
    assert!(!tick.to_string().contains("not cutting here"), "the install's pass is said: {tick}");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "10000"]).ok();
    assert!(doctor.to_string().contains("publish-access was installed on this host"), "{doctor}");
}

/// sunfmin/control#532: a cut need that fails at the rebuild's look and
/// passes when the cut is claimed. The tick says the evaluation it claimed
/// by, and no failed need beside the cut it starts.
#[test]
fn a_cut_need_that_fails_then_passes_is_not_said_beside_the_cut_it_starts() {
    let world = cutting();
    let looked = world.root.join("cut-looked");
    let config = format!(
        "{CONFIG}\n{}\n```yaml cut-needs\nchecks:\n  - name: publish-access\n    command: if ! test -f '{looked}'; then touch '{looked}'; echo first-look >&2; exit 1; fi\n    seconds: 5\n```\n",
        cut_block(&says_published(&world)),
        looked = looked.display()
    );
    commit_config(&world, &config, "Declare a cut need that fails once");
    let tick = tick_as(&world, "alpha", 10_000);
    assert!(looked.exists(), "the rebuild looked first: {tick}");
    assert!(claimed(&world), "{tick}");
    assert_eq!(cut(&tick), ["cut done"], "{tick}");
    assert!(!tick.to_string().contains("not cutting here"), "{tick}");
}

/// sunfmin/control#532: a cut need that passes at the rebuild's look and
/// fails when the cut would be claimed. No cut is claimed; the tick says the
/// failed need, and doctor names its stderr file on this host, never its
/// output.
#[test]
fn a_cut_need_that_passes_then_fails_claims_no_cut_and_doctor_names_its_stderr() {
    let world = cutting();
    let looked = world.root.join("cut-looked");
    let config = format!(
        "{CONFIG}\n{}\n```yaml cut-needs\nchecks:\n  - name: publish-access\n    command: if test -f '{looked}'; then echo private-late-failure >&2; exit 1; fi; touch '{looked}'\n    seconds: 5\n```\n",
        cut_block(&says_published(&world)),
        looked = looked.display()
    );
    commit_config(&world, &config, "Declare a cut need that passes once");
    let tick = tick_as(&world, "alpha", 10_000);
    assert!(!claimed(&world), "{tick}");
    assert!(!cut(&tick).contains(&"cut done".to_string()), "{tick}");
    assert!(tick.to_string().contains("not cutting here: publish-access failed"), "{tick}");
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", "10000"]).ok();
    let said = doctor.to_string();
    assert!(said.contains("not cutting here: publish-access failed"), "{doctor}");
    let stderr = said
        .split("stderr: ")
        .nth(1)
        .and_then(|rest| rest.split(['"', '\\']).next())
        .unwrap_or_else(|| panic!("doctor names the stderr file: {doctor}"));
    let lines = std::fs::read_to_string(stderr).unwrap();
    assert!(lines.contains("publish-access") && lines.contains("private-late-failure"), "{lines}");
    assert!(!format!("{tick}{doctor}{}", logged(&world).join(" ")).contains("private-late"));
}

/// The `cut-need` finding of a doctor's answer.
fn cut_need(world: &World, now: u64) -> Option<Value> {
    let doctor = world.afk(&["doctor", "--host", "h1", "--now", &now.to_string()]).ok();
    doctor["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|found| found["watch_id"].as_str().unwrap().contains("cut-need"))
        .cloned()
}

/// The stderr file a finding's evidence names.
fn named_stderr(found: &Value) -> String {
    let evidence = found["evidence"].as_str().unwrap();
    evidence.split("stderr: ").nth(1).unwrap().split(['"', ';']).next().unwrap().to_string()
}

/// sunfmin/control#570: a cut held first by one need, then by another, then
/// by both, is one `cut-need` condition. Its evidence names the needs failing
/// now and a stderr file that exists; a cut no longer held clears it.
#[test]
fn a_held_cut_is_one_cut_need_finding_whichever_need_fails() {
    let world = cutting();
    let (signing, upload) = (world.root.join("signing-ok"), world.root.join("upload-ok"));
    let config = format!(
        "{CONFIG}\n{}\n```yaml cut-needs\nchecks:\n  - name: signing\n    command: test -f '{}' || {{ echo no-signing >&2; exit 1; }}\n    seconds: 5\n  - name: upload\n    command: test -f '{}' || {{ echo no-upload >&2; exit 1; }}\n    seconds: 5\n```\n",
        cut_block(&says_published(&world)),
        signing.display(),
        upload.display()
    );
    commit_config(&world, &config, "Declare two cut needs");
    std::fs::write(&upload, "yes").unwrap();
    let first = cut_need(&world, 10_000).expect("signing holds the cut");
    assert_eq!(first["what"], "not cutting here: signing failed", "{first}");

    std::fs::write(&signing, "yes").unwrap();
    std::fs::remove_file(&upload).unwrap();
    let then = cut_need(&world, 10_010).expect("upload holds the cut");
    assert_eq!(then["what"], "not cutting here: upload failed", "{then}");
    assert_eq!(then["watch_id"], first["watch_id"], "one held cut, one condition");
    assert_eq!(named_stderr(&then), named_stderr(&first), "{then}");
    let lines = std::fs::read_to_string(named_stderr(&then)).unwrap();
    assert!(lines.contains("no-upload") && !lines.contains("no-signing"), "{lines}");

    std::fs::remove_file(&signing).unwrap();
    let both = cut_need(&world, 10_020).expect("both hold the cut");
    assert_eq!(both["what"], "not cutting here: signing, upload failed", "{both}");
    assert_eq!(both["watch_id"], first["watch_id"]);
    let evidence = both["evidence"].as_str().unwrap();
    assert!(evidence.contains("signing failed") && evidence.contains("upload failed"), "{both}");
    let lines = std::fs::read_to_string(named_stderr(&both)).unwrap();
    assert!(lines.contains("no-signing") && lines.contains("no-upload"), "{lines}");
    assert!(!both.to_string().contains("no-signing"), "never its output: {both}");

    std::fs::write(&signing, "yes").unwrap();
    std::fs::write(&upload, "yes").unwrap();
    assert_eq!(cut_need(&world, 10_030), None, "a cut no longer held clears it");
    assert!(!std::path::Path::new(&named_stderr(&both)).exists());
}

/// The plan's frontier, by number.
fn frontier(world: &World, now: u64) -> Vec<u64> {
    let plan = planned(world, "alpha", now);
    let rows = plan["frontier"].as_array().unwrap_or_else(|| panic!("{plan}"));
    rows.iter().map(|row| row["number"].as_u64().unwrap()).collect()
}

/// `cutting`, with a gate its cut can pass: red only while `red` exists.
fn cutting_green() -> World {
    let world = World::new();
    tag(&world, "v0.1.0");
    let gate = format!("test ! -f '{}'", world.root.join("red").display());
    let config = CONFIG.replace("./gate.sh", &gate);
    commit_config(
        &world,
        &format!("{config}\n{}", cut_block(&says_published(&world))),
        "Turn cutting on (#7)",
    );
    world
}

#[test]
fn an_issue_waiting_on_the_cut_lets_the_fleet_go_idle_and_returns_once_the_cut_installs() {
    let world = cutting_green();
    world.issue(8, &["ready-for-agent"]);
    let dispatched = tick_as(&world, "alpha", 10_000);
    assert!(cut(&dispatched).is_empty(), "a claim held: no idle cut yet: {dispatched}");
    assert_eq!(world.claims().len(), 1);

    // Its worker needs the next version, not installed yet: it waits on the cut.
    world.comment(8, "<!--convoy:verdict phase=blocked blocked_by=cut-->\nneeds the next version");
    let parked = tick_as(&world, "alpha", 10_010);
    assert_eq!(world.claims(), [] as [(u64, String); 0], "the claim is released");
    assert_eq!(world.labels(8), ["ready-for-agent", "convoy-waits-on-cut"]);
    assert_eq!(world.edges(8), [] as [u64; 0], "no issue is named");
    // Nothing else held, the fleet is idle: the due cut starts.
    assert_eq!(cut(&parked), ["cut done"], "{parked}");
    assert!(logged(&world).iter().any(|line| line.contains("fleet idle")));
    let plan = planned(&world, "alpha", 10_020);
    assert_eq!(plan["cut"]["waiting"], json!([8]), "{plan}");
    assert_eq!(plan["cut"]["standing"], "running", "{plan}");
    assert!(frontier(&world, 10_020).is_empty(), "still waiting while the cut runs: {plan}");
    let doctor = world.afk(&["doctor", "--now", "10020"]).ok().to_string();
    assert!(doctor.contains("#8 wait on the next cut"), "{doctor}");

    // Published and installed: the issue is on the frontier again.
    let checkout = checkout(&world);
    finish(&world, &checkout);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_100))[0], "end_cut done");
    assert_eq!(world.labels(8), ["ready-for-agent"]);
    assert_eq!(frontier(&world, 10_110), [8]);
    let plan = planned(&world, "alpha", 10_110);
    assert_eq!(plan["cut"]["waiting"], json!([]), "{plan}");
    tick_as(&world, "alpha", 10_120);
    assert_eq!(world.claims().len(), 1, "its worker is started again");
}

#[test]
fn a_failed_cut_leaves_what_waits_on_it_waiting() {
    let world = cutting_green();
    world.issue(8, &["ready-for-agent", "convoy-waits-on-cut"]);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    std::fs::write(world.root.join("red"), "red").unwrap();
    let failed = world.afk_in(&checkout, &["cut", "--host", "h1"]).ok();
    assert_eq!(failed["failed"], "gate", "{failed}");
    assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut done"]);
    assert_eq!(failures(&world).len(), 1);
    assert_eq!(world.labels(8), ["ready-for-agent", "convoy-waits-on-cut"]);
    assert!(frontier(&world, 10_110).is_empty());
    assert_eq!(world.claims(), [] as [(u64, String); 0]);
}

#[test]
fn a_verdict_on_the_cut_escalates_where_no_cut_will_come() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    tick_as(&world, "alpha", 10_000);
    world.comment(7, "<!--convoy:verdict phase=blocked blocked_by=cut-->\nneeds the next version");
    tick_as(&world, "alpha", 10_010);
    assert_eq!(world.claims(), [] as [(u64, String); 0]);
    assert_eq!(world.labels(7), ["ready-for-human"]);
    let escalation = world.comments(7).into_iter().find(|b| b.starts_with("<!--convoy:escalation"));
    let escalation = escalation.expect("an escalation comment");
    assert!(escalation.contains("the repo names no cut steps"), "{escalation}");
}

#[test]
fn a_cut_whose_gate_goes_red_names_the_failing_test_and_keeps_that_run_s_output() {
    let world = World::new();
    tag(&world, "v0.1.0");
    // A gate that, while `red` exists, fails as nextest says a test failed:
    // the test by name, what it printed after it, and a last line that names
    // no test.
    let script = world.root.join("red-gate");
    std::fs::write(
        &script,
        format!(
            "if [ -f '{red}' ]; then\n\
             echo '        PASS [   0.012s] (1/3) afkconvoy cutting::passes'\n\
             echo '        FAIL [   0.208s] (2/3) afkconvoy cutting::a_named_test'\n\
             echo '  stderr ───'\n\
             echo \"    thread 'cutting::a_named_test' panicked at tests/cutting.rs:9:5:\"\n\
             echo '    the widget was not cut'\n\
             echo '     Summary [   0.300s] 3 tests run: 2 passed, 1 failed, 0 skipped'\n\
             echo '        FAIL [   0.208s] (2/3) afkconvoy cutting::a_named_test'\n\
             echo 'gate: the seconds each step took: fmt 1s build 2s' >&2\n\
             exit 100\n\
             fi\n",
            red = world.root.join("red").display()
        ),
    )
    .unwrap();
    let config = CONFIG.replace("./gate.sh", &format!("sh '{}'", script.display()));
    commit_config(
        &world,
        &format!("{config}\n{}", cut_block(&says_published(&world))),
        "Turn cutting on (#7)",
    );
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    std::fs::write(world.root.join("red"), "red").unwrap();
    let failed = world.afk_in(&checkout, &["cut", "--host", "h1"]).ok();
    assert_eq!(failed["failed"], "gate", "{failed}");
    let output =
        PathBuf::from(failed["gate_output"].as_str().unwrap_or_else(|| panic!("{failed}")));
    let kept = std::fs::read_to_string(&output).unwrap();
    assert!(kept.contains("panicked at tests/cutting.rs:9:5"), "{kept}");
    let why = failed["why"].as_str().unwrap();
    assert!(why.contains("failing: afkconvoy cutting::a_named_test"), "{why}");
    assert!(!why.contains("its last line"), "{why}");

    // The fleet ends it: its issue names the test, quotes what it printed,
    // and names where the whole run is kept.
    assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut done"]);
    let failed = failures(&world);
    assert_eq!(failed.len(), 1, "{failed:?}");
    let issues = world.gh()["issues"].as_array().unwrap().clone();
    let issue = issues.iter().find(|issue| issue["number"] == failed[0].0).unwrap();
    let body = issue["body"].as_str().unwrap();
    assert!(body.contains("`afkconvoy cutting::a_named_test`"), "{body}");
    assert!(body.contains("    the widget was not cut"), "{body}");
    assert!(body.contains(&format!("`{}`", output.display())), "{body}");

    // Gone on from the gate, its run writes another file: the red run's stays.
    std::fs::remove_file(world.root.join("red")).unwrap();
    let answer = world.afk_in(&checkout, &["cut", "--host", "h1", "--from", "gate"]).ok();
    assert!(answer.get("failed").is_none(), "{answer}");
    assert_eq!(std::fs::read_to_string(&output).unwrap(), kept, "the red run's output stays");
    let dir = output.parent().unwrap();
    let runs: Vec<_> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("convoy-cut-gate-"))
        .collect();
    assert_eq!(runs.len(), 2, "each run keeps its own: {runs:?}");
}

/// A world whose cut's gate is a script: while `overloaded` holds a run,
/// each run takes one off and is red only on a TIMEOUT it says it cannot
/// classify on an overloaded host; while `quiet` exists, red on a TIMEOUT
/// on a host it does not say is overloaded; then green.
fn cutting_overloaded() -> World {
    let world = World::new();
    tag(&world, "v0.1.0");
    let script = world.root.join("overloaded-gate");
    std::fs::write(
        &script,
        format!(
            "runs=$(cat '{overloaded}' 2>/dev/null || echo 0)\n\
             echo run >> '{ran}'\n\
             if [ \"$runs\" -gt 0 ]; then\n\
             echo $((runs - 1)) > '{overloaded}'\n\
             echo '     TIMEOUT [   6.004s] (40/669) afkconvoy::cli dsh::the_line'\n\
             echo 'gate: host overloaded or load unavailable (load 12.71, CPUs 12); cannot classify TIMEOUT tests. Retry on a quiet host.' >&2\n\
             exit 100\n\
             fi\n\
             if [ -f '{quiet}' ]; then\n\
             echo '     TIMEOUT [   6.004s] (40/669) afkconvoy::cli dsh::the_line'\n\
             echo 'gate: afkconvoy::cli dsh::the_line was still TIMEOUT at 2s on a quiet host.' >&2\n\
             exit 100\n\
             fi\n",
            overloaded = world.root.join("overloaded").display(),
            quiet = world.root.join("quiet").display(),
            ran = world.root.join("ran").display(),
        ),
    )
    .unwrap();
    let config = CONFIG.replace("./gate.sh", &format!("sh '{}'", script.display()));
    commit_config(
        &world,
        &format!("{config}\n{}", cut_block(&says_published(&world))),
        "Turn cutting on (#7)",
    );
    world
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

fn gate_runs(world: &World) -> usize {
    std::fs::read_to_string(world.root.join("ran")).map_or(0, |ran| ran.lines().count())
}

#[test]
fn a_cut_s_gate_red_only_on_timeouts_it_cannot_classify_under_overload_gates_again() {
    let world = cutting_overloaded();
    let started = now();
    assert_eq!(cut(&tick_as(&world, "alpha", started)), ["cut done"]);
    let checkout = checkout(&world);
    std::fs::write(world.root.join("overloaded"), "2").unwrap();
    let answer = world.afk_in(&checkout, &["cut", "--host", "h1"]).ok();
    assert!(answer.get("failed").is_none(), "{answer}");
    assert_eq!(gate_runs(&world), 3, "two overloaded runs, then a green one");
    assert_eq!(cut(&tick_as(&world, "alpha", now()))[0], "end_cut done");
    assert!(failures(&world).is_empty(), "no failure issue opens");
    let help = std::process::Command::new(env!("CARGO_BIN_EXE_afkconvoy"))
        .args(["cut", "--help"])
        .output()
        .unwrap();
    let help =
        String::from_utf8_lossy(&help.stdout).split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(help.contains("cannot classify because the host is overloaded"), "{help}");
    assert!(help.contains("gates again in its place"), "{help}");
}

#[test]
fn a_cut_s_gate_red_on_a_timeout_on_a_quiet_host_fails_the_step_and_opens_its_issue() {
    let world = cutting_overloaded();
    assert_eq!(cut(&tick_as(&world, "alpha", now())), ["cut done"]);
    let checkout = checkout(&world);
    std::fs::write(world.root.join("quiet"), "quiet").unwrap();
    let failed = world.afk_in(&checkout, &["cut", "--host", "h1"]).ok();
    assert_eq!(failed["failed"], "gate", "{failed}");
    assert_eq!(gate_runs(&world), 1, "it is not gated again");
    let why = failed["why"].as_str().unwrap();
    assert!(why.contains("failing: afkconvoy::cli dsh::the_line"), "{why}");
    assert_eq!(cut(&tick_as(&world, "alpha", now()))[0], "end_cut done");
    assert_eq!(failures(&world).len(), 1);
}

#[test]
fn a_cut_whose_time_holds_no_other_gate_run_fails_on_an_overloaded_timeout() {
    let world = cutting_overloaded();
    // Started long ago: its time holds no wait and whole gate run more.
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    std::fs::write(world.root.join("overloaded"), "5").unwrap();
    let failed = world.afk_in(&checkout, &["cut", "--host", "h1"]).ok();
    assert_eq!(failed["failed"], "gate", "{failed}");
    assert_eq!(gate_runs(&world), 1);
    let why = failed["why"].as_str().unwrap();
    assert!(why.contains("cannot classify on an overloaded host, gated again 0 times"), "{why}");
}
