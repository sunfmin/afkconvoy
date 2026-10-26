//! The checkout a command is about: named after it, or with `-C` where what
//! follows the command is its own, or else the one it is run in — any
//! directory of a checkout naming that checkout, and a worker's worktree
//! refused but for the commands a worker runs there and those that only read.

use crate::common;

use std::path::{Path, PathBuf};
use std::process::Command;

use common::{World, git};
use serde_json::json;

/// What the binary's `--help` of `command` prints.
fn text_of(command: &str) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_afkconvoy")).args([command, "--help"]).output();
    String::from_utf8(out.unwrap().stdout).unwrap()
}

/// The worktree of the worker dispatched on #3, as alpha.
fn worker_worktree(world: &World) -> PathBuf {
    world.issue(3, &["ready-for-agent"]);
    let on = ["dispatch", "--issue", "3", "--instance", "alpha", "--now", "1000"];
    assert_eq!(world.afk(&on).ok()["started"], true);
    let worktrees = world.worktrees();
    assert_eq!(worktrees.len(), 1, "{worktrees:?}");
    worktrees[0].clone()
}

fn path(dir: &Path) -> &str {
    dir.to_str().unwrap()
}

/// What `exclusive -C <dir> -- pwd`, run in `cwd`, exits with and prints.
fn exclusive_in(world: &World, cwd: &Path, dir: &Path) -> (i32, String) {
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), cwd)
        .args(["exclusive", "-C", path(dir), "--", "pwd"])
        .output()
        .unwrap();
    (out.status.code().unwrap(), String::from_utf8_lossy(&out.stdout).trim().to_string())
}

#[test]
fn a_command_run_outside_any_checkout_is_about_the_one_it_names() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let elsewhere = &world.root;
    let here = world.afk(&["config"]).ok();
    assert_eq!(world.afk_in(elsewhere, &["config", path(&world.clone)]).ok(), here);
    let plan = world.afk_in(elsewhere, &["plan", path(&world.clone), "--now", "1000"]).ok();
    assert_eq!(plan["frontier"][0]["number"], 7, "{plan}");
    // a path is taken from where the command is run
    assert_eq!(world.afk_in(elsewhere, &["config", "clone"]).ok(), here);
    // naming none, it is about the checkout it is run in: here, none
    let error = world.afk_in(elsewhere, &["config"]).error();
    assert!(error.ends_with("is not in a git checkout"), "{error}");
    // a checkout named that is none is refused, whatever the command needs
    let file = world.clone.join("docs/agents/afkconvoy.md");
    let named = ["config", path(&world.root), "--file", path(&file)];
    let error = world.afk_in(elsewhere, &named).error();
    assert!(error.ends_with("is not in a git checkout"), "{error}");
    // …and with none named, a command that needs none is run anywhere
    assert_eq!(world.afk_in(elsewhere, &["config", "--file", path(&file)]).ok(), here);
}

#[test]
fn a_directory_inside_a_checkout_names_the_checkout() {
    let world = World::new();
    let inside = world.clone.join("docs/agents");
    let here = world.afk(&["config"]).ok();
    assert_eq!(world.afk_in(&world.root, &["config", path(&inside)]).ok(), here);
    assert_eq!(world.afk_in(&inside, &["config"]).ok(), here);
    assert_eq!(world.afk_in(&inside, &["flows"]).ok(), world.afk(&["flows"]).ok());
}

#[test]
fn a_command_with_arguments_of_its_own_names_its_checkout_with_c() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let clone = path(&world.clone);
    let claim = ["claim", "-C", clone, "7", "--instance", "beta", "--now", "1000"];
    assert_eq!(world.afk_in(&world.root, &claim).ok()["won"], true);
    assert_eq!(world.claims(), [(7, "beta".to_string())]);
    // where what follows it is its own, a checkout is no argument of it
    let error = world.afk_in(&world.root, &["claim", clone, "7", "--instance", "beta"]).error();
    assert!(error.contains("unexpected argument"), "{error}");
    let inside = world.clone.join("docs");
    let release = ["release", "-C", path(&inside), "--issue", "7", "--instance", "beta"];
    assert_eq!(world.afk_in(&world.root, &release).ok()["released"], true);
    assert_eq!(world.claims(), []);
    // `exclusive` runs its command there
    assert_eq!(exclusive_in(&world, &world.root, &inside), (0, path(&inside).to_string()));
}

#[test]
fn show_points_a_positional_checkout_to_c() {
    let world = World::new();
    let dir = world.root.join("a checkout's directory");
    std::fs::create_dir(&dir).unwrap();
    // A directory whose name is the issue number is not the extra argument.
    std::fs::create_dir(world.root.join("11")).unwrap();
    for named in [path(&dir), "a checkout's directory"] {
        for args in [["show", "--issue", "11", named].as_slice(), &["show", "11", named]] {
            let answer = world.afk_in(&world.root, args);
            assert_eq!(answer.code, 2, "{answer:?}");
            assert!(answer.stderr.is_empty(), "{answer:?}");
            let error = answer.error();
            assert!(error.starts_with("error: "), "{error}");
            assert!(error.contains("Usage: afkconvoy show"), "{error}");
            let quoted = format!("'{}'", named.replace('\'', "'\\''"));
            let line =
                format!("show names its checkout with -C: afkconvoy show -C {quoted} --issue 11");
            assert!(error.lines().any(|said| said == line), "{args:?}: {error}");
        }
    }
    let args = ["--now", "1000", "show", "--dir", ".", "--team", "acme/team", "11", path(&dir)];
    let error = world.afk_in(&world.root, &args).error();
    assert!(error.ends_with("--issue 11 --dir '.' --team 'acme/team' --now 1000"), "{error}");
}

#[test]
fn show_adds_no_checkout_guidance_for_other_argument_errors() {
    let world = World::new();
    let dir = path(&world.clone);
    let file = world.root.join("a file");
    std::fs::write(&file, "").unwrap();
    for args in [
        vec!["show", "--issue", "11", "not-a-directory"],
        vec!["show", "11", path(&file)],
        vec!["show", "--dir", dir, "11", "extra"],
        vec!["show", "-C", dir, "--issue", "11", "workflow"],
        vec!["show", "--issue", "invalid", dir],
        vec!["show", "11", dir, "extra"],
        vec!["claim", dir, "7", "--instance", "beta"],
    ] {
        let error = world.afk(&args).error();
        assert!(!error.contains("show names its checkout with -C:"), "{args:?}: {error}");
    }
}

#[test]
fn exclusive_refuses_a_directory_of_no_checkout_before_it_waits() {
    let world = World::new();
    for (named, said) in [
        (world.root.join("nowhere"), "is not a directory"),
        (world.home(), "is not in a git checkout"),
    ] {
        let (code, printed) = exclusive_in(&world, &world.root, &named);
        assert_eq!(code, 125, "{printed}");
        let answer: serde_json::Value = serde_json::from_str(&printed).unwrap();
        let error = answer["error"].as_str().unwrap();
        assert!(error.starts_with("exclusive: "), "{error}");
        assert!(error.contains(said), "{error}");
        assert!(error.ends_with("the command did not run"), "{error}");
    }
    // …and no place in the queue was ever taken for it
    let queue = world.cache().join("afkconvoy/exclusive");
    let held = std::fs::read_dir(&queue).map_or(0, |dir| dir.count());
    assert_eq!(held, 0, "{}", queue.display());
}

#[test]
fn a_workers_worktree_is_refused_for_a_fleet_command_and_taken_for_a_workers() {
    let world = World::new();
    let worktree = worker_worktree(&world);
    let refused = |cwd: &Path, args: &[&str]| {
        let error = world.afk_in(cwd, args).error();
        assert!(error.contains("is a worker's worktree"), "{args:?}: {error}");
        assert!(error.contains(path(&world.clone)), "it names a checkout to use: {error}");
    };
    refused(&worktree, &["flows"]);
    refused(&world.root, &["claim", "-C", path(&worktree), "3", "--instance", "beta"]);
    refused(&world.root, &["park", "-C", path(&worktree), "3", "--instance", "alpha"]);
    refused(&worktree, &["takeover", "--from", "beta-h2-1", "--instance", "alpha"]);
    // the commands a worker runs there are taken
    let woke = world.afk_in(&worktree, &["wake"]).ok();
    assert_eq!(woke["woke"], false, "{woke}");
    let plan = world.afk_in(&worktree, &["plan", "--now", "1000"]).ok();
    assert_eq!(plan["frontier"], json!([]), "{plan}");
    // …and those that only read, as a consult's brief lets it run any
    let scan = world.afk_in(&worktree, &["scan"]).ok();
    assert_eq!(scan["claims"][0]["number"], 3, "{scan}");
    assert_eq!(world.afk_in(&world.root, &["scan", path(&worktree)]).ok(), scan);
    assert_eq!(world.afk_in(&worktree.join("docs"), &["config"]).ok(), world.afk(&["config"]).ok());
    let listed = world.afk_in(&worktree, &["takeover", "--list", "--now", "1000"]).ok();
    let instances = listed["instances"].as_array().unwrap();
    assert!(instances.iter().any(|row| row["instance"] == "alpha"), "{listed}");
    world.afk_in(&worktree, &["recovery", "3", "--now", "1000"]).ok();
    let panel = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &worktree)
        .args(["panel", "--now", "1000"])
        .output()
        .unwrap();
    assert!(panel.status.success(), "{panel:?}");
    assert!(String::from_utf8_lossy(&panel.stdout).starts_with("<!"), "{panel:?}");
    // a human's own worktree of the repo is no worker's
    let own = world.root.join("own");
    git(&world.clone, &["worktree", "add", "-q", "-b", "own", path(&own)]);
    assert_eq!(world.afk_in(&own, &["scan"]).ok()["claims"][0]["number"], 3);
}

#[test]
fn every_command_a_workers_brief_gives_it_takes_its_worktree() {
    let world = World::new();
    let worktree = worker_worktree(&world);
    let git_dir = PathBuf::from(git(&worktree, &["rev-parse", "--absolute-git-dir"]));
    let brief = std::fs::read_to_string(git_dir.join("convoy-worker-prompt.md")).unwrap();
    assert!(brief.contains(" wake"), "{brief}");
    assert_eq!(common::refusing_a_workers_worktree(&brief), [] as [String; 0]);
    // …as a command that would refuse it is found
    let said = "an afkconvoy worker runs `afkconvoy claim -C . 3` and `afkconvoy scan`";
    assert_eq!(common::refusing_a_workers_worktree(said), ["claim"]);
    assert_eq!(common::refusing_a_workers_worktree("run `afkconvoy flows`"), ["flows"]);
    assert_eq!(
        common::refusing_a_workers_worktree("run `afkconvoy flows -C <target-checkout>`"),
        [] as [String; 0]
    );
}

#[test]
fn a_tick_run_outside_its_checkout_starts_its_worker_there_and_asks_nothing_where_it_runs() {
    let world = World::new();
    world.issue(7, &["ready-for-agent"]);
    let clone = path(&world.clone);
    let tick = ["tick", clone, "--instance", "alpha", "--host", "h1", "--now", "1000"];
    let ticked = world.afk_in(&world.root, &tick).ok();
    let did: Vec<&str> =
        ticked["did"].as_array().unwrap().iter().filter_map(|d| d["do"].as_str()).collect();
    assert!(did.contains(&"dispatch"), "{ticked}");
    assert_eq!(world.claims(), [(7, "alpha".to_string())]);
    // the worker's worktree is the checkout's, and its terminal is in it
    let worktrees = world.worktrees();
    assert_eq!(worktrees.len(), 1, "{worktrees:?}");
    let common_dir =
        git(&worktrees[0], &["rev-parse", "--path-format=absolute", "--git-common-dir"]);
    assert_eq!(PathBuf::from(common_dir), world.clone.join(".git"));
    let orca = world.orca();
    let started: Vec<&serde_json::Value> =
        orca["terminals"].as_array().unwrap().iter().filter(|t| t["command"].is_string()).collect();
    assert_eq!(started.len(), 1, "{started:?}");
    assert_eq!(started[0]["worktreePath"], path(&worktrees[0]));
    // orca was run from the host's home, never from where the tick ran, and
    // the login shell was asked in the checkout
    let home = path(&world.home()).to_string();
    let from = world.orca_run_from();
    assert!(!from.is_empty() && from.iter().all(|dir| *dir == home), "{from:?}");
    let asked_in = world.shell_asked_in();
    assert!(!asked_in.is_empty() && asked_in.iter().all(|dir| dir == clone), "{asked_in:?}");
}

#[test]
fn on_a_host_with_no_git_a_start_names_every_command_it_lacks_and_any_other_git_alone() {
    let world = World::new();
    world.logged_out("gh");
    world.take_off_path("git");
    let answer = world.afk(&["scan"]);
    assert_eq!(answer.json["error"], "git is not installed: brew install git", "{}", answer.json);
    assert_eq!(answer.json["see"], "afkconvoy help troubleshooting", "{}", answer.json);
    assert_eq!(world.shell_asked(), [] as [String; 0], "no other command was looked for");
    // …while a start names every one it lacks, before a host kit it has not
    // got yet either
    std::fs::remove_file(world.kit_path()).unwrap();
    let error = world.afk(&["plan", "--now", "1000"]).error();
    assert!(error.contains("nothing was started"), "{error}");
    assert!(error.contains("\n  git is not installed: brew install git\n"), "{error}");
    assert!(error.contains("  gh is not logged in: gh auth login"), "{error}");
    std::fs::remove_dir_all(world.root.join("path")).unwrap();
}

#[test]
fn each_command_about_one_repo_says_how_it_takes_its_checkout() {
    let heading = "## Which checkout it is about";
    for (command, form, worker) in [
        ("plan", "`[CHECKOUT]`", true),
        ("scan", "`[CHECKOUT]`", true),
        ("panel", "`[CHECKOUT]`", true),
        ("flows", "`[CHECKOUT]`", false),
        ("verdict", "`-C <CHECKOUT>`", true),
        ("recovery", "`-C <CHECKOUT>`", true),
        ("claim", "`-C <CHECKOUT>`", false),
    ] {
        let text = text_of(command);
        let said = text.split_once(heading).map(|(_, said)| said).unwrap_or_else(|| {
            panic!("`{command} --help` says nothing of its checkout: {text}");
        });
        let said = said.split("\n## ").next().unwrap();
        assert!(said.contains(form), "{command}: {said}");
        assert!(said.contains("directory in one"), "{command}: {said}");
        let refused = said.contains("A worker's worktree is refused");
        assert_eq!(!refused, worker, "{command}: {said}");
    }
    for command in ["kit", "skill", "scaffold", "draft", "guard"] {
        assert!(!text_of(command).contains(heading), "{command} is about no repo");
    }
}

#[test]
fn the_host_kit_is_asked_of_the_host_wherever_it_is_written_from() {
    let world = World::new();
    world.shell_claude_models(&["default", "opus"]);
    world.probes_answer();
    world.afk_in(&world.clone.join("docs"), &["kit", "--now", "1000"]).ok();
    let orca = world.orca();
    let questions = orca["questions"].as_array().unwrap();
    assert!(!questions.is_empty(), "{orca}");
    // in no worktree — not the checkout it was written from — and from the
    // host's home
    assert!(questions.iter().all(|asked| asked["worktreePath"] == ""), "{questions:?}");
    let home = path(&world.home()).to_string();
    let from = world.orca_run_from();
    assert!(from.iter().all(|dir| *dir == home), "{from:?}");
}
