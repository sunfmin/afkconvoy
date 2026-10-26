use crate::common::{World, git};

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};

use serde_json::Value;

fn dispatch(world: &World, issue: u64) -> PathBuf {
    world.issue(issue, &["ready-for-agent"]);
    let started =
        world.afk(&["dispatch", &issue.to_string(), "--instance", "alpha", "--host", "h1"]).ok();
    PathBuf::from(started["worktree"].as_str().unwrap())
}

/// A real process whose environment ps can read, standing in for a shared
/// server. Reaped even if the regression assertion fails.
struct Server(Child);

impl Server {
    fn start(tmp: &Path) -> Self {
        Self(
            Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tmpdir::a_server", "--ignored"])
                .env("TMPDIR", tmp)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .spawn()
                .unwrap(),
        )
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "the TMPDIR lifecycle tests start this as a shared server"]
fn a_server() {
    std::thread::sleep(std::time::Duration::from_secs(60));
}

/// The symptom of the incident, a cache hit included: a server must still
/// be able to create a temporary directory through its original link.
fn can_compile(tmp: &Path) {
    let compile = tempfile::Builder::new().prefix("sccache").tempdir_in(tmp);
    assert!(compile.is_ok(), "server lost its TMPDIR {}: {compile:?}", tmp.display());
}

#[test]
fn a_failed_process_listing_never_removes_a_live_servers_tmpdir() {
    for answer in ["exit 1", "exit 0", "echo an-unreadable-process-list"] {
        let world = World::new();
        let worktree = dispatch(&world, 7);
        let tmp = world.tmp(&worktree);
        let mut server = Server::start(&tmp);
        can_compile(&tmp);
        git(&world.clone, &["worktree", "remove", "--force", worktree.to_str().unwrap()]);

        let ps = world.root.join("bin/ps");
        std::fs::write(&ps, format!("#!/bin/sh\n{answer}\n")).unwrap();
        std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).unwrap();
        dispatch(&world, 8); // make reclaims old links, best effort
        assert!(server.0.try_wait().unwrap().is_none(), "the server still runs");
        assert!(tmp.symlink_metadata().is_ok(), "{answer}: removed a live server's link");

        std::fs::remove_file(ps).unwrap();
        dispatch(&world, 9); // a later successful observation repairs it
        can_compile(&tmp);
        drop(server);
        dispatch(&world, 10);
        assert!(tmp.symlink_metadata().is_err(), "reclaimed once the server ends");
    }
}

#[test]
fn retiring_a_link_outlives_the_callers_temporary_directory() {
    let world = World::new();
    let worktree = dispatch(&world, 7);
    let tmp = world.tmp(&worktree);
    let _server = Server::start(&tmp);
    let caller = world.root.join("the-callers-temporary-directory");
    std::fs::create_dir(&caller).unwrap();
    world.close_issue(7);

    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["tick", "--instance", "alpha", "--host", "h1"])
        .env("TMPDIR", &caller)
        .output()
        .unwrap();
    let tick: Value = serde_json::from_slice(&out.stdout).unwrap();
    assert!(out.status.success(), "{tick}");
    assert_eq!(tick["errors"], serde_json::json!([]));
    assert!(!worktree.exists(), "settling removed the worktree");
    std::fs::remove_dir_all(&caller).unwrap();
    can_compile(&tmp);
}

#[test]
fn a_test_reclaims_only_its_own_links_and_repairs_to_durable_storage() {
    let other = World::new();
    let other_worktree = dispatch(&other, 7);
    let other_tmp = other.tmp(&other_worktree);
    git(&other.clone, &["worktree", "remove", "--force", other_worktree.to_str().unwrap()]);
    let original = std::fs::read_link(&other_tmp).unwrap();

    let world = World::new();
    let worktree = dispatch(&world, 7);
    let tmp = world.tmp(&worktree);
    assert_ne!(tmp, afkconvoy::worker_tmp::of(&worktree), "outside the host's real links");
    let _server = Server::start(&tmp);
    git(&world.clone, &["worktree", "remove", "--force", worktree.to_str().unwrap()]);
    let caller = world.root.join("the-reclaimers-temporary-directory");
    std::fs::create_dir(&caller).unwrap();
    world.issue(8, &["ready-for-agent"]);
    let out = world
        .command(env!("CARGO_BIN_EXE_afkconvoy"), &world.clone)
        .args(["dispatch", "8", "--instance", "alpha", "--host", "h1"])
        .env("TMPDIR", &caller)
        .output()
        .unwrap();
    assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stdout));
    std::fs::remove_dir_all(caller).unwrap();
    can_compile(&tmp);
    assert_eq!(
        std::fs::read_link(other_tmp).unwrap(),
        original,
        "another test's link is untouched"
    );
}
