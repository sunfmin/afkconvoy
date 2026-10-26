//! Every test of the CLI, as one program: linked once, not once a file.
//! A file in this directory is a test only once it is a module here, which
//! `every_file_here_is_a_module` holds to.

mod common;

mod abandon;
mod admission;
mod board;
mod casting;
mod check;
mod checkout;
mod checkouts;
mod claims;
mod codex;
mod config;
mod consult;
mod continuation;
mod cut_train;
mod cutting;
mod declared;
mod dispatch;
mod doctor;
mod dsh;
mod exclusive;
mod fixture;
mod flows;
mod gate;
mod goodbye;
mod guard;
mod help;
mod host_runner;
mod host_sleep;
mod host_watch;
mod hot;
mod inspected;
mod intake;
mod joining;
mod kimi;
mod lander;
mod landing;
mod launch;
mod lock;
mod logged;
mod manual;
mod outside;
mod panel;
mod park;
mod patrol;
mod pi;
mod plan;
mod precedent;
mod probe;
mod proxy;
mod public_tracker;
mod rebuild;
mod rehearse;
mod release;
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
mod stock;
mod stopped;
mod switch;
mod takeover;
mod tend;
mod tick;
mod tmpdir;
mod tools;
mod tracker;
mod train;
mod unreached;
mod wake;
mod walk;

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
