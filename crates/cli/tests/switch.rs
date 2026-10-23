use crate::common;

use std::path::{Path, PathBuf};

use common::{World, commit, git};
use serde_json::{Value, json};

const CODEX: &str = "codex --dangerously-bypass-approvals-and-sandbox";

fn tick(world: &World, now: u64) -> Value {
    let now = now.to_string();
    let args = ["tick", "--instance", "alpha", "--host", "h1", "--now", &now];
    world.afk(&args).ok()
}

fn switch(world: &World, now: &str, rest: &[&str]) -> common::Answer {
    let args = ["switch", "--instance", "alpha", "--host", "h1", "--now", now];
    world.afk(&[&args[..], rest, &[]].concat())
}

fn worktree(world: &World, issue: u64) -> PathBuf {
    let orca = world.orca();
    let rows = orca["worktrees"].as_array().unwrap();
    let row = rows.iter().rev().find(|row| row["linkedIssue"] == issue).expect("a worktree");
    PathBuf::from(row["path"].as_str().unwrap())
}

fn brief(dir: &Path) -> String {
    let git_dir = PathBuf::from(git(dir, &["rev-parse", "--absolute-git-dir"]));
    std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap()
}

/// The commands of the terminals open in `dir`.
fn open_commands(world: &World, dir: &Path) -> Vec<String> {
    let orca = world.orca();
    let terminals = orca["terminals"].as_array().unwrap();
    let here = terminals.iter().filter(|t| t["worktreePath"] == dir.to_str().unwrap());
    let open = here.filter(|t| t["open"] == true);
    open.map(|t| t["command"].as_str().unwrap_or("").to_string()).collect()
}

/// A world whose worker on issue 7 pushed its work, opened PR #70, and sits
/// at its prompt, its harness's access turned off.
fn blocked() -> World {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    tick(&world, 700);
    // Every harness is held once the worker started: no caster held 7 back.
    world.hold_every_harness();
    let dir = worktree(&world, 7);
    commit(&dir, "seven.txt", "7\n");
    git(&dir, &["push", "-q", "origin", "HEAD"]);
    world.pr(70, 7, &git(&dir, &["symbolic-ref", "--short", "HEAD"]));
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            terminal["screen"] =
                json!(["", "Your organization has disabled Claude Code access", "> ", ""]);
        }
    });
    world
}

#[test]
fn a_switch_moves_the_worker_onto_the_harness_in_the_same_worktree_and_records_it() {
    let world = blocked();
    let dir = worktree(&world, 7);
    let head = git(&dir, &["rev-parse", "HEAD"]);
    assert_eq!(open_commands(&world, &dir), ["agent"]);
    world.orca_calls();

    let moved = switch(&world, "800", &["7", "--harness", "codex"]).ok();
    assert_eq!(moved["issue"], 7);
    assert_eq!((&moved["from"], &moved["to"]), (&json!("claude-code"), &json!("codex")));
    assert_eq!(moved["worktree"], json!(dir));
    assert_eq!(moved["pr"], 70);
    assert_eq!(moved["chosen"], json!({"was": null, "now": "codex"}));

    // the old worker's terminal is closed, a codex worker is in the same worktree
    assert_eq!(open_commands(&world, &dir), [CODEX]);
    let calls: Vec<String> = world.orca_calls().iter().map(|call| common::asked(call)).collect();
    assert!(!calls.iter().any(|call| call == "worktree rm" || call == "worktree create"));
    assert_eq!(git(&dir, &["rev-parse", "HEAD"]), head);
    assert_eq!(world.claims().len(), 1);

    // its brief says what it continues from
    let said = brief(&dir);
    assert!(said.contains("## You continue earlier work"), "{said}");
    assert!(said.contains("A worker on `claude-code` was at this before you"), "{said}");
    assert!(said.contains("1 on the branch ahead of `origin/main`"), "{said}");
    assert!(said.contains("#70 is open"), "{said}");
    assert!(said.contains("Your organization has disabled Claude Code access"), "{said}");

    // the PR records the switch, which is no restart
    let records: Vec<String> =
        world.comments(70).into_iter().filter(|body| body.contains("convoy:join")).collect();
    assert_eq!(records.len(), 1);
    assert!(records[0].starts_with("<!--convoy:join switched=800/claude-code/codex/alpha-->"));
    assert!(records[0].contains("switched from `claude-code` to `codex` by `alpha`"));

    // every worker this host starts for the repo from now on is a codex one
    world.issue(8, &["ready-for-agent", "bug"]);
    tick(&world, 900);
    assert_eq!(open_commands(&world, &worktree(&world, 8)), [CODEX]);

    // `--clear` forgets the choice: the worker it names runs what its casting
    // says again — the host kit's first, for none — recorded as a switch is
    let eight = worktree(&world, 8);
    commit(&eight, "eight.txt", "8\n");
    git(&eight, &["push", "-q", "origin", "HEAD"]);
    world.pr(80, 8, &git(&eight, &["symbolic-ref", "--short", "HEAD"]));
    let back = switch(&world, "1000", &["8", "--clear"]).ok();
    assert_eq!(back["chosen"], json!({"was": "codex", "now": null}));
    assert_eq!((&back["from"], &back["to"]), (&json!("codex"), &json!("claude-code")));
    assert_eq!(back["pr"], 80);
    assert_eq!(open_commands(&world, &eight), ["agent"]);
    let records: Vec<String> =
        world.comments(80).into_iter().filter(|body| body.contains("convoy:join")).collect();
    assert_eq!(records.len(), 1);
    assert!(records[0].contains("switched from `codex` to `claude-code` by `alpha`"));
    let said = switch(&world, "1000", &["--harness", "config"]).error();
    assert!(said.contains("`--harness config` is gone: `--clear`"), "{said}");
    assert_eq!(open_commands(&world, &dir), [CODEX], "a worker it names not is left alone");
}

#[test]
fn a_harness_that_cannot_be_started_is_refused_before_any_worker_is_ended() {
    let world = blocked();
    let dir = worktree(&world, 7);
    world.logged_out("codex");
    let said = switch(&world, "800", &["--all", "--harness", "codex"]).error();
    assert!(said.contains("the harness `codex` cannot be started here"), "{said}");
    assert_eq!(open_commands(&world, &dir), ["agent"]);
    assert!(!brief(&dir).contains("You continue earlier work"));
    // nothing was chosen: the next start is on its casting still, which a
    // switch would have overruled
    world.issue(8, &["ready-for-agent", "bug", "harness:claude-code"]);
    tick(&world, 900);
    assert_eq!(open_commands(&world, &worktree(&world, 8)), ["agent"]);
}

#[test]
fn all_moves_every_claim_of_the_instance_and_a_claim_it_does_not_hold_is_refused() {
    let world = World::new();
    world.issue(7, &["ready-for-agent", "bug"]);
    world.issue(8, &["ready-for-agent", "bug"]);
    tick(&world, 700);
    world.hold_every_harness();
    let (seven, eight) = (worktree(&world, 7), worktree(&world, 8));
    // one worker is already gone: it is started again all the same
    world.with_orca(|orca| {
        for terminal in orca["terminals"].as_array_mut().unwrap() {
            if terminal["worktreePath"] == eight.to_str().unwrap() {
                terminal["open"] = json!(false);
            }
        }
    });

    let said = switch(&world, "800", &["9", "--harness", "codex"]).error();
    assert!(said.contains("holds no claim on issue #9"), "{said}");
    assert_eq!(open_commands(&world, &seven), ["agent"]);

    let moved = switch(&world, "800", &["--all", "--harness", "codex"]).ok();
    let switched = moved["switched"].as_array().unwrap();
    let issues: Vec<&Value> = switched.iter().map(|one| &one["issue"]).collect();
    assert_eq!(issues, [&json!(7), &json!(8)], "{moved}");
    assert_eq!(moved["failed"], json!([]));
    assert_eq!(open_commands(&world, &seven), [CODEX]);
    assert_eq!(open_commands(&world, &eight), [CODEX]);
    assert!(brief(&eight).contains("its terminal was already gone"), "{}", brief(&eight));
    assert_eq!(world.claims().len(), 2);
}
