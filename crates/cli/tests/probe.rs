use crate::common;

use common::{Answer, World, commit, git};
use serde_json::{Value, json};

fn tick_at(world: &World, now: &str) -> Answer {
    world.afk(&[
        "tick",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--now",
        now,
        "--worker-command",
        "agent",
    ])
}

/// One tick of `run` → its last line, the answer.
fn run_once(world: &World) -> Answer {
    run_once_at(world, "1000")
}

fn run_once_at(world: &World, now: &str) -> Answer {
    let args = "run --instance alpha --host h1 --ticks 1 --second-ms 0";
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(args.split(' '))
        .args(["--now", now, "--worker-command", "agent"])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout.lines().last().unwrap_or_default();
    Answer {
        json: serde_json::from_str(last).unwrap_or_else(|e| panic!("{e}: {stdout}")),
        code: out.status.code().unwrap(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
    }
}

fn plan(world: &World) -> Answer {
    world.afk(&["plan", "--now", "1000"])
}

/// Every ref the remote has whose name starts with `prefix`.
fn refs(world: &World, prefix: &str) -> Vec<String> {
    let listed = world.remote(&["for-each-ref", "--format=%(refname)"]);
    listed.lines().filter(|name| name.starts_with(prefix)).map(str::to_string).collect()
}

fn warnings(answer: &Answer) -> Vec<&str> {
    answer.stderr.lines().filter(|line| line.starts_with("warning: ")).collect()
}

fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    steps
        .filter(|step| step["do"] != "board")
        .map(|step| {
            let issue = step["issue"].as_u64().map(|n| format!(" {n}")).unwrap_or_default();
            format!("{}{issue} {}", step["do"].as_str().unwrap(), step["outcome"].as_str().unwrap())
        })
        .collect()
}

#[test]
fn a_start_probes_the_remote_and_leaves_nothing_behind() {
    let world = World::new();
    world.watch_pushes(None);
    let ticked = tick_at(&world, "1000");
    assert_eq!(warnings(&ticked), [] as [&str; 0], "{}", ticked.stderr);
    ticked.ok();

    let probes = world.probes();
    assert_eq!(probes.len(), 4, "two probes, each pushed and deleted: {probes:?}");
    assert!(probes[0].starts_with("refs/convoy/probe-1000-"), "{probes:?}");
    assert!(probes[1].starts_with("refs/convoy/gate/probe-1000-"), "{probes:?}");
    assert_eq!(refs(&world, "refs/convoy"), [] as [String; 0]);
    assert_eq!(refs(&world, "refs/heads"), ["refs/heads/main"]);
}

#[test]
fn only_run_tick_and_plan_probe() {
    let world = World::new();
    world.issue(7, &[]);
    world.watch_pushes(None);
    world.afk(&["claim", "--issue", "7", "--instance", "alpha", "--now", "1000"]).ok();
    world.afk(&["scan"]).ok();
    world.afk(&["heartbeat", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(world.probes(), [] as [String; 0]);

    plan(&world).ok();
    assert_eq!(world.probes().len(), 4);
    run_once(&world).ok();
    assert_eq!(world.probes().len(), 8);
}

fn plan_at(world: &World, now: &str) -> Answer {
    world.afk(&["plan", "--now", now])
}

fn stands(answer: &Answer) -> bool {
    answer.stderr.lines().any(|line| line.starts_with("note: no startup probe was made"))
}

#[test]
fn plan_trusts_a_probe_that_passed_on_this_host_in_the_last_hour_and_says_so() {
    let world = World::new();
    world.protect(json!("Resource not accessible by integration (HTTP 403)"));
    world.watch_pushes(None);

    let first = plan_at(&world, "1000");
    assert!(!stands(&first), "{}", first.stderr);
    assert_eq!(warnings(&first).len(), 1, "{}", first.stderr);
    assert_eq!(world.probes().len(), 4);
    let asked = world.shell_asked().len();

    // within the hour: no probe, and what the probe warned of is said again
    let again = plan_at(&world, "4599");
    assert!(stands(&again), "{}", again.stderr);
    assert!(again.stderr.contains("59 min ago"), "{}", again.stderr);
    assert_eq!(warnings(&again), warnings(&first));
    assert_eq!(world.probes().len(), 4);
    let again_asked = world.shell_asked();
    assert_eq!(again_asked.len(), asked + 1, "only the current proxy exports are read again");
    assert!(again_asked[asked].starts_with("printf 'AFKCONVOY_PROXY"));
    let (mut first, mut again) = (first.ok(), again.ok());
    assert_eq!((first["now"].take(), again["now"].take()), (json!(1000), json!(4599)));
    assert_eq!(again, first, "and the answer is the one a probe leads to");

    // the hour is over
    let later = plan_at(&world, "4600");
    assert!(!stands(&later), "{}", later.stderr);
    assert_eq!(world.probes().len(), 8);
    // …and a probe from a clock ahead of this one is nobody's to trust
    assert!(!stands(&plan_at(&world, "4000")));
    assert_eq!(world.probes().len(), 12);
}

#[test]
fn a_probe_is_made_again_when_the_base_branch_or_the_remote_changed() {
    let world = World::new();
    world.watch_pushes(None);
    plan_at(&world, "1000").ok();
    assert!(stands(&plan_at(&world, "1001")));
    assert_eq!(world.probes().len(), 4);

    // the remote: the same repo, reached another way
    let elsewhere = format!("{}/.", world.bare.display());
    git(&world.clone, &["remote", "set-url", "origin", &elsewhere]);
    assert!(!stands(&plan_at(&world, "1002")));
    assert_eq!(world.probes().len(), 8);
    assert!(stands(&plan_at(&world, "1003")));

    // the base branch
    git(&world.clone, &["push", "-q", "origin", "main:release"]);
    let config = "```yaml\nbase_branch: release\ngate_command: ./gate.sh\n```\n";
    commit(&world.clone, "docs/agents/afkconvoy.md", config);
    assert!(!stands(&plan_at(&world, "1004")));
    assert_eq!(world.probes().len(), 12);
    assert!(stands(&plan_at(&world, "1005")));
}

#[test]
fn a_tick_trusts_only_a_probe_that_checked_its_worker_launch_command() {
    let world = World::new();
    world.watch_pushes(None);
    // `plan` starts no worker: its probe checked no command of a tick's
    plan_at(&world, "1000").ok();
    let ticked = tick_at(&world, "1001");
    assert!(!stands(&ticked), "{}", ticked.stderr);
    assert_eq!(world.probes().len(), 8);

    let again = tick_at(&world, "1002");
    assert!(stands(&again), "{}", again.stderr);
    again.ok();
    assert_eq!(world.probes().len(), 8);
    // …and a plan asks nothing of the command: a tick's probe stands for it
    assert!(stands(&plan_at(&world, "1003")));

    let other = ["tick", "--instance", "alpha", "--host", "h1", "--now", "1004"];
    let other = world.afk(&[&other[..], &["--worker-command", "other"]].concat());
    assert!(!stands(&other), "{}", other.stderr);
    assert_eq!(world.probes().len(), 12);

    // `run` probes at every start
    let run = run_once_at(&world, "1005");
    assert!(!stands(&run), "{}", run.stderr);
    assert_eq!(world.probes().len(), 16);
}

#[test]
fn plan_help_says_when_it_probes() {
    let world = World::new();
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["plan", "--help"])
        .output()
        .unwrap();
    let help = String::from_utf8_lossy(&out.stdout);
    for said in
        ["WHEN IT PROBES", "in the last\n  hour stands", "`base_branch`", "the remote's\n  URL"]
    {
        assert!(help.contains(said), "plan --help lost {said:?}:\n{help}");
    }
}

#[test]
fn a_remote_that_rejects_hidden_refs_has_every_ref_kept_as_a_branch() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.remote_rejects("refs/convoy/");

    let ticked = tick_at(&world, "1000");
    let said = warnings(&ticked).join("\n");
    assert!(said.contains("rejects refs under refs/convoy/*"), "{}", ticked.stderr);
    assert!(said.contains("not allowed here"), "the server's own words: {said}");
    assert!(said.contains("CI that runs on every push will fire"), "{said}");
    let ticked = ticked.ok();
    assert_eq!(did(&ticked), ["dispatch 7 done", "heartbeat done"]);
    assert_eq!(ticked["errors"], json!([]));

    assert_eq!(refs(&world, "refs/convoy"), [] as [String; 0]);
    assert_eq!(
        refs(&world, "refs/heads/convoy-"),
        ["refs/heads/convoy-claim/7", "refs/heads/convoy-heartbeat/alpha"]
    );
    // every later command names the refs the start settled on, probe or none
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    let scan = world.afk(&["scan"]).ok();
    assert_eq!(scan["heartbeats"]["alpha"], 1000);

    // …a worker's too: its green run goes on record as a branch
    world.set_gate("true");
    let gated = world.afk(&["gate", "--now", "1100"]).ok();
    assert_eq!(gated["recorded"], true, "{gated}");
    let tree = git(&world.clone, &["rev-parse", "HEAD^{tree}"]);
    let records = refs(&world, "refs/heads/convoy-gate");
    assert_eq!(records.len(), 1);
    assert!(records[0].starts_with(&format!("refs/heads/convoy-gate/{tree}-")), "{records:?}");
    // …and the next gate reads it back from there
    let again = world.afk(&["gate", "--now", "1200"]).ok();
    assert_eq!(again["status"], "green", "{again}");

    // the next start finds its claim where it left it
    let ticked = tick_at(&world, "1300").ok();
    assert_eq!(ticked["held"], 1, "{ticked}");
    assert_eq!(ticked["errors"], json!([]));
    assert_eq!(refs(&world, "refs/convoy"), [] as [String; 0]);
}

#[test]
fn a_remote_that_takes_hidden_refs_again_is_gone_back_to() {
    let world = World::new();
    world.remote_rejects("refs/convoy/");
    plan(&world).ok();
    world.watch_pushes(None);
    world.afk(&["claim", "--issue", "7", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(world.pushes().last().unwrap(), "refs/heads/convoy-claim/7");

    // the rule is lifted (the watching hook replaced the rejecting one), and
    // the probe that found it no longer stands
    let planned = world.afk(&["plan", "--now", "4600"]);
    assert_eq!(warnings(&planned), [] as [&str; 0]);
    world.afk(&["claim", "--issue", "8", "--instance", "alpha", "--now", "1000"]).ok();
    assert_eq!(world.pushes().last().unwrap(), "refs/convoy/claim/8");
}

#[test]
fn a_probe_push_that_reached_no_verdict_is_an_error_not_a_fallback() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    // The remote reads, and pushes go nowhere: nobody said refs are refused.
    let nowhere = world.root.join("no-such-remote.git");
    git(&world.clone, &["config", "remote.origin.pushurl", nowhere.to_str().unwrap()]);

    for start in [tick_at(&world, "1000"), plan(&world), run_once(&world)] {
        let error = start.error();
        assert!(error.contains("never reached a verdict from the remote"), "{error}");
        assert!(error.contains("refs/convoy/*"), "{error}");
    }
    assert_eq!(refs(&world, "refs/heads"), ["refs/heads/main"], "no branch layout was tried");
    assert_eq!(refs(&world, "refs/convoy"), [] as [String; 0]);
    assert!(!world.clone.join(".git/convoy-layout").exists(), "and none was noted");
    assert_eq!(world.orca()["worktrees"], json!([]), "nothing was dispatched");
}

#[test]
fn a_remote_that_rejects_both_layouts_is_an_error() {
    let world = World::new();
    world.remote_rejects("refs/");
    let error = plan(&world).error();
    assert!(error.contains("under both refs/convoy/*"), "{error}");
    assert!(error.contains("refs/heads/convoy-*"), "{error}");
    // A start's refusal names the topic that has a row for it.
    assert_eq!(plan(&world).json["see"], "afkconvoy help troubleshooting");
}

#[test]
fn a_base_branch_no_push_can_land_on_is_refused_at_start_naming_the_rule() {
    let rules = [
        (
            json!({"required_status_checks": {"contexts": ["ci"],
                "checks": [{"context": "ci"}, {"context": "lint"}]}}),
            "requires status checks (ci, lint)",
        ),
        (
            json!({"required_pull_request_reviews": {"required_approving_review_count": 1}}),
            "requires pull request reviews",
        ),
        (json!({"restrictions": {"users": [], "teams": []}}), "restricts who may push"),
        (json!({"lock_branch": {"enabled": true}}), "is locked"),
    ];
    for (rule, named) in rules {
        let world = World::new();
        world.issue(7, &["ready-for-agent"]);
        world.protect(rule);
        world.watch_pushes(None);
        for start in [tick_at(&world, "1000"), plan(&world), run_once(&world)] {
            let error = start.error();
            assert!(error.contains(&format!("the base branch \"main\" {named}")), "{error}");
        }
        assert_eq!(world.probes(), [] as [String; 0], "refused before anything was pushed");
        assert_eq!(world.pushes(), [] as [String; 0]);
        assert_eq!(world.orca()["worktrees"], json!([]), "{named}: nothing was dispatched");
    }
}

#[test]
fn a_protection_that_stops_no_push_is_no_obstacle() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.protect(json!({"required_linear_history": {"enabled": true},
        "required_status_checks": null, "restrictions": null,
        "lock_branch": {"enabled": false}}));
    let ticked = tick_at(&world, "1000");
    assert_eq!(warnings(&ticked), [] as [&str; 0]);
    assert_eq!(did(&ticked.ok()), ["dispatch 7 done", "heartbeat done"]);
}

#[test]
fn a_protection_that_cannot_be_read_is_a_warning() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.protect(json!("Resource not accessible by integration (HTTP 403)"));
    let ticked = tick_at(&world, "1000");
    let said = warnings(&ticked).join("\n");
    assert!(said.contains("protection of the base branch \"main\" could not be read"), "{said}");
    assert!(said.contains("HTTP 403"), "{said}");
    assert_eq!(did(&ticked.ok()), ["dispatch 7 done", "heartbeat done"]);
}

#[test]
fn a_base_branch_the_remote_lacks_is_an_error_and_none_is_created() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let config = "```yaml\nbase_branch: release\ngate_command: ./gate.sh\n```\n";
    commit(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.watch_pushes(None);

    for start in [tick_at(&world, "1000"), plan(&world), run_once(&world)] {
        let error = start.error();
        assert!(error.contains("the remote has no branch \"release\""), "{error}");
        assert!(error.contains("afkconvoy creates none"), "{error}");
    }
    assert_eq!(refs(&world, "refs/heads"), ["refs/heads/main"]);
    assert_eq!(world.probes(), [] as [String; 0]);
    assert_eq!(world.claims(), []);
}

#[test]
fn a_remote_that_refuses_gate_records_is_a_warning_and_every_landing_runs_the_gate() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    world.set_gate("true");
    world.remote_rejects("refs/convoy/gate/");

    let ticked = tick_at(&world, "1000");
    let said = warnings(&ticked).join("\n");
    assert!(said.contains("refuses refs under refs/convoy/gate/*"), "{}", ticked.stderr);
    assert!(said.contains("every landing runs the gate itself"), "{said}");
    assert_eq!(warnings(&ticked).len(), 1, "the layout itself was taken: {said}");
    assert_eq!(did(&ticked.ok()), ["dispatch 7 done", "heartbeat done"]);
    assert_eq!(
        refs(&world, "refs/convoy"),
        ["refs/convoy/claim/7", "refs/convoy/heartbeat/alpha"],
        "the fleet's other refs stay where they were, and no probe is left"
    );

    // a green run cannot go on record: the next run of the gate is a run
    for now in ["1100", "1200"] {
        let gated = world.afk(&["gate", "--now", now]).ok();
        assert_eq!(gated["status"], "green", "{gated}");
        assert_eq!(gated["recorded"], false, "{gated}");
    }
}

#[test]
fn a_start_sweeps_gate_records_past_their_day_and_probes_left_behind() {
    let world = World::new();
    world.set_gate("true");
    let records = || refs(&world, "refs/convoy/gate");

    world.afk(&["gate", "--now", "5000"]).ok();
    let old = records();
    commit(&world.clone, "more.txt", "more\n");
    world.afk(&["gate", "--now", "80000"]).ok();
    assert_eq!(records().len(), 2);
    // a start killed mid-probe left these; and one record nobody can read
    let head = git(&world.clone, &["rev-parse", "HEAD"]);
    for litter in ["refs/convoy/probe-1-1", "refs/convoy/gate/probe-1-1", "refs/convoy/gate/junk"] {
        git(&world.clone, &["push", "-q", "origin", &format!("{head}:{litter}")]);
    }

    // a day after the first run, to the second: it is still believed
    world.afk(&["plan", "--now", "91400"]).ok();
    assert_eq!(records().len(), 2, "{:?}", records());
    assert_eq!(refs(&world, "refs/convoy/probe-1-1"), [] as [String; 0]);

    // one second on, it is not: `run` probes at every start
    run_once_at(&world, "91401").ok();
    let kept = records();
    assert_eq!(kept.len(), 1, "{kept:?}");
    assert_ne!(kept, old, "the record of the second run stays");

    // only gate records are swept: a claim of any age stays
    world.issue(7, &[]);
    world.afk(&["claim", "--issue", "7", "--instance", "alpha", "--now", "100"]).ok();
    world.afk(&["plan", "--now", "999999"]).ok();
    assert_eq!(refs(&world, "refs/convoy"), ["refs/convoy/claim/7", "refs/convoy/heartbeat/alpha"]);
}

#[test]
fn under_the_branch_layout_a_pr_still_joins_the_train_and_lands() {
    let world = World::new();
    world.set_gate("true");
    world.remote_rejects("refs/convoy/");
    assert_eq!(warnings(&plan(&world)).len(), 1);
    let line = "refs/heads/convoy-train/line/main";

    let seven = world.worker(7, "seven.txt", "7\n");
    assert_eq!(world.afk_in(&seven, &["gate", "--train"]).ok()["recorded"], true);
    let joined = world.afk_in(&seven, &["land", "--issue", "7"]).ok();
    assert_eq!(joined["outcome"], "joined", "{joined}");
    assert_eq!(world.remote(&["rev-parse", line]), joined["commit"].as_str().unwrap());

    let train = world.root.join("train");
    git(&world.clone, &["fetch", "-q", "origin"]);
    let at = train.to_str().unwrap();
    git(&world.clone, &["worktree", "add", "-q", "-b", "the-train", at, "origin/main"]);
    let landed = world.afk_in(&train, &["land", "--train", "--merged-timeout", "5"]).ok();
    assert_eq!(landed["outcome"], "landed", "{landed}");
    assert_eq!(landed["gate"]["source"], "recorded", "the worker's run was read back: {landed}");
    assert_eq!(world.remote(&["rev-parse", "refs/heads/main"]), joined["commit"].as_str().unwrap());
    assert_eq!(refs(&world, "refs/convoy"), [] as [String; 0]);
}
