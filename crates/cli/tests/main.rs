//! Every test of the CLI, as one program: linked once, not once a file.
//! A file in this directory is a test only once it is a module here, which
//! `every_file_here_is_a_module` holds to.

mod common;

mod accounts;
mod admission;
mod asking;
mod board;
mod candidates;
mod casting;
mod check;
mod checkout;
mod checkouts;
mod claims;
mod codex;
mod config;
mod consult;
mod continuation;
mod cutting;
mod declared;
mod delivery;
mod dispatch;
mod doctor;
mod dsh;
mod escalation;
mod exclusive;
mod first_start;
mod fixture;
mod flows;
mod gate;
mod goodbye;
mod grok;
mod guard;
mod help;
mod host_runner;
mod host_watch;
mod intake;
mod joining;
mod kimi;
mod kit_research;
mod lander;
mod launch;
mod lock;
mod logged;
mod migration;
mod named_harness;
mod no_node;
mod outside;
mod panel;
mod park;
mod patrol;
mod pi;
mod plan;
mod probe;
mod proxy;
mod public_tracker;
mod queue;
mod quota;
mod rebuild;
mod rehearse;
mod release;
mod retired_proposals;
mod role;
mod roster;
mod scaffold;
mod slow_gate;
mod smoke;
mod smoke_claude_skills;
mod smoke_codex;
mod smoke_kimi;
mod smoke_pi;
mod split;
mod stage;
mod steer;
mod stop_release;
mod stopped;
mod switch;
mod takeover;
mod target;
mod tick;
mod tmpdir;
mod tools;
mod tracker;
mod turns;
mod unreached;
mod wake;
mod walk;
mod watch_landing;

/// With `autotests = false`, a file here that is no module is never built:
/// its tests would pass by not running.
#[test]
fn every_file_here_is_a_module() {
    let here = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests");
    let main = std::fs::read_to_string(here.join("main.rs")).unwrap();
    for entry in std::fs::read_dir(&here).unwrap() {
        let name = entry.unwrap().file_name().into_string().unwrap();
        let Some(module) = name.strip_suffix(".rs") else { continue };
        if module != "main" {
            assert!(
                main.contains(&format!("\nmod {module};\n")),
                "tests/{name} is not a module of tests/main.rs"
            );
        }
    }
}
