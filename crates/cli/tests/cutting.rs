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

/// The cut's PR lands: the train puts `widgets 0.1.1` on the base branch, and
/// the cut's record in `checkout` says so → the commit that landed it.
fn its_pr_lands(world: &World, checkout: &Path) -> String {
    land(world, "widgets 0.1.1");
    let landed = git(&world.clone, &["rev-parse", "HEAD"]).trim().to_string();
    let mut record = record(checkout);
    record["pr"] = json!(8);
    record["landed"] = json!(landed);
    std::fs::write(record_file(checkout), record.to_string()).unwrap();
    landed
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

/// The real cut command takes its own landing turn; no auxiliary worker runs.
fn finish_in_turn(world: &World, checkout: &Path) -> Value {
    let answer = world.afk_in(checkout, &["cut", "--host", "h1", "--second-ms", "10"]).ok();
    let landed = answer["landed"].as_str().unwrap();
    let pr_number = answer["pr"].as_u64().unwrap();
    world.with_gh(|gh| {
        let pr = gh["prs"].as_array().unwrap().iter().find(|pr| pr["number"] == pr_number).unwrap();
        assert_eq!(pr["state"], "closed");
        let comments = gh["comments"][pr_number.to_string()].as_array().unwrap();
        assert!(comments.iter().any(|c| c["body"].as_str().unwrap().contains(landed)
            && c["body"].as_str().unwrap().contains("v1.2.3")));
    });
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), landed);
    let tree = world.remote(&["rev-parse", &format!("{landed}^{{tree}}")]);
    assert!(
        !world
            .remote(&["for-each-ref", "--format=%(refname)", &format!("refs/convoy/gate/{tree}-*")])
            .is_empty()
    );
    assert!(world.remote(&["for-each-ref", "--format=%(refname)", "refs/convoy/train"]).is_empty());
    common::stopped_candidates_gone(world, checkout);
    answer
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
    let released = finish_in_turn(&world, &cut_checkout);
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
fn a_cut_adds_one_commit_and_leaves_the_set_commit_off_the_base() {
    let world = initial(false);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    // The base tip is certified while the gate still passes. A red file after
    // that fails the cut's own turn, and the turn returns, instead of waiting
    // on a base tip that was never green.
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true, "the base tip is gated first");
    std::fs::write(world.root.join("red"), "red").unwrap();
    let failed = world.afk_in(&checkout, &["cut", "--host", "h1", "--second-ms", "10"]).ok();
    assert_eq!(failed["failed"], "gate", "{failed}");
    let set = git(&checkout, &["rev-parse", "HEAD"]);
    std::fs::remove_file(world.root.join("red")).unwrap();
    land(&world, "Add a gadget (#9)");
    let tip = world.remote(&["rev-parse", BASE]);

    let answer = finish_in_turn(&world, &checkout);
    let landed = answer["landed"].as_str().unwrap();
    assert_eq!(world.remote(&["rev-parse", "v1.2.3^{commit}"]), landed);
    assert_eq!(parents(&world, landed), vec![tip], "one parent, the base tip at land time");
    assert!(!ancestor(&world, &set, landed), "the set commit stays off the base");
    assert_eq!(
        world.remote(&["diff", "--name-only", &format!("{landed}^"), landed]),
        "VERSION",
        "the diff against the parent is the version"
    );
    assert_eq!(world.remote(&["show", &format!("{landed}:VERSION")]), "1.2.3");
    assert!(
        world.remote(&["show", &format!("{landed}:widget.txt")]).contains("Add a gadget"),
        "a landing already on the tip stays"
    );
    let message = world.remote(&["log", "-1", "--format=%B", landed]);
    assert!(message.contains("By a cut on h1"), "{message}");
    assert!(message.contains("(#"), "{message}");
    assert!(message.lines().next().unwrap().ends_with(')'), "{message}");
}

#[test]
fn a_cut_that_landed_and_stopped_before_tagging_tags_that_commit() {
    let world = initial(false);
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    let answer = finish_in_turn(&world, &checkout);
    let landed = answer["landed"].as_str().unwrap().to_string();
    let pr = answer["pr"].clone();
    git(&checkout, &["tag", "-d", "v1.2.3"]);
    git(&world.bare, &["tag", "-d", "v1.2.3"]);
    let mut record = record(&checkout);
    record["landed"] = Value::Null;
    record["pr"] = Value::Null;
    record["took"] = json!([]);
    record["at"] = Value::Null;
    std::fs::write(record_file(&checkout), record.to_string()).unwrap();

    let again = world.afk_in(&checkout, &["cut", "--host", "h1", "--second-ms", "10"]).ok();
    assert_eq!(again["landed"], landed, "{again}");
    assert_eq!(again["pr"], pr, "{again}");
    assert_eq!(world.remote(&["rev-parse", "v1.2.3^{commit}"]), landed);
    assert_eq!(world.remote(&["rev-parse", BASE]), landed, "tagged the commit it had");
    assert_eq!(parents(&world, &landed).len(), 1);
    let message = world.remote(&["log", "-1", "--format=%B", &landed]);
    assert!(message.contains("By a cut on h1"), "{message}");
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
    assert_eq!(world.afk(&["gate"]).ok()["recorded"], true, "the cut starts on a green base");
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
    assert!(!screen.contains("[afkconvoy] cut: land started"), "{screen}");
    let failed_record = record(&checkout);
    let pr = failed_record["pr"].as_u64().unwrap();
    assert_ne!(git(&checkout, &["rev-parse", "HEAD"]), git(&world.clone, &["rev-parse", "HEAD"]));
    let same_tree = git(&checkout, &["rev-parse", "HEAD^{tree}"])
        == git(&world.clone, &["rev-parse", "HEAD^{tree}"]);
    assert_eq!(same_tree, preset, "a version already set still gets a commit");
    assert_eq!(cut(&tick_as(&world, "alpha", 10_100)), ["end_cut done"]);
    assert_eq!(standing(&world, "beta", 10_110), "failed");
    assert_eq!(cut(&tick_as(&world, "beta", 10_110)), Vec::<String>::new());
    std::fs::remove_file(world.root.join("red")).unwrap();

    let answer = finish_in_turn(&world, &checkout);
    assert_eq!(answer["tag"], "v1.2.3", "{answer}");
    assert_eq!(answer["pr"], pr);
    assert_eq!(answer["prs"], json!([7]), "the cut's own PR is excluded");
    assert_eq!(answer["closed"].as_array().unwrap().len(), 1);
    let steps: Vec<_> = answer["took"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| step["step"].as_str().unwrap())
        .collect();
    assert_eq!(steps, ["set", "pr", "gate", "land", "tag", "publish"]);
    let tagged = world.remote(&["rev-parse", "v1.2.3^{commit}"]);
    assert_eq!(answer["landed"], tagged);
    assert_eq!(world.remote(&["rev-parse", BASE]), tagged);
    assert_eq!(std::fs::read_to_string(world.root.join("published")).unwrap(), "1.2.3 v1.2.3\n");
    assert_eq!(world.gh()["prs"].as_array().unwrap().len(), 2, "one old PR and one cut PR");
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
fn an_initial_version_already_set_is_gated_landed_and_cut_then_the_next_minor() {
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
        .args(["cut", "--host", "h1", "--second-ms", "10"])
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
    assert_eq!(answer["took"].as_array().unwrap().len(), 6, "{answer}");
    let first = screen.lines().next().unwrap();
    assert!(first.contains("v0.1.1 (past v0.1.0, holding #7)"), "{first}");
    assert!(first.contains("steps: set, pr, gate, land, tag, publish"), "{first}");
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
fn a_cut_tags_the_commit_that_landed_it_and_publishes_with_the_tag_checked_out() {
    let world = cutting();
    assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
    let checkout = checkout(&world);
    let landed = its_pr_lands(&world, &checkout);

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
    assert_eq!(git(&world.bare, &["rev-parse", BASE]).trim(), landed, "pushed nothing to the base");

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
fn a_minor_landing_before_or_after_the_cuts_own_landing_supersedes_the_patch() {
    for minor_after_cut in [false, true] {
        let world = cutting();
        assert_eq!(cut(&tick_as(&world, "alpha", 10_000)), ["cut done"]);
        let checkout = checkout(&world);
        // A minor may land on either side of the cut's own landing.
        if minor_after_cut {
            its_pr_lands(&world, &checkout);
        }
        land(&world, "Add a gadget (#9)");
        world.with_gh(|gh| {
            let pr = json!({"number": 9, "title": "Add a gadget", "headRefName": "nine",
                "headRefOid": "", "body": "", "state": "merged", "labels": ["release:minor"]});
            gh["prs"].as_array_mut().unwrap().push(pr);
        });
        if !minor_after_cut {
            its_pr_lands(&world, &checkout);
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
        its_pr_lands(&world, &checkout);

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
