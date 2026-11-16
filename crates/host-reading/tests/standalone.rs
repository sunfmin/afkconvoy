//! The host reading crate as another program uses it: through its public API
//! alone, beside the gate crate and no fleet (docs/adr/0089).

use afkconvoy_gate::exclusive::{By, Reading};
use afkconvoy_host_reading::{Minute, cpu};

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_secs()
}

/// An embedder of the gate crate's holder says its host's reading from this
/// crate alone: ticks over the last minute it keeps, and memory under the
/// floor it names.
#[test]
fn an_embedder_reads_its_host_for_the_queue() {
    let mut minute = Minute::default();
    let first = now();
    let read: Reading =
        afkconvoy_host_reading::reading(&mut minute, first, 101).expect("this host is read");
    assert!(read.busy <= 100, "{read:?}");
    assert!(read.since <= read.taken && read.taken == first, "{read:?}");
    assert!(read.taken - read.since <= cpu::WINDOW.as_secs(), "{read:?}");
    assert_eq!(read.memory.map(|short| short.floor), Some(101), "all free is under 101%");
    if std::env::var_os("AFKCONVOY_FAKE_CPU_BUSY").is_none() {
        assert!(cpu::ticks().is_some(), "the kernel says its ticks");
    }
    let unfloored = afkconvoy_host_reading::reading(&mut minute, first + 1, 0).unwrap();
    assert!(matches!(unfloored.by, By::Ticks | By::Load) && unfloored.memory.is_none());
    assert!(afkconvoy_host_reading::cores() >= 1);
}

/// What the crate depends on: the gate crate, for the reading it says, and
/// libc — never afkconvoy-core, whose fleet concepts its API is to stay free
/// of.
#[test]
fn the_crate_depends_on_nothing_of_the_fleet() {
    let manifest = include_str!("../Cargo.toml");
    let mut names: Vec<&str> = manifest
        .split("dependencies]")
        .skip(1)
        .flat_map(|section| section.split("\n[").next().unwrap().lines())
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once(['=', '.']).map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["afkconvoy-gate", "libc"]);
}

/// The crate's public API names no fleet concept.
#[test]
fn its_public_api_names_no_fleet_concept() {
    const FLEET: [&str; 9] =
        ["team", "role", "stage", "issue", "claim", "patrol", "intake", "tracker", "github"];
    for source in [include_str!("../src/lib.rs"), include_str!("../src/cpu.rs")] {
        let public = source.split("#[cfg(test)]").next().unwrap();
        for line in public.lines().map(str::trim).filter(|line| line.starts_with("pub ")) {
            let lower = line.to_lowercase();
            for word in lower.split(|c: char| !c.is_ascii_alphanumeric()) {
                for concept in FLEET {
                    assert!(
                        !word.starts_with(concept),
                        "`{line}` names a fleet concept: {concept}"
                    );
                }
            }
        }
    }
}
