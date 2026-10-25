//! The real sampler and inspector against a host whose pressure and process
//! readings are controlled. No process of the test host is counted.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use afkconvoy::log::{Entry, What};
use common::{Running, World};
use serde_json::json;

use crate::common;

const SOON: Duration = Duration::from_secs(40);

fn until(world: &World, what: &str, done: impl Fn() -> bool) {
    let until = Instant::now() + SOON;
    while !done() {
        assert!(Instant::now() < until, "{what} never happened: {:#?}", entries(world));
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn entries(world: &World) -> Vec<Entry> {
    let runs = world.cache().join("afkconvoy/runs");
    let Ok(files) = std::fs::read_dir(runs) else { return vec![] };
    files
        .flatten()
        .filter(|file| file.path().extension().is_some_and(|ext| ext == "jsonl"))
        .flat_map(|file| {
            let text = std::fs::read_to_string(file.path()).unwrap();
            text.lines()
                .filter_map(|line| serde_json::from_str::<Entry>(line).ok())
                .collect::<Vec<_>>()
        })
        .collect()
}

#[cfg(target_os = "macos")]
fn started(world: &World) -> usize {
    entries(world).iter().filter(|entry| matches!(entry.what, What::HostInspecting { .. })).count()
}

fn pressure(world: &World, level: u64) {
    std::fs::write(world.root.join("host-pressure"), level.to_string()).unwrap();
}

fn worker(world: &World, issue: u64) -> PathBuf {
    world.issue(issue, &["ready-for-agent"]);
    let started = world
        .afk(&[
            "dispatch",
            &issue.to_string(),
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            "1000",
        ])
        .ok();
    PathBuf::from(started["worktree"].as_str().unwrap())
}

fn processes(world: &World, workers: &[(&Path, u64)]) {
    // An outside process holds 10 GiB, while workers hold only their own
    // listed memory. Fake pids cannot be sampled or signalled on the host.
    let mut rows = "999990 2 10485760 0:00.00 outside-memory-holder\n".to_string();
    for (index, (path, mib)) in workers.iter().enumerate() {
        let tmp = afkconvoy::worker_tmp::of(path);
        rows.push_str(&format!(
            "{} 2 {} 0:00.00 worker TMPDIR={}\n",
            999980 + index,
            mib * 1024,
            tmp.display()
        ));
    }
    // Rename makes every observation one complete reading.
    std::fs::write(world.root.join("host-processes-next"), rows).unwrap();
    std::fs::rename(world.root.join("host-processes-next"), world.root.join("host-processes"))
        .unwrap();
}

fn host(world: &World) {
    use std::os::unix::fs::PermissionsExt;
    let ps = world.root.join("bin/ps");
    std::fs::write(&ps, "#!/bin/sh\ncase \"$*\" in axeww*) echo sample >> \"${0%/*}/../ps-samples\" ;; esac\ncat \"${0%/*}/../host-processes\"\n").unwrap();
    std::fs::set_permissions(&ps, std::fs::Permissions::from_mode(0o755)).unwrap();
    processes(world, &[]);
    world.say_host_settings(json!({"hot_memory": 1, "admit_inspect_after": 0}));
}

fn samples(world: &World) -> usize {
    std::fs::read_to_string(world.root.join("ps-samples")).unwrap_or_default().lines().count()
}

fn run(world: &World) -> Running {
    world.running(&["--instance", "alpha", "--host", "h1", "--now", "1000", "--second-ms", "10"])
}

fn inspection(world: &World) -> PathBuf {
    world
        .worktrees()
        .into_iter()
        .find(|path| path.file_name().unwrap().to_string_lossy().contains("afkconvoy-inspection"))
        .expect("inspection checkout")
}

fn finish(world: &World) {
    world
        .afk_in(
            &inspection(world),
            &["inspected", "--text", "The outside process still holds memory; no runaway worker."],
        )
        .ok();
}

#[cfg(target_os = "macos")]
#[test]
fn pressure_inspects_the_host_once_across_fleets_and_never_heats_the_largest_worker() {
    let world = World::new();
    host(&world);
    let a = worker(&world, 7);
    let b = worker(&world, 8);
    processes(&world, &[(&a, 620), (&b, 170)]);
    pressure(&world, 2);
    world
        .afk(&["run", "--instance", "alpha", "--host", "h1", "--now", "1000", "--second-ms", "10"])
        .ok();
    until(&world, "host inspection", || started(&world) == 1);
    let brief = common::git(&inspection(&world), &["rev-parse", "--absolute-git-dir"]);
    let brief = std::fs::read_to_string(Path::new(&brief).join("convoy-worker-prompt.md")).unwrap();
    assert!(brief.contains("the host has been under memory pressure"), "{brief}");
    assert!(brief.contains("admission may already admit"), "{brief}");
    let evidence =
        std::fs::read_to_string(world.cache().join("afkconvoy/host-inspection/evidence.md"))
            .unwrap();
    assert!(evidence.contains("memory pressure (level 2)"), "{evidence}");
    assert!(
        evidence.contains("process tree of each worker and of the landing train"),
        "{evidence}"
    );
    assert!(evidence.contains("outside-memory-holder"), "{evidence}");

    finish(&world);
    until(&world, "host inspection ended", || {
        entries(&world).iter().any(
            |entry| matches!(&entry.what, What::HostInspected { found, .. } if found == "said"),
        )
    });
    // A fleet added after the inspection shares the host's existing episode,
    // even though its own first observation is later than that inspection.
    let beta = world.second_repo();
    world
        .afk_in(
            &beta,
            &["run", "--instance", "beta", "--host", "h1", "--now", "1001", "--second-ms", "10"],
        )
        .ok();
    // Changing the largest worker cannot create another episode.
    processes(&world, &[(&a, 170), (&b, 620)]);
    let before = samples(&world);
    until(&world, "further worker samples", || samples(&world) >= before + 6);
    assert_eq!(started(&world), 1);
    assert!(
        !entries(&world)
            .iter()
            .any(|entry| matches!(entry.what, What::Hot { .. } | What::Inspecting { .. }))
    );
    let plan = world.afk(&["plan", "--now", "1000"]).ok();
    assert_eq!(plan["admission"]["admits"], true, "{plan}");
    assert_eq!(plan["admission"]["reading"]["memory_free_percent"], 80);
}

#[test]
fn a_worker_past_its_own_memory_limit_is_hot_under_pressure_or_without_it() {
    for level in [1, 2] {
        let world = World::new();
        host(&world);
        let worker = worker(&world, 7);
        processes(&world, &[(&worker, 1200)]);
        pressure(&world, level);
        let _run = run(&world);
        until(&world, "worker memory heat", || {
            entries(&world).iter().any(|entry|
            matches!(&entry.what, What::Hot { worker, why, .. } if worker == "#7" && why.starts_with("memory:")))
        });
    }
}

#[cfg(target_os = "macos")]
#[test]
fn pressure_coverage_survives_an_inspection_for_another_reason_and_a_cache_upgrade() {
    let memory = json!({"repo": "/another-fleet", "reason": "memory", "reading": "pressure level 2",
        "by": "stock-consult", "started": 1000, "ended": 1001, "found": "said", "said": "outside process"});
    let mut load = memory.clone();
    load["reason"] = json!("load");
    load["started"] = json!(1200);
    load["ended"] = json!(1300);
    for prior in [memory.clone(), json!([memory, load])] {
        let world = World::new();
        host(&world);
        pressure(&world, 2);
        let last = world.cache().join("afkconvoy/host-inspection/last.json");
        std::fs::create_dir_all(last.parent().unwrap()).unwrap();
        std::fs::write(&last, prior.to_string()).unwrap();
        world.afk_lines(&[
            "run",
            "--foreground",
            "--instance",
            "alpha",
            "--host",
            "h1",
            "--now",
            "1000",
            "--second-ms",
            "0",
            "--ticks",
            "2",
        ]);
        assert_eq!(started(&world), 0);
        assert!(entries(&world).iter().any(|entry|
            matches!(&entry.what, What::HostInspected { found, .. } if found == "covered")), "{:#?}", entries(&world));
    }
}

#[test]
fn a_hot_worker_cooling_behind_a_running_inspection_is_never_inspected() {
    let world = World::new();
    host(&world);
    let a = worker(&world, 7);
    let b = worker(&world, 8);
    processes(&world, &[(&a, 1200), (&b, 100)]);
    let _run = run(&world);
    until(&world, "first worker inspection", || {
        entries(&world)
            .iter()
            .any(|entry| matches!(&entry.what, What::Inspecting { worker, .. } if worker == "#7"))
    });
    processes(&world, &[(&a, 1200), (&b, 1200)]);
    until(&world, "second worker hot and waiting", || {
        entries(&world)
            .iter()
            .any(|entry| matches!(&entry.what, What::Hot { worker, .. } if worker == "#8"))
    });
    let release = world.root.join("release-inspection-comment");
    world.with_gh(|gh| gh["comment_wait"] = json!(release));
    finish(&world);
    until(&world, "ending the first inspection", || release.with_extension("waiting").exists());
    processes(&world, &[(&a, 1200), (&b, 100)]);
    until(&world, "waiting worker cooled while the first inspection ends", || {
        entries(&world)
            .iter()
            .any(|entry| matches!(&entry.what, What::Cooled { worker, .. } if worker == "#8"))
    });
    std::fs::write(&release, "").unwrap();
    until(&world, "first inspection ended", || {
        entries(&world).iter().any(|entry|
        matches!(&entry.what, What::Inspected { worker, found, .. } if worker == "#7" && found == "said"))
    });
    let before = samples(&world);
    until(&world, "further samples after the turn is free", || samples(&world) >= before + 3);
    assert!(
        !entries(&world)
            .iter()
            .any(|entry| matches!(&entry.what, What::Inspecting { worker, .. } if worker == "#8")),
        "{:#?}",
        entries(&world)
    );
}
