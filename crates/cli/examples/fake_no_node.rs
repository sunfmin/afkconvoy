//! Fault injection at the platform-command boundary. Normal execution uses
//! the real macOS log reader; faults never fabricate a successful monitor.
use std::process::{Command, ExitCode};

use serde_json::{Value, json};

fn main() -> ExitCode {
    let path = std::env::var_os("NO_NODE_FAKE_STATE").expect("test's state path");
    let mut state: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    state["calls"].as_array_mut().unwrap().push(json!(args));
    std::fs::write(path, serde_json::to_vec(&state).unwrap()).unwrap();
    match state["mode"].as_str().unwrap() {
        "empty" => println!("[]"),
        "error" => {
            eprintln!("injected unified log read failure");
            return ExitCode::FAILURE;
        }
        "malformed" => println!("not JSON"),
        "sandbox" => {
            eprintln!("injected sandbox startup failure");
            return ExitCode::FAILURE;
        }
        mode => {
            let out = Command::new("/usr/bin/log").args(args).output().unwrap();
            assert!(out.status.success());
            let mut events: Vec<Value> = serde_json::from_slice(&out.stdout).unwrap();
            if !events.is_empty() {
                match mode {
                    "missing-before" => {
                        events.remove(0);
                    }
                    "missing-after" => {
                        events.pop();
                    }
                    _ => panic!("unknown fault {mode}"),
                }
            }
            println!("{}", json!(events));
        }
    }
    ExitCode::SUCCESS
}
