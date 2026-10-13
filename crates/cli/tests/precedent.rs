//! Precedents (docs/adr/0011): a human's ruling on an escalated issue is
//! proposed once, by a worker in the role the team names, as a PR on the
//! team's repo that appends one entry to its precedents file. The fleet
//! closes a PR that is anything else and merges none; the issue goes on
//! without waiting.

mod common;

use std::path::{Path, PathBuf};

use common::{World, git, write};
use serde_json::{Value, json};

const ROLES: &str = "escalate_label: stuck\nroles:\n  dev: acme/dev\n  clerk: acme/clerk\n";
const FEATURE: &str = "\
```yaml
stages:
  - name: build
    by: dev
    do: implement
    on: { pass: sign-off }
  - name: sign-off
    by: human
    on: { pass: done, reject: build }
```
";
const KEPT: &str = "# Precedents\n\n- Keep v1 schemas. **When:** a migration is not asked for \
                    (https://gh/c/1)\n";

/// A world whose repo is bound to a team that has `precedents` — the rest of
/// its `team.md` — and runs four workers at once.
fn bound(precedents: &str) -> World {
    let world = World::new();
    world.repo("acme/dev", &[(".claude/skills/implement/SKILL.md", "build\n")]);
    world.repo("acme/clerk", &[(".claude/skills/propose/SKILL.md", "propose\n")]);
    let team = format!("```yaml\n{ROLES}{precedents}```\n");
    world.repo(
        "acme/team",
        &[("team.md", &team), ("workflows/feature.md", FEATURE), ("precedents.md", KEPT)],
    );
    let config = "```yaml\nbase_branch: main\ngate_command: ./gate.sh\nteam: acme/team\n\
                  concurrency: 4\n```\n";
    write(&world.clone, "docs/agents/afkconvoy.md", config);
    git(&world.clone, &["commit", "-q", "-am", "bind the repo"]);
    git(&world.clone, &["push", "-q", "origin", "main"]);
    world
}

/// A team that has the role `clerk` propose, `max_open` at once.
fn proposing(max_open: u64) -> World {
    bound(&format!("precedents: {{ by: clerk, do: propose, max_open: {max_open} }}\n"))
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

/// Issue `issue` is escalated from its first stage, from `now`, and a human
/// rules on it → the id and the URL of the ruling.
fn ruled(world: &World, issue: u64, now: u64) -> (u64, String) {
    escalated(world, issue, now);
    rule(world, issue)
}

/// How `plan` says the proposals stand at `now`.
fn standing(world: &World, now: u64) -> Vec<String> {
    let plan = world.afk(&["plan", "--instance", "alpha", "--now", &now.to_string()]).ok();
    let Some(rows) = plan["proposals"].as_array() else { return vec![] };
    rows.iter()
        .map(|row| format!("{} {}", row["issue"], row["standing"].as_str().unwrap()))
        .collect()
}

/// The fleet's claims on proposals that the remote has.
fn claimed(world: &World) -> Vec<String> {
    let listed = git(&world.bare, &["for-each-ref", "--format=%(refname)"]);
    listed.lines().filter(|name| name.contains("precedent")).map(str::to_string).collect()
}

/// What names the proposal of the ruling in comment `comment` of `issue`:
/// its branch on the team's repo, and its checkout.
fn named(issue: u64, comment: u64) -> String {
    format!("convoy-precedent-{issue}-c{comment}")
}

fn checkout(world: &World, issue: u64, comment: u64) -> PathBuf {
    world.root.join("wt").join(named(issue, comment))
}

/// The worker of the proposal in `checkout` is gone.
fn worker_leaves(world: &World, checkout: &Path) {
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == checkout.to_str().unwrap() {
                terminal["open"] = json!(false);
            }
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

fn team(world: &World) -> PathBuf {
    world.root.join("github/acme/team")
}

/// A worker opens PR `number` on the team's repo from `branch`, having
/// changed there what `change` does.
fn opens(world: &World, number: u64, branch: &str, change: impl FnOnce(&Path)) {
    let dir = team(world);
    git(&dir, &["checkout", "-q", "-b", branch]);
    change(&dir);
    git(&dir, &["add", "-A"]);
    git(&dir, &["commit", "-q", "-m", "propose"]);
    git(&dir, &["checkout", "-q", "main"]);
    world.with_gh(|gh| {
        let rows = &mut gh["prs_of"]["acme/team"];
        if !rows.is_array() {
            *rows = json!([]);
        }
        let pr = json!({"number": number, "headRefName": branch, "state": "open"});
        rows.as_array_mut().unwrap().push(pr);
    });
}

fn append(dir: &Path, line: &str) {
    write(dir, "precedents.md", &format!("{KEPT}{line}\n"));
}

/// PR `number` of the team's repo, as the stand-in GitHub has it.
fn pr(world: &World, number: u64) -> Value {
    let gh = world.gh();
    let rows = gh["prs_of"]["acme/team"].as_array().unwrap();
    rows.iter().find(|pr| pr["number"] == number).unwrap().clone()
}

/// Whether anything was merged on GitHub, on any repo.
fn merged(world: &World) -> bool {
    world.gh_calls().iter().any(|call| call.contains(&"merge".to_string()))
}

#[test]
fn check_reads_who_proposes_precedents_and_a_team_without_the_key_has_none_proposed() {
    let world = proposing(3);
    assert_eq!(
        world.afk(&["check"]).ok()["precedents"],
        json!({"by": "clerk", "do": "propose", "max_open": 3})
    );
    let refused = bound("precedents: { by: nobody, do: propose }\n");
    let said = refused.afk(&["check"]).error();
    assert!(said.contains("precedents.by") && said.contains("precedents.max_open"), "{said}");

    // Without the key a ruling is proposed by nobody.
    let world = bound("");
    assert_eq!(world.afk(&["check"]).ok()["precedents"], json!(null));
    ruled(&world, 7, 1000);
    assert_eq!(standing(&world, 1100), Vec::<String>::new());
    assert_eq!(did(&tick(&world, 1100)), ["dispatch 7 done"]);
    assert_eq!(claimed(&world), Vec::<String>::new());
    assert!(!world.comments(7).join("\n").contains("convoy:precedent-proposed"));
}

#[test]
fn a_humans_ruling_starts_one_worker_in_the_role_the_team_names_and_the_issue_does_not_wait() {
    let world = proposing(3);
    let (comment, link) = ruled(&world, 7, 1000);
    assert_eq!(standing(&world, 1100), ["7 due"]);
    let base = git(&world.clone, &["rev-parse", "origin/main"]);
    world.orca_calls();

    // The stage that asked runs again first, in the same tick: it waits for nothing.
    let ticked = tick(&world, 1100);
    assert_eq!(did(&ticked), ["dispatch 7 done", "propose 7 done"]);
    assert_eq!(ticked["held"], 2, "the proposal's worker takes a slot");
    let line = world.afk_lines(&["tick", "--instance", "alpha", "--now", "1101"]).0.join("\n");
    assert!(!line.contains("proposed as a precedent"), "nothing is proposed twice: {line}");

    // Its worker is in a checkout of the base branch that is no issue's…
    let calls = world.orca_calls();
    let of = |what: &str| -> Vec<&Vec<String>> {
        calls.iter().filter(|call| common::asked(call) == what).collect()
    };
    let name = named(7, comment);
    let created: Vec<_> = of("worktree create")
        .into_iter()
        .filter(|call| option(call, "--name") == Some(name.as_str()))
        .collect();
    assert_eq!(created.len(), 1, "{calls:?}");
    assert_eq!(option(created[0], "--base-branch"), Some(base.as_str()));
    assert_eq!(option(created[0], "--issue"), None);
    // …started in the role the team names, with that skill…
    let started = of("terminal create");
    let commands: Vec<&str> = started.iter().filter_map(|call| option(call, "--command")).collect();
    let proposing: Vec<&&str> =
        commands.iter().filter(|command| command.contains("AFKCONVOY_PROPOSAL")).collect();
    let [command] = proposing.as_slice() else { panic!("one worker proposes: {commands:?}") };
    assert!(command.contains(" AFKCONVOY_PROPOSAL='7' AFKCONVOY_ROLE='clerk' "), "{command}");
    assert!(command.contains(" AFKCONVOY_SKILL='propose' "), "{command}");
    assert!(!command.contains("AFKCONVOY_ISSUE") && !command.contains("AFKCONVOY_STAGE"));
    let clerk = world.cache().join("afkconvoy/copies/acme/clerk");
    assert!(command.contains(&format!("cd '{}", clerk.display())), "{command}");
    // …and handed the ruling, where it is proposed, and what an entry is.
    let dir = checkout(&world, 7, comment);
    let brief = git(&dir, &["rev-parse", "--absolute-git-dir"]);
    let brief = std::fs::read_to_string(PathBuf::from(brief).join("convoy-worker-prompt.md"));
    let brief = brief.unwrap();
    assert!(brief.contains(&format!("issue #7, which was escalated to them — {link}")), "{brief}");
    assert!(brief.contains("> Keep the v1 schema.\n> Migrate later."), "{brief}");
    assert!(brief.contains("`propose`, of the role you were started in (`clerk`)"), "{brief}");
    assert!(brief.contains(&format!("opening one PR on `acme/team`, from the branch `{name}`")));
    assert!(
        brief.contains(&format!(
            "- <the ruling, in a sentence> **When:** <when it applies> ({link})\n"
        )),
        "{brief}"
    );
    assert!(brief.contains("/acme/team/") && brief.contains("/precedents.md`"), "{brief}");
    // It ends with the line that declares the run over, run from the checkout.
    let over = format!(" over --run 'precedent-7-c{comment}'\n```\n");
    assert!(brief.ends_with(&over) && brief.contains(&format!("cd '{}' && ", dir.display())));

    // The issue says it was proposed, and the proposal is claimed.
    let said = world.comments(7).join("\n");
    assert!(said.contains(&format!("<!--convoy:precedent-proposed ruling={comment}-->")), "{said}");
    assert_eq!(claimed(&world), [format!("refs/convoy/claim/precedent-7-c{comment}")]);
    assert_eq!(standing(&world, 1101), ["7 running"]);
    assert_eq!(world.afk(&["scan"]).ok()["claims"].as_array().unwrap().len(), 1);

    // While its worker is at it nothing more is started; once it is gone the
    // proposal is over, with or without a PR, and its slot and checkout go.
    assert_eq!(did(&tick(&world, 1200)), Vec::<String>::new());
    worker_leaves(&world, &dir);
    let ended = tick(&world, 1300);
    assert_eq!(did(&ended), ["end_proposal 7 done"]);
    assert_eq!(ended["held"], 1);
    assert_eq!(claimed(&world), Vec::<String>::new());
    assert!(!dir.exists(), "its checkout is removed");
    // A ruling gets one proposal: none is started for it again.
    assert_eq!(standing(&world, 1400), Vec::<String>::new());
    assert_eq!(did(&tick(&world, 1400)), Vec::<String>::new());
    assert!(calls_none(&world), "no worker is started again");
    assert!(!merged(&world));
}

fn calls_none(world: &World) -> bool {
    world.orca_calls();
    calls(world, "terminal create").is_empty()
}

#[test]
fn a_verdict_a_human_declares_for_their_own_stage_is_no_ruling_to_propose() {
    let world = proposing(3);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    let commits = ["--role-commit", "r1", "--team-commit", "t1"];
    let pass = ["verdict", "pass", "--issue", "7", "--stage", "build", "--text", "Built."];
    world.afk(&[&pass[..], &commits].concat()).ok();
    assert_eq!(did(&tick(&world, 1030)), ["advance 7 done"]);
    // The stage `sign-off` is a human's: they reject, and `build` runs again.
    let reject = ["verdict", "reject", "--issue", "7", "--stage", "sign-off", "--text", "No."];
    assert_eq!(world.afk(&reject).ok()["declared"], true);
    let said = world.comments(7).join("\n");
    assert!(said.contains("stage=sign-off verdict=reject by=human-->"), "{said}");

    assert_eq!(standing(&world, 1100), Vec::<String>::new());
    assert_eq!(did(&tick(&world, 1100)), ["dispatch 7 done"]);
    assert_eq!(claimed(&world), Vec::<String>::new());
    assert!(!world.comments(7).join("\n").contains("convoy:precedent-proposed"));
}

#[test]
fn a_proposal_s_worker_declares_its_run_over_and_the_next_tick_ends_it() {
    let world = proposing(3);
    let (comment, _) = ruled(&world, 7, 1000);
    assert_eq!(did(&tick(&world, 1100)), ["dispatch 7 done", "propose 7 done"]);
    let dir = checkout(&world, 7, comment);
    let run = format!("precedent-7-c{comment}");

    // Its worker still at it, the proposal goes on…
    assert_eq!(did(&tick(&world, 1110)), Vec::<String>::new());
    // …until it declares the run over: the next tick ends it, PR or none.
    let said = world.afk_in(&dir, &["over", "--run", &run]).ok();
    assert_eq!(said["ref"], format!("refs/convoy/over/{run}"));
    assert_eq!(claimed(&world).len(), 2, "the claim, and what was declared of it");
    assert_eq!(did(&tick(&world, 1160)), ["end_proposal 7 done"]);
    assert!(!dir.exists(), "its checkout is removed");
    assert_eq!(claimed(&world), Vec::<String>::new());
}

/// A ruling on issue 7 is proposed, its worker opens PR 31 on the team's
/// repo having changed what `change` does — handed the team's checkout and
/// the link to the ruling — and stops → the PR after the tick that ends the
/// proposal, and the world.
fn proposed(change: impl FnOnce(&Path, &str)) -> (Value, World) {
    let world = proposing(3);
    let (comment, link) = ruled(&world, 7, 1000);
    assert_eq!(did(&tick(&world, 1100)), ["dispatch 7 done", "propose 7 done"]);
    opens(&world, 31, &named(7, comment), |dir| change(dir, &link));
    // While the worker is at it, its PR is left alone whatever it holds.
    tick(&world, 1150);
    assert_eq!(pr(&world, 31)["state"], "open");
    worker_leaves(&world, &checkout(&world, 7, comment));
    assert_eq!(did(&tick(&world, 1200)), ["end_proposal 7 done"]);
    (pr(&world, 31), world)
}

#[test]
fn a_pr_that_appends_one_entry_linked_to_the_ruling_is_left_for_a_human_to_merge() {
    let (left, world) = proposed(|dir, link| {
        append(dir, &format!("- Keep the v1 schema. **When:** a migration is asked ({link})"));
    });
    assert_eq!((&left["state"], &left["closed_with"]), (&json!("open"), &json!(null)));
    // The fleet merges nothing on the team's repo, now or later.
    tick(&world, 1300);
    assert_eq!(pr(&world, 31)["state"], "open");
    assert!(!merged(&world));
    assert_eq!(standing(&world, 1300), Vec::<String>::new());
}

#[test]
fn a_pr_that_is_anything_but_one_appended_entry_is_closed_with_the_reason() {
    let closed = |change: &dyn Fn(&Path, &str)| -> String {
        let (pr, world) = proposed(change);
        assert_eq!(pr["state"], "closed", "{pr}");
        assert!(!merged(&world));
        // Closed is the end of it: the ruling is not proposed again.
        assert_eq!(did(&tick(&world, 1300)), Vec::<String>::new());
        pr["closed_with"].as_str().unwrap().to_string()
    };
    let entry = |link: &str| format!("- Keep the v1 schema. **When:** always ({link})");

    // It changes another file of the team.
    let why = closed(&|dir, link| {
        append(dir, &entry(link));
        write(dir, "workflows/feature.md", "loosened\n");
    });
    assert!(
        why.contains(
            "it changes `workflows/feature.md`: a proposal changes `precedents.md` and nothing \
             else"
        ),
        "{why}"
    );
    assert!(why.starts_with("**afkconvoy** — closed: this PR was opened to propose a precedent"));

    // It rewrites a precedent that was kept.
    let why = closed(&|dir, link| {
        let rewritten = KEPT.replace("Keep v1 schemas.", "Drop v1 schemas.");
        write(dir, "precedents.md", &format!("{rewritten}{}\n", entry(link)));
    });
    assert!(why.contains("it removes or rewrites a line of `precedents.md`"), "{why}");

    // It appends more than its one entry.
    let why = closed(&|dir, link| append(dir, &format!("{}\n{}", entry(link), entry(link))));
    assert!(why.contains("it adds 2 lines to `precedents.md`"), "{why}");

    // What it appends is no entry: it does not say when it applies.
    let why = closed(&|dir, link| append(dir, &format!("- Keep the v1 schema. ({link})")));
    assert!(why.contains("the line it appends is no entry"), "{why}");
    assert!(why.contains("<the ruling, in a sentence> **When:** <when it applies>"), "{why}");

    // Its entry does not link to the ruling.
    let why = closed(&|dir, _| append(dir, &entry("https://gh/c/1")));
    assert!(why.contains("its entry links to https://gh/c/1, not to the ruling"), "{why}");
}

#[test]
fn no_proposal_is_started_while_the_open_ones_have_reached_max_open() {
    let world = proposing(1);
    // A PR on the team's repo that is no proposal is not counted.
    opens(&world, 30, "tidy-the-roles", |dir| write(dir, "notes.md", "tidy\n"));
    escalated(&world, 7, 1000);
    escalated(&world, 8, 2000);
    let (first, link) = rule(&world, 7);
    rule(&world, 8);
    assert_eq!(standing(&world, 2100), ["7 due", "8 full"]);
    assert_eq!(did(&tick(&world, 2100)), ["dispatch 7 done", "dispatch 8 done", "propose 7 done"]);
    // One is being made: the other waits for it, and for the human after it.
    assert_eq!(standing(&world, 2150), ["7 running", "8 full"]);
    opens(&world, 31, &named(7, first), |dir| {
        append(dir, &format!("- Keep the v1 schema. **When:** always ({link})"));
    });
    worker_leaves(&world, &checkout(&world, 7, first));
    assert_eq!(did(&tick(&world, 2200)), ["end_proposal 7 done"]);
    assert_eq!(standing(&world, 2300), ["8 full"]);
    assert_eq!(did(&tick(&world, 2300)), Vec::<String>::new());

    // A human merges it: there is room for the next.
    world.with_gh(|gh| gh["prs_of"]["acme/team"][1]["state"] = json!("merged"));
    assert_eq!(standing(&world, 2400), ["8 due"]);
    assert_eq!(did(&tick(&world, 2400)), ["propose 8 done"]);
    assert!(!merged(&world));
}

#[test]
fn a_stage_s_brief_says_where_the_precedents_are_and_to_read_them_before_an_escalate() {
    let world = proposing(3);
    world.issue(7, &["flow:feature"]);
    assert_eq!(did(&tick(&world, 1000)), ["dispatch 7 done"]);
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().find(|row| row["linkedIssue"] == 7).expect("a worktree");
    let git_dir =
        git(Path::new(row["path"].as_str().unwrap()), &["rev-parse", "--absolute-git-dir"]);
    let brief = std::fs::read_to_string(PathBuf::from(git_dir).join("convoy-worker-prompt.md"));
    let brief = brief.unwrap();
    let kept = world.cache().join("afkconvoy/copies/acme/team");
    let at = brief.lines().find(|line| line.starts_with("- **Precedents:** `")).expect("a place");
    assert!(at.contains(&kept.display().to_string()) && at.ends_with("/precedents.md`"), "{at}");
    assert!(brief.contains("Read them before you declare `escalate`."), "{brief}");
    assert!(
        brief.contains("say in the text of that verdict that you read the precedents."),
        "{brief}"
    );
}

/// #132: a proposal claimed by a fleet that was killed is taken by the start
/// of the next one on its host, with the host's claims on issues.
#[test]
fn a_start_takes_a_proposal_stamped_with_its_host_and_the_next_tick_ends_it() {
    let world = proposing(3);
    let (comment, _) = ruled(&world, 7, 1000);
    assert_eq!(did(&tick(&world, 1100)), ["dispatch 7 done", "propose 7 done"]);
    let plan = world.afk(&["plan", "--instance", "omega", "--now", "1110"]).ok();
    assert_eq!(plan["proposals"][0]["standing"], "peer_running");

    worker_leaves(&world, &checkout(&world, 7, comment));
    let args = ["run", "--instance", "omega", "--host", "h1", "--second-ms", "0"];
    let (lines, _) = world.afk_lines(&[&args[..], &["--ticks", "1", "--now", "1160"]].concat());
    assert_eq!(lines[0], "start: took #7, the proposal of the ruling on #7 stamped with this host");
    assert!(lines[1].contains("ended the proposal of the ruling on #7"), "{}", lines[1]);
    assert_eq!(claimed(&world), Vec::<String>::new());
    let plan = world.afk(&["plan", "--instance", "omega", "--now", "1170"]).ok();
    assert_eq!(plan["proposals"].as_array().map_or(0, Vec::len), 0, "{}", plan["proposals"]);
}
