//! Upgrade retirement and ruling-to-resume regressions for control#141.
use crate::common::{self, World, git, write};
use serde_json::{Value, json};
use std::path::PathBuf;
const ROLES: &str = "escalate_label: stuck\nroles:\n  dev: acme/dev\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    escalate: { to: human }
    on: { pass: done }
```
";
/// A bound team with optional legacy configuration in the rest of
/// its `team.md` and room for four workers.
fn bound(legacy: &str) -> World {
    let world = World::new();
    world.repo(
        "acme/dev",
        &[(".claude/skills/implement/SKILL.md", "---\nname: implement\ndescription: build\n---\n")],
    );
    let team = format!("```yaml\n{ROLES}{legacy}```\n");
    world.repo("acme/team", &[("team.md", &team), ("workflows/feature.md", FEATURE)]);
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world.list_checkout(&world.clone);
    world
}

/// One tick at `now`, every worker still in a terminal being at work.
fn tick(world: &World, now: u64) -> Value {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["open"] == true && !terminal["command"].is_null() {
                terminal["agent"] = json!({"state": "working", "stateStartedAt": now * 1000});
                terminal["lastOutputAt"] = json!(now * 1000);
            }
        }
    });
    let now = now.to_string();
    world.afk(&["tick", "--instance", "alpha", "--host", "h1", "--now", &now]).ok()
}

/// What a tick did that came to anything, but for the boards and the
/// `stage:` labels, in the order it did it.
fn did(ticked: &Value) -> Vec<String> {
    let steps = ticked["did"].as_array().unwrap().iter();
    let kept = steps.filter(|step| {
        !["board", "stage_label", "heartbeat"].contains(&step["do"].as_str().unwrap())
            && step["outcome"] != "nothing"
    });
    kept.map(|step| {
        let word = |key: &str| step[key].as_str().unwrap();
        format!("{} {} {}", word("do"), step["issue"], word("outcome"))
    })
    .collect()
}

/// The id and the URL of the comment in which a human ruled on `issue`.
fn ruling(world: &World, issue: u64) -> (u64, String) {
    let gh = world.gh();
    let comments = gh["comments"][issue.to_string()].as_array().unwrap().iter();
    let ruled = comments
        .rev()
        .find(|comment| comment["body"].as_str().unwrap().contains(" verdict=ruling by=human-->"))
        .expect("a ruling");
    (ruled["id"].as_u64().unwrap(), ruled["html_url"].as_str().unwrap().to_string())
}

/// Issue `issue` is escalated from its first stage, from `now`.
fn escalated(world: &World, issue: u64, now: u64) {
    let number = issue.to_string();
    world.issue(issue, &["flow:feature"]);
    assert_eq!(did(&tick(world, now)), [format!("dispatch {issue} done")]);
    let line = ["verdict", "escalate", "--issue", &number, "--stage", "build", "--text", "Which?"];
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    assert_eq!(world.afk(&[&line[..], &commits].concat()).ok()["declared"], true);
    assert_eq!(did(&tick(world, now + 30)), [format!("advance {issue} done")]);
}

/// A human rules on `issue`, escalated → the id and the URL of the ruling.
fn rule(world: &World, issue: u64) -> (u64, String) {
    let number = issue.to_string();
    world.afk(&["rule", "--issue", &number, "--text", "Keep the v1 schema.\nMigrate later."]).ok();
    ruling(world, issue)
}

fn proposal_refs(world: &World) -> Vec<String> {
    git(&world.bare, &["for-each-ref", "--format=%(refname)", "refs/convoy/claim"])
        .lines()
        .filter(|name| name.contains("precedent-"))
        .map(str::to_string)
        .collect()
}

fn record(world: &World, key: &str, subject: &str) {
    let repo = afkconvoy::git::Repo::at(&world.clone).unwrap();
    let sha = repo.record_commit(subject).unwrap();
    git(&world.clone, &["push", "-q", "--force", "origin", &format!("{sha}:refs/convoy/{key}")]);
}

fn legacy_worker(world: &World) -> PathBuf {
    let name = "convoy-precedent-7-c2001";
    let repo = world.orca()["repos"][0]["id"].as_str().unwrap().to_string();
    let out = world
        .command("orca", &world.clone)
        .args([
            "worktree",
            "create",
            "--repo",
            &format!("id:{repo}"),
            "--name",
            name,
            "--base-branch",
            "main",
            "--no-parent",
            "--json",
        ])
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
    let answer: Value = serde_json::from_slice(&out.stdout).unwrap();
    let path = PathBuf::from(answer["result"]["worktree"]["path"].as_str().unwrap());
    write(&path, "uncommitted-decision.md", "Keep this evidence.\n");
    path
}

#[test]
fn human_rulings_resume_the_issue_repeatedly_without_creating_proposals() {
    let world = bound("");
    for (at, now) in [1000, 1200].into_iter().enumerate() {
        if at == 0 {
            escalated(&world, 7, now);
        } else {
            let line =
                ["verdict", "escalate", "--issue", "7", "--stage", "build", "--text", "Again"];
            let commits = ["--role-commit", "r1", "--team-commit", "t1"];
            world.afk(&[&line[..], &commits].concat()).ok();
            tick(&world, now);
        }
        rule(&world, 7);
        let resumed = tick(&world, now + 90);
        assert_eq!(did(&resumed), ["dispatch 7 done"]);
        assert_eq!(resumed["held"], 1);
        assert!(resumed.get("proposals").is_none());
        assert!(proposal_refs(&world).is_empty());
        assert!(!world.comments(7).join("\n").contains("convoy:precedent-proposed"));
        let dir = world
            .worktrees()
            .into_iter()
            .find(|dir| dir.to_string_lossy().contains("issue-7"))
            .unwrap();
        let note = git(&dir, &["rev-parse", "--absolute-git-dir"]);
        let brief =
            std::fs::read_to_string(PathBuf::from(note).join("convoy-worker-prompt.md")).unwrap();
        assert!(
            brief.contains("Keep the v1 schema.") && !brief.contains("precedents.md"),
            "{brief}"
        );
    }
    assert!(world.gh_calls().iter().all(|call| common::asked(call) != "pr create"));
}

#[test]
fn legacy_configuration_is_refused_with_migration_instructions_and_no_proposer_skill_is_loaded() {
    for legacy in ["precedents: { by: missing, do: absent, max_open: 1 }\n", "precedents: null\n"] {
        let world = bound(legacy);
        let answer = world.afk(&["check"]);
        let said = answer.error();
        assert!(said.contains("retired") && said.contains("preserve source links"), "{said}");
        assert!(!said.contains("precedents.by") && !said.contains("precedents.do"), "{said}");
        tick(&world, 1000);
        assert!(proposal_refs(&world).is_empty());
    }
}

#[test]
fn an_owned_legacy_worker_is_stopped_but_its_work_branch_and_ruling_history_are_retained() {
    let world = bound("");
    let path = legacy_worker(&world);
    world.issue(7, &[]);
    world.comment(7, "Original decision evidence");
    record(&world, "claim/precedent-7-c2001", "convoy:claim instance=alpha host=h1 ts=1000");
    let result = tick(&world, 1100);
    assert_eq!(result["held"], 0, "{result}");
    assert_eq!(result["idle"], true);
    assert!(proposal_refs(&world).is_empty());
    assert!(!world.open_terminals().contains(&path.display().to_string()));
    assert!(path.join("uncommitted-decision.md").is_file());
    assert!(
        git(&world.clone, &["branch", "--list", "*convoy-precedent*"]).contains("convoy-precedent")
    );
    assert!(world.comments(7).iter().any(|body| body == "Original decision evidence"));
    let again = tick(&world, 1200);
    assert_eq!(again["held"], 0);
    assert_eq!(world.worktrees().len(), 1, "no replacement proposal worktree");
    assert!(world.afk(&["plan", "--instance", "alpha"]).ok().get("proposals").is_none());
}

#[test]
fn a_peers_legacy_worker_is_never_stopped_and_stale_claims_take_no_slot() {
    let world = bound("");
    let path = legacy_worker(&world);
    record(&world, "claim/precedent-7-c2001", "convoy:claim instance=beta host=h2 ts=1000");
    record(&world, "heartbeat/beta", "convoy:heartbeat instance=beta host=h2 ts=1000");
    let live = tick(&world, 1100);
    assert_eq!(live["held"], 0);
    assert_eq!(live["idle"], true);
    assert_eq!(proposal_refs(&world).len(), 1);
    assert!(world.open_terminals().contains(&path.display().to_string()));
    tick(&world, 10000);
    assert!(proposal_refs(&world).is_empty());
    assert!(
        world.open_terminals().contains(&path.display().to_string()),
        "never stop a remote peer's worker"
    );
    assert!(path.join("uncommitted-decision.md").is_file());
}

#[test]
fn legacy_prs_receive_a_retained_disposition_once_and_are_never_closed_or_merged() {
    let world = bound("");
    world.with_gh(|gh| {
        gh["prs_of"]["acme/team"] = json!([
            {"number": 31, "headRefName": "convoy-precedent-7-c2001", "state": "open"},
            {"number": 32, "headRefName": "convoy-precedent-8-c2002", "state": "open", "fork": true}
        ])
    });
    tick(&world, 1000);
    let gh = world.gh();
    let pr = &gh["prs_of"]["acme/team"][0];
    assert_eq!(pr["state"], "open");
    let text = pr["comments"][0].as_str().expect("migration disposition");
    for said in [
        "https://github.com/acme/team/pull/31",
        "Disposition: retain",
        "ordinary work",
        "source links",
    ] {
        assert!(text.contains(said), "{text}");
    }
    assert!(gh["prs_of"]["acme/team"][1]["comments"].is_null(), "forks left alone");
    world.gh_calls();
    tick(&world, 1100);
    assert!(
        !world
            .gh_calls()
            .iter()
            .any(|call| common::asked(call) == "pr list" && call.contains(&"acme/team".into())),
        "no ongoing proposal PR polling"
    );
    assert_eq!(world.gh()["prs_of"]["acme/team"][0]["comments"].as_array().unwrap().len(), 1);
}

#[test]
fn retirement_failure_does_not_hold_concurrency_and_the_next_tick_retries() {
    let world = bound("");
    world.list_checkout(&world.clone);
    let path = legacy_worker(&world);
    record(&world, "claim/precedent-7-c2001", "convoy:claim instance=alpha host=h1 ts=1000");
    world.with_orca(|orca| orca["close_fails"] = json!([path]));
    world.issue(9, &["flow:feature"]);
    let failed = tick(&world, 1100);
    assert_eq!(failed["held"], 1);
    assert_eq!(did(&failed), ["dispatch 9 done"]);
    assert_eq!(proposal_refs(&world).len(), 1, "failed stop retains the claim for retry");
    world.with_orca(|orca| orca["close_fails"] = json!([]));
    tick(&world, 1200);
    assert!(proposal_refs(&world).is_empty());
    assert!(path.join("uncommitted-decision.md").is_file());
}

#[test]
fn a_start_takes_and_retires_its_hosts_legacy_claim_without_restarting_the_proposal() {
    let world = bound("");
    let path = legacy_worker(&world);
    record(&world, "claim/precedent-7-c2001", "convoy:claim instance=before host=h1 ts=1000");
    record(&world, "heartbeat/before", "convoy:heartbeat instance=before host=h1 ts=1000");
    let (_, answer) = world.afk_lines(&[
        "run",
        "--foreground",
        "--instance",
        "alpha",
        "--host",
        "h1",
        "--now",
        "1100",
        "--ticks",
        "1",
        "--second-ms",
        "0",
    ]);
    assert_eq!(answer["ticks"], 1, "{answer}");
    assert!(proposal_refs(&world).is_empty());
    assert!(path.join("uncommitted-decision.md").is_file());
    assert!(!world.open_terminals().contains(&path.display().to_string()));
}

#[test]
fn retirement_accepts_orcas_numeric_name_suffix_and_leaves_the_archived_worktree_alone() {
    let world = bound("");
    let archived = legacy_worker(&world);
    world.with_orca(|orca| {
        for row in orca["worktrees"].as_array_mut().unwrap() {
            if row["path"] == archived.to_str().unwrap() {
                row["isArchived"] = json!(true);
            }
        }
    });
    let active = legacy_worker(&world);
    assert_eq!(active.file_name().unwrap(), "convoy-precedent-7-c2001-2");
    record(&world, "claim/precedent-7-c2001", "convoy:claim instance=alpha host=h1 ts=1000");
    let result = tick(&world, 1100);
    assert_eq!(result["held"], 0);
    assert!(proposal_refs(&world).is_empty());
    assert!(!world.open_terminals().contains(&active.display().to_string()));
    assert!(world.open_terminals().contains(&archived.display().to_string()));
    for kept in [active, archived] {
        assert!(kept.join("uncommitted-decision.md").is_file());
    }
}
