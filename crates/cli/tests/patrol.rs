//! Patrols (docs/adr/0008): a role run on a schedule and for no issue. A tick
//! runs the patrols its repo names that are due, one host of several; a run
//! takes a slot, declares nothing, and has failed when it left a commit or a
//! PR.

mod common;

use std::path::PathBuf;

use common::{World, commit, git, write};
use serde_json::{Value, json};

const TEAM: &str = "```yaml\nroles:\n  dev: acme/dev\n```\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: done }
```
";
const SWEEP: &str = "```yaml\nby: dev\ndo: sweep\nevery: 1h\nmax_open: 2\n```\n";
const AUDIT: &str = "```yaml\nby: dev\ndo: sweep\nevery: 2d\nmodel: opus\nmax_open: 1\n```\n";

/// A world whose repo is bound to a team with the patrols `sweep` and
/// `audit`, and names `patrols` of them.
fn bound(patrols: &str) -> World {
    let world = World::new();
    let skills = [
        (".claude/skills/implement/SKILL.md", "build\n"),
        (".claude/skills/sweep/SKILL.md", "s\n"),
    ];
    world.repo("acme/dev", &skills);
    world.repo(
        "acme/team",
        &[
            ("team.md", TEAM),
            ("workflows/feature.md", FEATURE),
            ("patrols/sweep.md", SWEEP),
            ("patrols/audit.md", AUDIT),
        ],
    );
    let config = format!(
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nconcurrency: 2\nteam: \
         acme/team\npatrols: {patrols}\n```\n"
    );
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

fn tick_as(world: &World, instance: &str, now: u64) -> Value {
    let now = now.to_string();
    world.afk(&["tick", "--instance", instance, "--host", "h1", "--now", &now]).ok()
}

fn tick(world: &World, now: u64) -> Value {
    tick_as(world, "alpha", now)
}

/// What a tick did about the patrols.
fn patrolled(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    steps
        .filter(|step| !step["patrol"].is_null())
        .map(|step| {
            assert!(step["issue"].is_null(), "{step}");
            let said = [&step["do"], &step["patrol"], &step["outcome"]];
            said.map(|word| word.as_str().unwrap()).join(" ")
        })
        .collect()
}

/// How `plan` says the patrols stand, for `instance` at `now`.
fn planned(world: &World, instance: &str, now: u64) -> Value {
    world.afk(&["plan", "--instance", instance, "--now", &now.to_string()]).ok()
}

fn standing(world: &World, now: u64) -> Vec<String> {
    let plan = planned(world, "alpha", now);
    let rows = plan["patrols"].as_array().expect("a plan lists the patrols").iter();
    rows.map(|row| {
        format!("{} {}", row["name"].as_str().unwrap(), row["standing"].as_str().unwrap())
    })
    .collect()
}

/// The fleet's refs the remote has for patrols.
fn refs(world: &World) -> Vec<String> {
    let listed = git(&world.bare, &["for-each-ref", "--format=%(refname)"]);
    listed.lines().filter(|name| name.contains("patrol")).map(str::to_string).collect()
}

/// The checkout of the patrol `sweep`.
fn checkout(world: &World) -> PathBuf {
    world.root.join("wt/convoy-patrol-sweep")
}

/// The worker in every terminal is gone.
fn workers_leave(world: &World) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["open"] = json!(false);
        }
    });
}

/// The worker in every terminal reports that it is at work at `now`.
fn workers_work(world: &World, now: u64) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["agent"] = json!({"state": "working", "stateStartedAt": now * 1000});
            terminal["lastOutputAt"] = json!(now * 1000);
        }
    });
}

fn calls(world: &World, what: &str) -> Vec<Vec<String>> {
    world.orca_calls().into_iter().filter(|call| call[..2].join(" ") == what).collect()
}

fn option<'a>(call: &'a [String], name: &str) -> Option<&'a str> {
    let at = call.iter().position(|arg| arg == name)?;
    call.get(at + 1).map(String::as_str)
}

#[test]
fn check_reads_patrol_files_and_holds_the_repos_patrols_against_them() {
    let world = bound("[sweep]");
    let read = world.afk(&["check"]).ok();
    assert_eq!(
        read["patrols"],
        json!([
            {"name": "audit", "by": "dev", "do": "sweep", "every": 172_800, "model": "opus",
                "max_open": 1, "runs_here": false},
            {"name": "sweep", "by": "dev", "do": "sweep", "every": 3600, "model": null,
                "max_open": 2, "runs_here": true},
        ])
    );
    let files: Vec<&str> =
        read["files"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(
        files.contains(&"patrols/sweep.md") && files.contains(&"patrols/audit.md"),
        "{files:?}"
    );

    // A repo names patrols its team has.
    let world = bound("[sweep, nightly]");
    let refused = world.afk(&["check"]);
    assert_ne!(refused.code, 0);
    let faults = refused.json["faults"].as_array().unwrap();
    assert_eq!(faults.len(), 1, "{faults:?}");
    assert_eq!(faults[0]["file"], "docs/agents/afkconvoy.md");
    assert_eq!(faults[0]["key"], "patrols");
    let problem = faults[0]["problem"].as_str().unwrap();
    assert!(
        problem.contains("`nightly` is no patrol") && problem.contains("audit, sweep"),
        "{problem}"
    );
    // …and a fleet does not run on such a repo.
    let ticked = world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", "10000"]);
    assert!(ticked.error().contains("`nightly` is no patrol"));
}

#[test]
fn check_reports_every_fault_of_a_patrol_file() {
    let world = World::new();
    world.repo("acme/dev", &[(".claude/skills/sweep/SKILL.md", "s\n")]);
    let dir = world.repo(
        "acme/team",
        &[
            ("team.md", TEAM),
            ("patrols/soon.md", "```yaml\nby: dev\ndo: sweep\nevery: soon\nmax_open: 1\n```\n"),
            ("patrols/bare.md", "```yaml\nby: ops\ndo: sweep\nwhen: 1h\n```\n"),
            ("patrols/Loud_One.md", SWEEP),
            ("patrols/never.md", "```yaml\nby: dev\ndo: mop\nevery: 0h\nmax_open: 1\n```\n"),
        ],
    );
    let refused = world.afk(&["check", "--dir", dir.to_str().unwrap()]);
    assert_ne!(refused.code, 0);
    let faults: Vec<String> = refused.json["faults"]
        .as_array()
        .unwrap()
        .iter()
        .map(|fault| {
            format!("{} {}", fault["file"].as_str().unwrap(), fault["key"].as_str().unwrap())
        })
        .collect();
    for wanted in [
        "patrols/soon.md every",
        "patrols/never.md every",
        "patrols/never.md do",
        "patrols/bare.md when",
        "patrols/bare.md by",
        "patrols/bare.md every",
        "patrols/bare.md max_open",
        "patrols/Loud_One.md ",
    ] {
        assert!(faults.iter().any(|fault| fault == wanted), "no fault {wanted:?} among {faults:?}");
    }
}

#[test]
fn a_patrol_the_repo_names_runs_when_due_and_one_it_does_not_name_never_runs() {
    let world = bound("[sweep]");
    assert_eq!(standing(&world, 10_000), ["sweep due"]);
    let before = planned(&world, "alpha", 10_000);
    assert_eq!(
        before["patrols"],
        json!([{"name": "sweep", "standing": "due", "every": 3600, "last_run": null, "open": 0,
            "max_open": 2}])
    );
    assert_eq!(before["free_slots"], 2);
    let base = git(&world.clone, &["rev-parse", "origin/main"]);
    world.orca_calls();

    let ticked = tick(&world, 10_000);
    assert_eq!(patrolled(&ticked), ["patrol sweep done"]);
    assert_eq!(ticked["held"], 1, "a run takes a slot");

    // Its worker is in a checkout of the base branch that is no issue's…
    let calls = world.orca_calls();
    let of = |what: &str| -> Vec<&Vec<String>> {
        calls.iter().filter(|call| call[..2].join(" ") == what).collect()
    };
    let created = of("worktree create");
    assert_eq!(created.len(), 1, "{created:?}");
    assert_eq!(option(created[0], "--name"), Some("convoy-patrol-sweep"));
    assert_eq!(option(created[0], "--base-branch"), Some(base.as_str()));
    assert_eq!(option(created[0], "--issue"), None);
    assert_eq!(git(&checkout(&world), &["rev-parse", "HEAD"]), base);
    // …started in its role, with the patrol in its environment and no issue…
    let started = of("terminal create");
    let command = option(started[0], "--command").unwrap();
    assert!(command.contains(" AFKCONVOY_PATROL='sweep' AFKCONVOY_ROLE='dev' "), "{command}");
    assert!(command.contains(" AFKCONVOY_SKILL='sweep' AFKCONVOY_WRITES='false' "), "{command}");
    assert!(!command.contains("AFKCONVOY_ISSUE") && !command.contains("AFKCONVOY_STAGE"));
    assert!(command.contains(&format!("cd '{}", world.cache().display())), "{command}");
    // …and handed a patrol's brief.
    let brief = git(&checkout(&world), &["rev-parse", "--absolute-git-dir"]);
    let brief = std::fs::read_to_string(PathBuf::from(brief).join("convoy-worker-prompt.md"));
    let brief = brief.unwrap();
    assert!(brief.contains("**Patrol:** `sweep`") && brief.contains("`feature`"), "{brief}");
    assert!(brief.contains("intake --issue <number> --workflow <name>"), "{brief}");
    assert!(brief.contains("You may have 2 more such issue(s) open"), "{brief}");
    assert_eq!(of("terminal send").len(), 1);

    // The run is claimed and recorded on the remote, with the fleet's other refs.
    let kept = refs(&world);
    assert_eq!(kept, ["refs/convoy/claim/patrol-sweep", "refs/convoy/patrol/sweep"]);
    let after = planned(&world, "alpha", 10_001);
    assert_eq!(after["patrols"][0]["standing"], "running");
    assert_eq!(after["patrols"][0]["last_run"], 10_000);
    assert_eq!(after["free_slots"], 1);
    // The claim on a run is no claim on an issue.
    assert_eq!(world.afk(&["scan"]).ok()["claims"], json!([]));
}

#[test]
fn a_patrol_due_while_its_run_is_still_going_is_skipped_and_a_missed_run_is_not_made_up() {
    let world = bound("[sweep]");
    tick(&world, 10_000);
    world.land_elsewhere("later");
    let base = git(&world.bare, &["rev-parse", "main"]);

    // Three intervals on, its worker still at it: nothing is started.
    workers_work(&world, 21_000);
    world.orca_calls();
    assert_eq!(patrolled(&tick(&world, 21_000)), ["end_patrol sweep nothing"]);
    assert!(calls(&world, "terminal create").is_empty());
    assert_eq!(standing(&world, 21_000), ["sweep running"]);

    // The worker stops: the tick that finds it gone ends the run…
    workers_leave(&world);
    let ended = tick(&world, 21_100);
    assert_eq!(patrolled(&ended), ["end_patrol sweep done"]);
    assert_eq!(ended["held"], 0);
    assert_eq!(refs(&world), ["refs/convoy/patrol/sweep"]);
    assert_eq!(world.gh()["issues"], json!([]), "a run that left nothing is no failure");
    // …and the next runs it once, in the same checkout, at the base branch as it is now.
    world.orca_calls();
    assert_eq!(patrolled(&tick(&world, 21_200)), ["patrol sweep done"]);
    let calls = world.orca_calls();
    assert!(!calls.iter().any(|call| call[..2].join(" ") == "worktree create"), "{calls:?}");
    assert_eq!(git(&checkout(&world), &["rev-parse", "HEAD"]), base);
    workers_leave(&world);
    assert_eq!(patrolled(&tick(&world, 21_300)), ["end_patrol sweep done"]);
    // Three were missed and one was run: it is not due again for an interval.
    assert_eq!(standing(&world, 24_799), ["sweep not_due"]);
    assert_eq!(patrolled(&tick(&world, 24_799)), Vec::<String>::new());
    assert_eq!(standing(&world, 24_800), ["sweep due"]);
}

#[test]
fn of_several_hosts_one_runs_a_patrol() {
    let world = bound("[sweep]");
    assert_eq!(patrolled(&tick_as(&world, "alpha", 10_000)), ["patrol sweep done"]);
    world.orca_calls();

    // A peer finds the run claimed, and leaves it alone…
    let peer = planned(&world, "beta", 10_010);
    assert_eq!(peer["patrols"][0]["standing"], "peer_running");
    assert_eq!(peer["free_slots"], 2, "a peer's run takes none of my slots");
    assert_eq!(patrolled(&tick_as(&world, "beta", 10_010)), Vec::<String>::new());
    // …and once it ended, reads on the remote that it ran.
    workers_leave(&world);
    assert_eq!(patrolled(&tick_as(&world, "alpha", 10_020)), ["end_patrol sweep done"]);
    assert_eq!(planned(&world, "beta", 10_030)["patrols"][0]["standing"], "not_due");
    assert_eq!(patrolled(&tick_as(&world, "beta", 10_030)), Vec::<String>::new());
    assert!(calls(&world, "terminal create").is_empty());

    // A run whose host died is unclaimed once its lease is over, by whoever finds it.
    assert_eq!(patrolled(&tick_as(&world, "alpha", 20_000)), ["patrol sweep done"]);
    let late = 20_000 + 3 * 3600;
    assert_eq!(planned(&world, "beta", late)["patrols"][0]["standing"], "stale_run");
    assert_eq!(patrolled(&tick_as(&world, "beta", late)), ["end_patrol sweep done"]);
    assert_eq!(refs(&world), ["refs/convoy/patrol/sweep"]);
    assert_eq!(patrolled(&tick_as(&world, "beta", late + 1)), ["patrol sweep done"]);
}

/// A worker running the patrol `sweep` takes `issue` in to `feature`.
fn take_in(world: &World, issue: u64) -> Value {
    let issue = issue.to_string();
    let line = ["intake", "--issue", &issue, "--workflow", "feature", "--role", "dev"];
    world.afk(&[&line[..], &["--patrol", "sweep"]].concat()).ok()
}

#[test]
fn an_intake_by_a_patrols_worker_is_on_record_as_the_patrols() {
    let world = bound("[sweep]");
    world.issue(12, &[]);
    let taken = take_in(&world, 12);
    assert_eq!(
        taken,
        json!({"issue": 12, "workflow": "feature", "taken": true, "role": "dev",
            "patrol": "sweep", "open": 1, "limit": 50, "woke": false})
    );
    assert_eq!(world.labels(12), ["flow:feature"]);
    let record = &world.comments(12)[0];
    assert!(record.starts_with("<!--convoy:intake workflow=feature role=dev patrol=sweep-->"));
    assert!(record.contains("on the patrol `sweep`"), "{record}");
    let shown = world.afk(&["show", "--issue", "12"]).ok();
    assert_eq!(shown["intake"]["by"], "worker");
    assert_eq!(shown["intake"]["patrol"], "sweep");
    assert!(shown["intake"].get("from").is_none());
    assert_eq!(take_in(&world, 12)["standing"]["patrol"], "sweep");

    // The patrol is one the team has, and a worker is on an issue or a patrol.
    world.issue(13, &[]);
    let line = ["intake", "--issue", "13", "--workflow", "feature", "--role", "dev"];
    let unknown = world.afk(&[&line[..], &["--patrol", "nightly"]].concat()).error();
    assert!(unknown.contains("has no patrol \"nightly\""), "{unknown}");
    let neither = world.afk(&line).error();
    assert!(neither.contains("--patrol") && neither.contains("AFKCONVOY_PATROL"), "{neither}");
    assert_eq!(world.labels(13), Vec::<String>::new());
}

#[test]
fn a_patrol_is_skipped_while_the_open_issues_it_opened_have_reached_its_max_open() {
    let world = bound("[sweep]");
    for issue in [12, 13] {
        // Blocked, so that no worker is dispatched to them here.
        world.issue(issue, &[]);
        take_in(&world, issue);
    }
    // One a human labelled, and one a stage's worker took in, are not the patrol's.
    world.issue(14, &["flow:feature"]);
    world.issue(15, &["flow:feature"]);
    world.comment(15, "<!--convoy:intake workflow=feature role=dev from=14-->\ntaken in");

    let plan = planned(&world, "alpha", 10_000);
    assert_eq!(plan["patrols"][0]["standing"], "full");
    assert_eq!(plan["patrols"][0]["open"], 2);
    assert_eq!(patrolled(&tick_as(&world, "gamma", 10_000)), Vec::<String>::new());
    assert_eq!(refs(&world), Vec::<String>::new());

    // One handed to a human is open still.
    world.with_gh(|gh| {
        for issue in gh["issues"].as_array_mut().unwrap() {
            if issue["number"] == 12 {
                issue["labels"] = json!(["ready-for-human"]);
            }
        }
    });
    assert_eq!(planned(&world, "alpha", 10_000)["patrols"][0]["open"], 2);
    // One closed makes room.
    world.close_issue(13);
    let plan = planned(&world, "alpha", 10_000);
    assert_eq!(plan["patrols"][0]["standing"], "due");
    assert_eq!(plan["patrols"][0]["open"], 1);
}

#[test]
fn no_patrol_runs_while_a_fleet_lock_stands() {
    let world = bound("[sweep, audit]");
    world.issue(30, &["fleet-lock"]);
    assert_eq!(standing(&world, 10_000), ["sweep locked", "audit locked"]);
    assert_eq!(patrolled(&tick(&world, 10_000)), Vec::<String>::new());
    assert_eq!(refs(&world), Vec::<String>::new());

    world.close_issue(30);
    assert_eq!(standing(&world, 10_100), ["sweep due", "audit due"]);
    let ticked = tick(&world, 10_100);
    assert_eq!(patrolled(&ticked), ["patrol sweep done", "patrol audit done"]);
    assert_eq!(ticked["held"], 2);
}

#[test]
fn a_patrol_takes_one_slot_of_concurrency() {
    let world = bound("[sweep, audit]");
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:feature"]);
    world.issue(9, &["flow:feature"]);

    // Two slots: the two patrols due take them, and no issue is dispatched.
    let ticked = tick(&world, 10_000);
    assert_eq!(patrolled(&ticked), ["patrol sweep done", "patrol audit done"]);
    assert_eq!(ticked["held"], 2);
    assert_eq!(ticked["frontier_remaining"], 3);
    assert_eq!(planned(&world, "alpha", 10_001)["free_slots"], 0);
    assert_eq!(world.claims(), vec![]);

    // A run that ended gives its slot back, to an issue.
    workers_leave(&world);
    let ended = tick(&world, 10_400);
    assert_eq!(patrolled(&ended), ["end_patrol sweep done", "end_patrol audit done"]);
    let claimed: Vec<u64> = world.claims().into_iter().map(|(issue, _)| issue).collect();
    assert_eq!(claimed, [7, 8]);
    // …and with every slot held by an issue, a patrol due waits.
    workers_work(&world, 13_700);
    assert_eq!(standing(&world, 13_700), ["sweep due", "audit not_due"]);
    assert_eq!(patrolled(&tick(&world, 13_700)), Vec::<String>::new());
}

/// The issue that reports a failed run, when there is one open.
fn report(world: &World) -> Option<Value> {
    let gh = world.gh();
    let rows = gh["issues"].as_array().unwrap();
    rows.iter()
        .find(|row| {
            row["title"] == "afkconvoy: the patrol `sweep` failed" && row["state"] == "open"
        })
        .cloned()
}

#[test]
fn a_patrol_that_leaves_a_commit_has_failed_and_is_reported_to_a_human() {
    let world = bound("[sweep]");
    world.issue(5, &["bug"]);
    tick(&world, 10_000);
    let started = git(&checkout(&world), &["rev-parse", "HEAD"]);
    let left = commit(&checkout(&world), "fix.txt", "a patrol writes no code\n");
    workers_leave(&world);

    assert_eq!(patrolled(&tick(&world, 10_400)), ["end_patrol sweep done"]);
    let failed = report(&world).expect("a failed run is reported");
    assert_eq!(failed["labels"], json!(["ready-for-human"]));
    let body = failed["body"].as_str().unwrap();
    assert!(body.starts_with("<!--convoy:patrol-failed patrol=sweep-->"), "{body}");
    assert!(body.contains(&format!("it left a commit in its checkout ({started} → {left})")));
    assert!(body.contains(checkout(&world).to_str().unwrap()) && body.contains("`h1`"), "{body}");
    // No issue is touched, and nothing is undone.
    assert_eq!(world.labels(5), ["bug"]);
    assert_eq!(world.comments(5), Vec::<String>::new());
    assert_eq!(git(&checkout(&world), &["rev-parse", "HEAD"]), left);
    assert_eq!(refs(&world), ["refs/convoy/patrol/sweep"]);

    // It is not run again while the report is open, however long that is…
    let number = failed["number"].as_u64().unwrap();
    let plan = planned(&world, "alpha", 90_000);
    assert_eq!(plan["patrols"][0]["standing"], "failed");
    assert_eq!(plan["patrols"][0]["failure"], number);
    assert_eq!(patrolled(&tick(&world, 90_000)), Vec::<String>::new());
    assert_eq!(world.gh()["issues"].as_array().unwrap().len(), 2, "one report, not one a tick");
    // …and is once a human closed it.
    world.close_issue(number);
    assert_eq!(patrolled(&tick(&world, 90_100)), ["patrol sweep done"]);
    assert_eq!(git(&checkout(&world), &["rev-parse", "HEAD"]), started);
}

#[test]
fn a_patrol_that_opens_a_pr_has_failed() {
    let world = bound("[sweep]");
    tick(&world, 10_000);
    world.pr(90, 5, "tester/convoy-patrol-sweep");
    world.pr(91, 6, "someone/else");
    workers_leave(&world);

    assert_eq!(patrolled(&tick(&world, 10_400)), ["end_patrol sweep done"]);
    let failed = report(&world).expect("a failed run is reported");
    let body = failed["body"].as_str().unwrap();
    assert!(body.contains("failed: it opened #90."), "{body}");
    assert_eq!(standing(&world, 20_000), ["sweep failed"]);
}

#[test]
fn the_line_of_a_tick_names_the_patrols_it_ran_and_ended() {
    let world = bound("[sweep]");
    let run = |now: &str, ticks: &str| {
        let args = ["run", "--host", "h1", "--second-ms", "0", "--ticks", ticks, "--now", now];
        world.afk_lines(&args).0
    };
    let lines = run("10000", "1");
    assert!(lines.iter().any(|line| line.contains("ran the patrol sweep")), "{lines:?}");
}
