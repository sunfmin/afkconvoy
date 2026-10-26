//! The fixture itself: what every later test stands on.

use crate::common;

use common::{BASE, World, git};

#[test]
fn the_bare_repo_is_the_clones_remote() {
    let world = World::new();
    std::fs::write(world.clone.join("a.txt"), "a\n").unwrap();
    git(&world.clone, &["add", "."]);
    git(&world.clone, &["commit", "-q", "-m", "a"]);
    git(&world.clone, &["push", "-q", "origin", BASE]);
    assert_eq!(world.remote(&["rev-parse", BASE]), git(&world.clone, &["rev-parse", "HEAD"]));
}

#[test]
fn gh_and_orca_on_path_are_the_stand_ins() {
    let world = World::new();
    let gh = world.command("gh", &world.clone).args(["issue", "list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&gh.stderr).contains("fake gh"));
    assert_eq!(world.gh()["calls"], serde_json::json!([["issue", "list"]]));

    let orca =
        world.command("orca", &world.clone).args(["no-such-call", "--json"]).output().unwrap();
    let answer: serde_json::Value = serde_json::from_slice(&orca.stdout).unwrap();
    assert_eq!(answer["ok"], false);
    assert_eq!(world.orca()["calls"], serde_json::json!([["no-such-call", "--json"]]));
}

/// A test killed before it ends never drops its world: nextest ends one past
/// its limit, and whoever stops a run ends every test in it. The next world
/// started where it ran removes it — the read-only copies a fleet kept there
/// included — and ends the host runner it started, which no one else would.
#[test]
fn a_world_its_test_left_killed_is_removed_by_the_next_with_its_host_runner() {
    use std::io::{BufRead, BufReader};
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};

    let mut killed = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "fixture::a_killed_test", "--ignored", "--nocapture"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let said = BufReader::new(killed.stdout.take().unwrap())
        .lines()
        .map_while(Result::ok)
        .find_map(|line| line.strip_prefix("left: ").map(str::to_string));
    killed.kill().unwrap();
    killed.wait().unwrap();
    let said = said.expect("the killed test says its world");
    let (root, runner) = said.rsplit_once(' ').unwrap();

    let _next = World::new();
    assert!(!std::path::Path::new(root).exists(), "{root} is left");
    let until = Instant::now() + Duration::from_secs(10);
    while Command::new("kill")
        .args(["-0", runner])
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success()
    {
        assert!(Instant::now() < until, "the host runner {runner} still runs");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
#[ignore = "a_world_its_test_left_killed_is_removed_by_the_next_with_its_host_runner starts and kills it"]
fn a_killed_test() {
    use std::os::unix::fs::PermissionsExt;

    let world = World::new();
    world.afk(&["run"]).ok();
    let copy = world.cache().join("afkconvoy/copies/acme/team/a-commit");
    std::fs::create_dir_all(&copy).unwrap();
    std::fs::write(copy.join("CLAUDE.md"), "rules\n").unwrap();
    for dir in [&copy, copy.parent().unwrap()] {
        std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    }
    println!("left: {} {}", world.root.display(), world.host_runner().unwrap());
    std::thread::sleep(std::time::Duration::from_secs(60));
}
