//! Patrols (docs/adr/0008): a role run on a schedule and for no issue. A tick
//! runs the patrols its repo names that are due, one host of several; a run
//! takes a slot, declares nothing, and has failed when it left a commit not
//! on the base branch, working-tree changes, or a PR.

use crate::common;

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
const AUDIT: &str = "```yaml\nby: dev\ndo: sweep\nevery: 2d\nmax_open: 1\n```\n";

/// A world whose repo is bound to a team with the patrols `sweep` and
/// `audit`, and names `patrols` of them.
fn bound(patrols: &str) -> World {
    let began = std::time::Instant::now();
    let world = World::new();
    world.reuse_repository_templates();
    let skills = [
        (".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n"),
        (".claude/skills/sweep/SKILL.md", "---\nname: sweep\ndescription: s\n---\n"),
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
        "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n\
         patrols: {patrols}\n```\n"
    );
    write(&world.clone, "docs/agents/afkconvoy.md", &config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.list_checkout(&world.clone);
    common::fixture_timing("patrol bound setup", began);
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

/// The issues queries among `calls`, in order.
fn issue_queries(calls: &[Vec<String>]) -> Vec<String> {
    calls
        .iter()
        .filter_map(|call| {
            call.iter().find_map(|arg| {
                arg.strip_prefix("query=")
                    .filter(|query| query.contains("issues("))
                    .map(str::to_string)
            })
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

/// The fleet's refs the remote has for patrols: their runs, and the asks for one.
fn refs(world: &World) -> Vec<String> {
    let listed = git(&world.bare, &["for-each-ref", "--format=%(refname)"]);
    listed
        .lines()
        .filter(|name| name.contains("patrol") || name.contains("/asked/"))
        .map(str::to_string)
        .collect()
}

/// The checkout of the patrol `sweep`: the one its last run was started in.
fn checkout(world: &World) -> PathBuf {
    checkouts(world).pop().expect("the patrol has a checkout")
}

/// Every checkout orca lists for the patrol `sweep`, oldest first.
fn checkouts(world: &World) -> Vec<PathBuf> {
    let listed = world.worktrees().into_iter();
    listed.filter(|path| path.to_str().unwrap().contains("/convoy-patrol-sweep")).collect()
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
    world.orca_calls().into_iter().filter(|call| common::asked(call) == what).collect()
}

fn option<'a>(call: &'a [String], name: &str) -> Option<&'a str> {
    let at = call.iter().position(|arg| arg == name)?;
    call.get(at + 1).map(String::as_str)
}

/// #119: the first tick of a fresh bound repo held a slot for each patrol it
/// could not run. The startup probe refuses before any is taken.
#[test]
fn in_a_checkout_orca_does_not_know_no_patrol_is_run_and_no_slot_held() {
    let world = bound("[sweep, audit]");
    world.issue(7, &["feature"]);
    world.with_orca(|orca| orca["repos"] = json!([]));
    let now = "1000";
    for start in [
        world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", now]),
        world.afk(&["plan", "--now", now]),
    ] {
        let error = start.error();
        assert!(
            error.contains("orca does not know this checkout: orca repo add --path '"),
            "{error}"
        );
    }
    assert_eq!(refs(&world), [] as [String; 0], "no patrol's run");
    assert_eq!(world.claims(), [], "and no claim");
}

#[test]
fn check_reads_patrol_files_and_holds_the_repos_patrols_against_them() {
    let world = bound("[sweep]");
    let read = world.afk(&["check"]).ok();
    assert_eq!(
        read["patrols"],
        json!([
            {"name": "audit", "by": "dev", "do": "sweep", "every": 172_800,
                "max_open": 1, "max_runs_a_day": null, "max_landed_a_day": null,
                "runs_here": false},
            {"name": "sweep", "by": "dev", "do": "sweep", "every": 3600,
                "max_open": 2, "max_runs_a_day": null, "max_landed_a_day": null,
                "runs_here": true},
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
    world.repo(
        "acme/dev",
        &[(".claude/skills/sweep/SKILL.md", "---\nname: sweep\ndescription: s\n---\n")],
    );
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
    let base = git(&world.clone, &["rev-parse", "origin/main"]);
    world.orca_calls();

    let ticked = tick(&world, 10_000);
    assert_eq!(patrolled(&ticked), ["patrol sweep done"]);
    assert_eq!(ticked["held"], 1, "a run takes a slot");

    // Its worker is in a checkout of the base branch that is no issue's…
    let calls = world.orca_calls();
    let of = |what: &str| -> Vec<&Vec<String>> {
        calls.iter().filter(|call| common::asked(call) == what).collect()
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
    assert!(!brief.contains("Team rules"), "the team has none: {brief}");
    assert_eq!(of("terminal send").len(), 1);

    // The run is claimed and recorded on the remote, with the fleet's other refs.
    let kept = refs(&world);
    assert_eq!(kept, ["refs/convoy/claim/patrol-sweep", "refs/convoy/patrol/sweep"]);
    let after = planned(&world, "alpha", 10_001);
    assert_eq!(after["patrols"][0]["standing"], "running");
    assert_eq!(after["patrols"][0]["last_run"], 10_000);
    // The claim on a run is no claim on an issue.
    assert_eq!(world.afk(&["scan"]).ok()["claims"], json!([]));
}

/// A patrol's worker starts as a stage's does: in its role's directory alone,
/// where the role is a subdirectory of its repo (docs/adr/0014).
#[test]
fn a_patrol_by_a_role_in_a_subdirectory_starts_in_that_directory_alone() {
    let world = bound("[sweep]");
    let repo = world.root.join("github/acme/team");
    write(&repo, "CLAUDE.md", "the rules of the team\n");
    write(
        &repo,
        ".claude/skills/release/SKILL.md",
        "---\nname: release\ndescription: a human's\n---\n",
    );
    write(&repo, "roles/dev/CLAUDE.md", "the rules of a dev\n");
    write(
        &repo,
        "roles/dev/.claude/skills/sweep/SKILL.md",
        "---\nname: sweep\ndescription: s\n---\n",
    );
    write(
        &repo,
        "roles/dev/.claude/skills/implement/SKILL.md",
        "---\nname: implement\ndescription: build\n---\n",
    );
    let head = commit(&repo, "team.md", "```yaml\nroles:\n  dev: ./roles/dev\n```\n");
    world.orca_calls();

    assert_eq!(patrolled(&tick(&world, 10_000)), ["patrol sweep done"]);
    let started = calls(&world, "terminal create");
    let command = common::unniced(option(&started[0], "--command").unwrap());
    let kept = world.cache().join("afkconvoy/copies/acme/team");
    let (role_dir, team_dir) = (
        kept.join(format!("{head}-role+claude-code-roles%2Fdev")),
        kept.join(format!("{head}-team+rules-")),
    );
    assert!(command.starts_with(&format!("cd '{}' && ", role_dir.display())), "{command}");
    assert!(command.contains(&format!(" --add-dir '{}' ", team_dir.display())), "{command}");
    let commits = format!(" AFKCONVOY_ROLE_COMMIT='{head}' AFKCONVOY_TEAM_COMMIT='{head}' ");
    assert!(command.contains(&commits), "{command}");
    assert!(role_dir.join("CLAUDE.md").is_file() && !role_dir.join("team.md").exists());
    assert!(team_dir.join("patrols/sweep.md").is_file());
    assert!(!team_dir.join("roles/dev").exists() && !team_dir.join(".claude").exists());
    // The team rules are a patrol's worker's too: in the copy added to it,
    // and named in its brief.
    let rules = team_dir.join("CLAUDE.md");
    assert_eq!(std::fs::read_to_string(&rules).unwrap(), "the rules of the team\n");
    assert!(command.contains(" export CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1 "));
    let brief = git(&checkout(&world), &["rev-parse", "--absolute-git-dir"]);
    let brief = std::fs::read_to_string(PathBuf::from(brief).join("convoy-worker-prompt.md"));
    let brief = brief.unwrap();
    assert!(brief.contains(&format!("- **Team rules:** `{}`\n", rules.display())), "{brief}");
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
    // …its checkout with it: nothing will use it again…
    assert_eq!(checkouts(&world), Vec::<PathBuf>::new());
    assert_eq!(world.open_terminals(), Vec::<String>::new());
    // …and the next runs it once, in a new one, at the base branch as it is now.
    world.orca_calls();
    assert_eq!(patrolled(&tick(&world, 21_200)), ["patrol sweep done"]);
    let calls = world.orca_calls();
    let created = calls.iter().filter(|call| common::asked(call) == "worktree create");
    assert_eq!(created.count(), 1, "{calls:?}");
    assert_eq!(checkouts(&world).len(), 1);
    assert_eq!(git(&checkout(&world), &["rev-parse", "HEAD"]), base);
    assert_eq!(world.open_terminals().len(), 1, "its worker's, and no other");
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
            "patrol": "sweep", "blocked_by": [], "open": 1, "limit": 50, "woke": false})
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
fn a_patrol_starts_beside_the_frontier_with_no_bound_between_them() {
    let world = bound("[sweep, audit]");
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:feature"]);
    let ticked = tick(&world, 10_000);
    assert_eq!(patrolled(&ticked), ["patrol sweep done", "patrol audit done"]);
    assert_eq!(ticked["held"], 4);
    assert_eq!(ticked["frontier_remaining"], 0);
    let claimed: Vec<u64> = world.claims().into_iter().map(|(issue, _)| issue).collect();
    assert_eq!(claimed, [7, 8]);
}

/// The line the brief of the patrol `sweep` ends with: what declares its run over.
fn over_line(world: &World) -> String {
    let git_dir = git(&checkout(world), &["rev-parse", "--absolute-git-dir"]);
    let brief = PathBuf::from(git_dir).join("convoy-worker-prompt.md");
    let brief = std::fs::read_to_string(brief).unwrap();
    let fence = brief.trim_end().strip_suffix("\n```").expect("the brief ends with a line to run");
    fence.rsplit_once("```bash\n").unwrap().1.to_string()
}

#[test]
fn a_patrol_s_worker_declares_its_run_over_and_the_next_tick_ends_it() {
    let world = bound("[sweep]");
    world.issue(7, &["flow:feature"]);
    world.issue(8, &["flow:feature"]);
    tick(&world, 10_000);
    assert_eq!(world.claims().len(), 2, "the run starts beside both issues");

    // The brief ends with the line, the run named in it.
    let line = over_line(&world);
    assert!(line.ends_with(" over --run 'patrol-sweep'"), "{line}");
    assert!(line.starts_with(&format!("cd '{}' && ", checkout(&world).display())), "{line}");

    // A worker that has not run it is still at work…
    workers_work(&world, 10_010);
    assert_eq!(patrolled(&tick(&world, 10_010)), ["end_patrol sweep nothing"]);
    // …and once it has, the next tick ends the run, a minute into it.
    let patrol = checkout(&world);
    let pointer = std::fs::read_to_string(patrol.join(".git")).unwrap();
    let git_dir = pointer.trim().strip_prefix("gitdir:").unwrap().trim();
    let brief = patrol.join(git_dir).join("convoy-worker-prompt.md");
    let pointer = afkconvoy_core::prompt::brief_pointer(&brief.display().to_string());
    let harness = afkconvoy_core::harness::Harness::ClaudeCode;
    let session = &harness.session_files(&patrol.display().to_string(), true, 10_000)[0];
    let root = world.home().join(harness.session_place(&patrol.display().to_string()).under_home);
    let file = root.join(&session.path).with_file_name("patrol-usage.jsonl");
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    let mut text = session.body.replace("\"brief\"", &serde_json::to_string(&pointer).unwrap());
    text.push_str(&format!(
        "{}\n",
        json!({"timestamp": 10_000, "type": "assistant", "cwd": patrol,
            "isSidechain": false, "message": {"id": "patrol", "role": "assistant",
            "model": "fixture", "usage": {"input_tokens": 7, "cache_read_input_tokens": 2,
            "cache_creation_input_tokens": 1, "output_tokens": 3}}})
    ));
    assert!(text.contains(&patrol.join(git_dir).display().to_string()), "{text}");
    std::fs::write(&file, text).unwrap();
    let said = world.afk_in(&checkout(&world), &["over", "--run", "patrol-sweep"]).ok();
    assert_eq!(said["over"], true, "{said}");
    assert_eq!(said["ref"], "refs/convoy/over/patrol-sweep");
    assert_eq!(said["woke"], false, "no `run` listens here: {said}");
    let ended = tick(&world, 10_060);
    assert_eq!(patrolled(&ended), ["end_patrol sweep done"]);
    assert_eq!(world.claims().len(), 2);
    assert_eq!(refs(&world), ["refs/convoy/patrol/sweep"], "nothing of the run is left");
    assert_eq!(world.gh()["issues"].as_array().unwrap().len(), 2, "no failure is reported");
    // Its worker is ended with it, whatever it was doing: the checkout is
    // gone, and the terminals left are those of the two issues' workers.
    assert_eq!(checkouts(&world), Vec::<PathBuf>::new());
    assert_eq!(world.open_terminals().len(), 2);
    let accounts = world.afk(&["accounts", "--all", "--now", "10060"]).ok();
    assert_eq!(accounts["patrol_runs"]["count"], 1);
    assert_eq!(accounts["patrol_runs"]["totals"]["input"], 7);
    assert_eq!(accounts["patrol_runs"]["top"][0]["conditions"], json!(["patrol:sweep"]));

    // With no run going there is nothing to declare over, and what was
    // declared of one run says nothing of the next.
    let refused = world.afk(&["over", "--run", "patrol-sweep"]).error();
    assert!(refused.contains("no run \"patrol-sweep\" is going"), "{refused}");
}

#[test]
fn a_run_declared_over_is_still_checked_for_what_a_patrol_never_leaves() {
    let world = bound("[sweep]");
    tick(&world, 10_000);
    commit(&checkout(&world), "fix.txt", "a patrol that fixed it itself");
    world.afk_in(&checkout(&world), &["over", "--run", "patrol-sweep"]).ok();
    assert_eq!(patrolled(&tick(&world, 10_060)), ["end_patrol sweep done"]);
    let report = report(&world).expect("the failed run is reported");
    let body = report["body"].as_str().unwrap();
    assert!(body.contains("it left a commit"), "{report}");
    // The checkout it names is kept for the human, and the worker that sat at
    // its prompt there is ended.
    let kept = checkout(&world);
    assert!(kept.is_dir() && body.contains(kept.to_str().unwrap()), "{report}");
    assert_eq!(world.open_terminals(), Vec::<String>::new());
}

#[test]
fn a_worker_on_an_issue_declares_no_run_over() {
    let world = bound("[sweep]");
    world.issue(7, &["flow:feature"]);
    tick(&world, 10_000);
    // Its stage ends with a verdict: neither its claim nor a run's is its to end.
    for run in ["7", "patrol-sweep"] {
        let mut on_issue = world.command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone);
        let out = on_issue.args(["over", "--run", run]).env("AFKCONVOY_ISSUE", "7").output();
        let said = String::from_utf8(out.unwrap().stdout).unwrap();
        assert!(said.contains("\"error\"") && said.contains("`afkconvoy verdict`"), "{said}");
    }
    let an_issue = world.afk(&["over", "--run", "7"]).error();
    assert!(an_issue.contains("7 is an issue, and no run"), "{an_issue}");
    workers_work(&world, 10_010);
    assert_eq!(patrolled(&tick(&world, 10_010)), ["end_patrol sweep nothing"]);
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
fn a_patrol_moved_along_the_base_branch_leaves_no_commit() {
    for detached in [false, true] {
        for at_tip in [false, true] {
            let world = bound("[sweep]");
            tick(&world, 10_000);
            let patrol = checkout(&world);
            let started = git(&patrol, &["rev-parse", "HEAD"]);
            let repair = commit(&world.clone, "repair.txt", "a watch's repair\n");
            let later = commit(&world.clone, "later.txt", "more landed work\n");
            git(&world.clone, &["push", "-q", "origin", "main"]);
            // A remote-tracking ref is no evidence of what the base branch
            // holds now. The run must read the remote itself.
            git(&world.clone, &["update-ref", "refs/remotes/origin/main", &started]);
            let target = if at_tip { &later } else { &repair };
            if detached {
                git(&patrol, &["checkout", "--detach", target]);
            } else {
                git(&patrol, &["merge", "--ff-only", target]);
            }
            workers_leave(&world);

            assert_eq!(patrolled(&tick(&world, 10_400)), ["end_patrol sweep done"]);
            assert!(report(&world).is_none(), "a clean run on the base branch did not fail");
            assert!(!patrol.exists(), "the clean checkout is removed");
            assert_eq!(refs(&world), ["refs/convoy/patrol/sweep"], "the claim is released");
            assert_eq!(standing(&world, 13_600), ["sweep due"], "it can run again");
        }
    }
}

#[test]
fn a_patrol_commit_already_on_the_base_branch_is_not_left_behind() {
    let world = bound("[sweep]");
    tick(&world, 10_000);
    let patrol = checkout(&world);
    let landed = commit(&patrol, "repair.txt", "a repair that has since landed\n");
    git(&world.clone, &["merge", "--ff-only", &landed]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    workers_leave(&world);

    assert_eq!(patrolled(&tick(&world, 10_400)), ["end_patrol sweep done"]);
    assert!(report(&world).is_none());
    assert!(!patrol.exists());
}

#[test]
fn a_patrol_that_leaves_working_tree_changes_has_failed() {
    for change in ["unstaged", "staged", "deleted", "untracked"] {
        let world = bound("[sweep]");
        tick(&world, 10_000);
        let patrol = checkout(&world);
        let tracked = "docs/agents/afkconvoy.md";
        match change {
            "untracked" => write(&patrol, "unfinished.txt", "a change with no commit\n"),
            "deleted" => std::fs::remove_file(patrol.join(tracked)).unwrap(),
            _ => {
                let content = git(&patrol, &["show", &format!("HEAD:{tracked}")]);
                write(&patrol, tracked, &format!("{content}\n\n"));
                if change == "staged" {
                    git(&patrol, &["add", tracked]);
                }
            }
        }
        let status = git(&patrol, &["status", "--porcelain"]);
        workers_leave(&world);

        assert_eq!(patrolled(&tick(&world, 10_400)), ["end_patrol sweep done"]);
        let failed = report(&world).expect("working-tree changes fail the run");
        let body = failed["body"].as_str().unwrap();
        assert!(body.contains("it left working-tree changes"), "{change}: {body}");
        assert!(!body.contains("it left a commit"), "{change}: {body}");
        assert!(patrol.is_dir(), "the changes are kept for the human");
        assert_eq!(git(&patrol, &["status", "--porcelain"]), status);
        assert_eq!(refs(&world), ["refs/convoy/patrol/sweep"]);
    }
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
    assert!(body.contains(&format!("it left a commit not on the base branch ({left})")));
    let kept = checkout(&world);
    assert!(body.contains(kept.to_str().unwrap()) && body.contains("`h1`"), "{body}");
    // No issue is touched, and nothing is undone: the checkout the report
    // names is kept, with no worker in it.
    assert_eq!(world.labels(5), ["bug"]);
    assert_eq!(world.comments(5), Vec::<String>::new());
    assert_eq!(git(&kept, &["rev-parse", "HEAD"]), left);
    assert_eq!(refs(&world), ["refs/convoy/patrol/sweep"]);

    // It is not run again while the report is open, however long that is…
    let number = failed["number"].as_u64().unwrap();
    let plan = planned(&world, "alpha", 90_000);
    assert_eq!(plan["patrols"][0]["standing"], "failed");
    assert_eq!(plan["patrols"][0]["failure"], number);
    // The report waits for a human, and says what answers it.
    let waiting = &plan["waiting_on_human"];
    assert_eq!(waiting.as_array().unwrap().len(), 1, "{waiting}");
    assert_eq!(
        (&waiting[0]["issue"], &waiting[0]["why"], &waiting[0]["answer"]),
        (
            &json!(number),
            &json!(format!(
                "reports a failed run of the patrol `sweep`: it left a commit not on the base branch ({left})"
            )),
            &json!(format!("gh issue close {number}"))
        )
    );
    let comment = world.comments(number).pop().unwrap();
    assert_eq!(waiting[0]["url"], world.comment_url(number, &comment));
    world.comment(number, "The investigation continues.");
    let doctor = world.afk(&["doctor", "--now", "90000"]).json;
    assert_eq!(doctor["waiting_on_human"], *waiting);
    let excluded = plan["excluded"].as_array().unwrap();
    let reason = excluded.iter().find(|issue| issue["number"] == number).unwrap();
    assert_eq!(reason["reason"], "reports a failed run of the patrol `sweep`: waits to be closed");
    assert_eq!(patrolled(&tick(&world, 90_000)), Vec::<String>::new());
    assert_eq!(world.gh()["issues"].as_array().unwrap().len(), 2, "one report, not one a tick");
    // Reports from older versions have no reason comment: their body is
    // still evidence, read with every issue rather than one request each.
    world.with_gh(|gh| gh["comments"][number.to_string()] = json!([]));
    let old = planned(&world, "alpha", 90_000);
    let old = &old["waiting_on_human"][0];
    assert!(
        old["why"].as_str().unwrap().contains("it left a commit not on the base branch"),
        "{old}"
    );
    assert_eq!(old["url"], format!("https://gh/i/{number}"));
    // …and is once a human closed it, in a new checkout: the one that was
    // kept for them goes then.
    world.close_issue(number);
    assert_eq!(patrolled(&tick(&world, 90_100)), ["patrol sweep done"]);
    assert_eq!(checkouts(&world).len(), 1);
    assert!(!kept.exists() && checkout(&world) != kept, "{kept:?}");
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
        let args = [
            "run",
            "--foreground",
            "--host",
            "h1",
            "--second-ms",
            "0",
            "--ticks",
            ticks,
            "--now",
            now,
        ];
        world.afk_lines(&args).0
    };
    let lines = run("10000", "1");
    assert!(lines.iter().any(|line| line.contains("ran the patrol sweep")), "{lines:?}");
}

#[test]
fn the_tick_that_ends_a_patrol_starts_the_frontier_beside_it() {
    let world = bound("[sweep]");
    tick(&world, 10_000);
    workers_leave(&world);
    for issue in [5, 6, 7] {
        world.issue(issue, &["flow:feature"]);
    }
    // The frontier was not empty as the run ended: the same tick started its
    // workers, and left `run` nothing to skip its sleep for.
    let args = ["run", "--foreground", "--instance", "alpha", "--host", "h1", "--second-ms", "0"];
    let (lines, _) = world.afk_lines(&[&args[..], &["--ticks", "1", "--now", "10400"]].concat());
    assert_eq!(
        lines,
        [
            "tick 1: dispatched #5, #6, #7 · ended the patrol sweep · holding 3 · frontier 0 · next in 90s"
        ]
    );
}

/// Who holds the claim on the run of the patrol `sweep`.
fn runner(world: &World) -> String {
    let subject = git(&world.bare, &["log", "-1", "--format=%s", "refs/convoy/claim/patrol-sweep"]);
    let named = subject.split_once("instance=").expect("a claim names its instance").1;
    named.split([' ', '-']).next().unwrap().to_string()
}

/// #132: a `run` killed with a patrol's run claimed left the patrol
/// `peer_running` for the whole lease, however often the fleet was started
/// again on its host. A start takes the run with the host's other claims.
#[test]
fn a_start_takes_the_run_of_a_patrol_stamped_with_its_host_and_the_next_tick_ends_it() {
    let world = bound("[sweep]");
    let run = |instance: &str, host: &str, now: &str| {
        let args =
            ["run", "--foreground", "--instance", instance, "--host", host, "--second-ms", "0"];
        world.afk_lines(&[&args[..], &["--ticks", "1", "--now", now]].concat()).0
    };
    // The fleet that ran here claimed a run, and was killed.
    assert_eq!(patrolled(&tick_as(&world, "before", 10_000)), ["patrol sweep done"]);
    assert_eq!(planned(&world, "alpha", 10_010)["patrols"][0]["standing"], "peer_running");

    // A fleet that starts on another host leaves a live peer's run alone.
    let elsewhere = run("beta", "h2", "10010");
    assert!(!elsewhere.iter().any(|line| line.starts_with("start:")), "{elsewhere:?}");
    assert_eq!(runner(&world), "before");

    // One that starts here takes it, and its worker still at it, the run goes on as its own…
    workers_work(&world, 10_020);
    let lines = run("alpha", "h1", "10020");
    assert_eq!(
        lines[0],
        "start: took the run of the patrol sweep stamped with this host · swept 1 dead \
         heartbeat(s) that held nothing"
    );
    assert!(!lines[1].contains("the patrol"), "{}", lines[1]);
    assert_eq!(runner(&world), "alpha");
    assert_eq!(standing(&world, 10_030), ["sweep running"]);
    // …and is ended, as any run of its own, once that worker is gone.
    workers_leave(&world);
    assert_eq!(patrolled(&tick(&world, 10_060)), ["end_patrol sweep done"]);
    assert_eq!(standing(&world, 10_070), ["sweep not_due"]);
    assert_eq!(refs(&world), ["refs/convoy/patrol/sweep"]);
}

/// #132: `takeover` sees the claim on a run as it sees one on an issue.
#[test]
fn takeover_lists_and_takes_the_run_of_a_patrol_with_what_its_worker_declared() {
    let world = bound("[sweep]");
    assert_eq!(patrolled(&tick_as(&world, "before", 10_000)), ["patrol sweep done"]);
    // Its worker declared the run over, and the fleet died before it read that.
    world.afk_in(&checkout(&world), &["over", "--run", "patrol-sweep"]).ok();
    workers_work(&world, 10_010);

    let listed = world.afk(&["takeover", "--list", "--now", "10010"]).ok();
    let row = &listed["instances"][0];
    assert_eq!(
        (&row["instance"], &row["host"], &row["claims"], &row["runs"], &row["claim_count"]),
        (&json!("before"), &json!("h1"), &json!([]), &json!(["patrol-sweep"]), &json!(1))
    );

    let from = ["takeover", "--from", "before", "--instance", "beta", "--host", "h2"];
    let asked = world.afk(&[&from[..], &["--now", "10010"]].concat()).ok();
    assert_eq!((&asked["action"], &asked["taken_runs"]), (&json!("confirm"), &json!([])));
    let taken = world.afk(&[&from[..], &["--now", "10010", "--yes"]].concat()).ok();
    assert_eq!(taken["action"], "taken");
    assert_eq!((&taken["taken"], &taken["taken_runs"]), (&json!([]), &json!(["patrol-sweep"])));
    assert_eq!(taken["lost_runs"], json!([]));
    assert!(taken["detail"].as_str().unwrap().starts_with("took 1 claim(s)"), "{taken}");
    assert_eq!(runner(&world), "beta");

    // The run is over under the claim that took it: the next tick ends it.
    assert_eq!(patrolled(&tick_as(&world, "beta", 10_020)), ["end_patrol sweep done"]);
    assert_eq!(refs(&world), ["refs/convoy/patrol/sweep"], "nothing of the run is left");
}

/// #303: `max_runs_a_day` is a whole number from 1, fewer than `every` lets
/// a day hold.
#[test]
fn check_holds_max_runs_a_day_to_a_count_its_interval_does_not_already_keep() {
    let world = World::new();
    world.repo(
        "acme/dev",
        &[(".claude/skills/sweep/SKILL.md", "---\nname: sweep\ndescription: s\n---\n")],
    );
    let file = |every: &str, max: &str| {
        format!(
            "```yaml\nby: dev\ndo: sweep\nevery: {every}\nmax_open: 1\nmax_runs_a_day: {max}\n```\n"
        )
    };
    let (zero, word, fraction) = (file("1h", "0"), file("1h", "five"), file("1h", "1.5"));
    let (hourly, daily, fine) = (file("1h", "24"), file("1d", "1"), file("1h", "23"));
    let dir = world.repo(
        "acme/team",
        &[
            ("team.md", TEAM),
            ("patrols/zero.md", &zero),
            ("patrols/word.md", &word),
            ("patrols/fraction.md", &fraction),
            ("patrols/hourly.md", &hourly),
            ("patrols/daily.md", &daily),
            ("patrols/fine.md", &fine),
        ],
    );
    let refused = world.afk(&["check", "--dir", dir.to_str().unwrap()]);
    assert_ne!(refused.code, 0);
    let faults: Vec<(String, String, String)> = refused.json["faults"]
        .as_array()
        .unwrap()
        .iter()
        .map(|fault| {
            let said = |key: &str| fault[key].as_str().unwrap().to_string();
            (said("file"), said("key"), said("problem"))
        })
        .collect();
    let problem = |file: &str| -> &str {
        let found = faults.iter().find(|(at, key, _)| at == file && key == "max_runs_a_day");
        &found.unwrap_or_else(|| panic!("no fault of {file} among {faults:?}")).2
    };
    assert!(problem("patrols/zero.md").contains("from 1 up, not a number below 1"));
    assert!(problem("patrols/word.md").contains("from 1 up, not "));
    assert!(problem("patrols/fraction.md").contains("not a number with a fraction"));
    let hourly = problem("patrols/hourly.md");
    assert!(hourly.contains("24 bounds nothing") && hourly.contains("at most 24 time(s) a day"));
    assert!(problem("patrols/daily.md").contains("at most 1 time(s) a day"));
    assert!(faults.iter().all(|(file, _, _)| file != "patrols/fine.md"), "{faults:?}");
    assert_eq!(faults.len(), 5, "{faults:?}");
}

/// #303: a patrol that ran `max_runs_a_day` times in the last 24 hours is not
/// due, whatever `every` says, until the oldest of those runs is a day old —
/// and `plan` and the panel say until when.
#[test]
fn a_patrol_that_ran_its_max_runs_a_day_waits_until_the_oldest_is_a_day_old() {
    let world = bound("[sweep]");
    let team = world.root.join("github/acme/team");
    let bounded = "```yaml\nby: dev\ndo: sweep\nevery: 1h\nmax_open: 2\nmax_runs_a_day: 2\n```\n";
    commit(&team, "patrols/sweep.md", bounded);
    let check = world.afk(&["check"]).ok();
    assert_eq!(check["patrols"][1]["max_runs_a_day"], 2, "{check}");

    let run = |at: u64| {
        assert_eq!(patrolled(&tick(&world, at)), ["patrol sweep done"], "at {at}");
        workers_leave(&world);
        assert_eq!(patrolled(&tick(&world, at + 100)), ["end_patrol sweep done"]);
    };
    run(10_000);
    let once = planned(&world, "alpha", 13_600);
    assert_eq!(once["patrols"][0]["standing"], "due");
    assert_eq!(once["patrols"][0]["runs_in_day"], 1);
    run(13_600);

    // An interval later the patrol would be due, but the day holds two runs.
    let held = planned(&world, "alpha", 17_200);
    assert_eq!(
        held["patrols"],
        json!([{"name": "sweep", "standing": "spent", "every": 3600, "last_run": 13_600,
            "open": 0, "max_open": 2, "max_runs_a_day": 2, "runs_in_day": 2,
            "until": 10_000 + 86_400}])
    );
    assert_eq!(patrolled(&tick(&world, 17_200)), Vec::<String>::new());
    let page = world.render_panel(
        "patrol-spent",
        &["--instance", "alpha", "--host", "h1", "--now", "17200"],
        (1440, 1000),
    );
    assert!(
        page.html.contains(
            "held by max_runs_a_day: 2 runs in the last 24h, of 2 — until 02:46 UTC, in 22h 0m"
        ) && page.html.contains("data-patrol=\"sweep\" data-until=\"96400\""),
        "{}",
        page.html
    );
    assert_eq!(standing(&world, 96_399), ["sweep spent"]);
    assert_eq!(patrolled(&tick(&world, 96_399)), Vec::<String>::new());

    // The oldest is a day old: it runs, and the day holds the two last runs.
    assert_eq!(standing(&world, 96_400), ["sweep due"]);
    run(96_400);
    let after = planned(&world, "alpha", 100_000);
    assert_eq!(after["patrols"][0]["standing"], "due", "{after}");
    assert_eq!(after["patrols"][0]["runs_in_day"], 1, "13_600 is a day old at 100_000");
    let spent = planned(&world, "alpha", 99_999);
    assert_eq!(spent["patrols"][0]["standing"], "not_due", "the interval comes first");
}

/// A human asks for a run of the patrol `sweep` now, at `now`.
fn ask(world: &World, now: u64) -> common::Answer {
    world.afk(&["patrol-now", "sweep", "--now", &now.to_string()])
}

/// The subject of the record of the patrol `sweep`'s last run.
fn last_record(world: &World) -> String {
    git(&world.bare, &["log", "-1", "--format=%s", "refs/convoy/patrol/sweep"])
}

#[test]
fn a_human_asks_for_a_patrols_run_now_past_its_interval_and_its_day() {
    let world = bound("[sweep]");
    let team = world.root.join("github/acme/team");
    let bounded = "```yaml\nby: dev\ndo: sweep\nevery: 1h\nmax_open: 2\nmax_runs_a_day: 2\n```\n";
    commit(&team, "patrols/sweep.md", bounded);
    let run = |at: u64| {
        assert_eq!(patrolled(&tick(&world, at)), ["patrol sweep done"], "at {at}");
        workers_leave(&world);
        assert_eq!(patrolled(&tick(&world, at + 100)), ["end_patrol sweep done"]);
    };
    run(10_000);
    run(13_600);
    assert_eq!(standing(&world, 14_000), ["sweep not_due"]);
    assert_eq!(standing(&world, 17_200), ["sweep spent"]);
    assert!(!last_record(&world).contains("asked"), "a due run is no asked one");

    // Within the interval, and the day's runs at the bound: asked for, it is due…
    let asked = ask(&world, 14_000).ok();
    assert_eq!(
        asked,
        json!({"patrol": "sweep", "asked": 14_000, "ref": "refs/convoy/asked/sweep",
            "woke": false})
    );
    assert_eq!(git(&world.clone, &["status", "--porcelain"]), "", "no file changed");
    let plan = planned(&world, "alpha", 14_010);
    assert_eq!(plan["patrols"][0]["standing"], "due", "{plan}");
    assert_eq!(plan["patrols"][0]["asked"], 14_000);
    let page = world.render_panel(
        "patrol-asked",
        &["--instance", "alpha", "--host", "h1", "--now", "14010"],
        (1440, 1000),
    );
    assert!(page.html.contains("due: a human asked for it, and the next tick runs it"));

    // …the next tick runs it, records it as asked for, and the ask is answered.
    assert_eq!(patrolled(&tick(&world, 14_010)), ["patrol sweep done"]);
    assert_eq!(last_record(&world), "convoy:patrol-run ts=14010 before=10000,13600 asked=human");
    assert_eq!(refs(&world), ["refs/convoy/claim/patrol-sweep", "refs/convoy/patrol/sweep"]);
    let running = planned(&world, "alpha", 14_020);
    assert_eq!(running["patrols"][0]["standing"], "running");
    assert!(running["patrols"][0].get("asked").is_none(), "{running}");
    workers_leave(&world);
    assert_eq!(patrolled(&tick(&world, 14_100)), ["end_patrol sweep done"]);

    // The schedule goes on from it: `every` after its start, and it is one of the day's.
    assert_eq!(standing(&world, 17_609), ["sweep not_due"]);
    let after = planned(&world, "alpha", 17_610);
    assert_eq!(after["patrols"][0]["standing"], "spent", "{after}");
    assert_eq!(after["patrols"][0]["runs_in_day"], 3);
    assert_eq!(after["patrols"][0]["until"], 13_600 + 86_400);
    assert_eq!(patrolled(&tick(&world, 17_610)), Vec::<String>::new());
}

#[test]
fn an_ask_is_refused_with_why_while_what_holds_a_due_run_holds_it() {
    let world = bound("[sweep]");
    let asks = |world: &World| refs(world).into_iter().filter(|name| name.contains("asked"));

    world.issue(30, &["fleet-lock"]);
    let locked = ask(&world, 10_000);
    assert_eq!(locked.json["see"], "afkconvoy help patrols");
    let locked = locked.error();
    assert!(locked.contains("a fleet lock stands (#30)"), "{locked}");
    world.close_issue(30);

    // A run still going, mine or a peer's.
    assert_eq!(patrolled(&tick(&world, 10_000)), ["patrol sweep done"]);
    let going = ask(&world, 10_010).error();
    assert!(going.contains("a run of the patrol `sweep` is still going"), "{going}");
    workers_leave(&world);
    assert_eq!(patrolled(&tick(&world, 10_100)), ["end_patrol sweep done"]);

    for issue in [12, 13] {
        world.issue(issue, &[]);
        take_in(&world, issue);
    }
    let full = ask(&world, 10_200).error();
    assert!(full.contains("have reached its `max_open` (2 of 2)"), "{full}");

    let unknown = world.afk(&["patrol-now", "audit", "--now", "10200"]).error();
    assert!(unknown.contains("runs no patrol \"audit\": it runs sweep"), "{unknown}");
    assert_eq!(asks(&world).count(), 0, "nothing was recorded");

    // Room again: the ask is taken.
    world.close_issue(13);
    assert_eq!(ask(&world, 10_300).ok()["asked"], 10_300);
    assert_eq!(asks(&world).collect::<Vec<_>>(), ["refs/convoy/asked/sweep"]);
}

#[test]
fn of_two_hosts_asked_at_once_one_runs_the_patrol() {
    let world = bound("[sweep]");
    assert_eq!(patrolled(&tick_as(&world, "alpha", 10_000)), ["patrol sweep done"]);
    workers_leave(&world);
    assert_eq!(patrolled(&tick_as(&world, "alpha", 10_100)), ["end_patrol sweep done"]);

    // Both ask; one ask stands, and one run answers it.
    ask(&world, 10_200).ok();
    ask(&world, 10_201).ok();
    assert_eq!(standing(&world, 10_202), ["sweep due"]);
    assert_eq!(patrolled(&tick_as(&world, "alpha", 10_210)), ["patrol sweep done"]);
    assert_eq!(patrolled(&tick_as(&world, "beta", 10_211)), Vec::<String>::new());
    assert_eq!(planned(&world, "beta", 10_212)["patrols"][0]["standing"], "peer_running");
    workers_leave(&world);
    assert_eq!(patrolled(&tick_as(&world, "alpha", 10_300)), ["end_patrol sweep done"]);
    assert_eq!(standing(&world, 10_301), ["sweep not_due"], "the ask was answered");
}

#[test]
fn a_patrol_waits_for_its_completed_issues_to_leave_the_rolling_day() {
    let world = bound("[sweep]");
    commit(
        &world.root.join("github/acme/team"),
        "patrols/sweep.md",
        "```yaml\nby: dev\ndo: sweep\nevery: 1h\nmax_open: 2\nmax_landed_a_day: 2\n```\n",
    );
    assert_eq!(world.afk(&["check"]).ok()["patrols"][1]["max_landed_a_day"], 2);
    for (number, at, reason) in [
        (12, 10_000, "completed"),
        (13, 20_000, "completed"),
        (14, 30_000, "not_planned"),
        (15, 1, "completed"),
    ] {
        world.issue(number, &[]);
        take_in(&world, number);
        world.close_issue(number);
        world.with_gh(|gh| {
            let issue = gh["issues"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|issue| issue["number"] == number)
                .unwrap();
            issue["closed_at"] = json!(at);
            issue["state_reason"] = json!(reason);
            issue["labels"] = json!([]); // Ownership survives removing the flow label.
        });
    }
    // A human's completed issue and another patrol's do not count.
    for number in [16, 17] {
        world.issue(number, &[]);
        world.close_issue(number);
        world.with_gh(|gh| {
            let issue = gh["issues"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|issue| issue["number"] == number)
                .unwrap();
            issue["closed_at"] = json!(30_000);
            issue["state_reason"] = json!("completed");
        });
    }
    world.comment(17, "<!--convoy:intake workflow=feature role=dev patrol=audit-->");
    let _ = world.gh_calls();
    let held = planned(&world, "alpha", 90_000);
    let queries = issue_queries(&world.gh_calls());
    assert_eq!(queries, vec![afkconvoy::gh::OPEN_AND_LANDED_QUERY.to_string()], "{queries:?}");
    let patrol = &held["patrols"][0];
    assert_eq!(patrol["standing"], "landed", "{held}");
    assert_eq!(patrol["landed_in_day"], 2);
    assert_eq!(patrol["until"], 96_400);
    assert_eq!(planned(&world, "beta", 90_000)["patrols"], held["patrols"]);
    assert!(ask(&world, 90_000).error().contains("max_landed_a_day"));
    assert_eq!(patrolled(&tick(&world, 96_399)), Vec::<String>::new());
    let released = planned(&world, "alpha", 96_400);
    assert_eq!(released["patrols"][0]["standing"], "due");
    assert_eq!(released["patrols"][0]["landed_in_day"], 1);
    assert_eq!(ask(&world, 96_400).ok()["asked"], 96_400);
    assert_eq!(patrolled(&tick(&world, 96_400)), ["patrol sweep done"]);
}

#[test]
fn check_requires_a_positive_whole_landing_bound() {
    let world = bound("[sweep]");

    let mut files = vec![("team.md".to_string(), TEAM.to_string())];
    for (name, value) in [
        ("zero", "0"),
        ("negative", "-1"),
        ("fraction", "1.5"),
        ("word", "many"),
        ("boolean", "true"),
        ("null", "null"),
        ("fine", "1000"),
    ] {
        files.push((format!("patrols/{name}.md"),
            format!("```yaml\nby: dev\ndo: sweep\nevery: 1d\nmax_open: 1\nmax_landed_a_day: {value}\n```\n")));
    }
    let refs: Vec<(&str, &str)> = files.iter().map(|(a, b)| (a.as_str(), b.as_str())).collect();
    let dir = world.repo("acme/landing-bounds", &refs);
    let refused = world.afk(&["check", "--dir", dir.to_str().unwrap()]);
    assert_ne!(refused.code, 0);
    let faults = refused.json["faults"].as_array().unwrap();
    assert_eq!(faults.len(), 6, "{faults:?}");
    for fault in faults {
        assert_eq!(fault["key"], "max_landed_a_day");
        assert!(fault["problem"].as_str().unwrap().contains("whole number from 1 up"));
    }
}
