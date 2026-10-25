//! Local verification of the real, tool-free diagnosis launch chain.
//! Input is recorded evidence JSON; this never performs a continuation.
use anyhow::{Context, Result};
use serde_json::{Value, json};

fn main() -> Result<()> {
    let path = std::env::args_os().nth(1).context("usage: diagnose_worker <evidence.json>")?;
    let evidence: Value = serde_json::from_slice(&std::fs::read(path)?)?;
    let kit = afkconvoy::kit::running()?.0;
    let (harness, answer) = afkconvoy::health::diagnose(&kit, &evidence)?;
    println!("{}", json!({"harness": harness, "answer": answer, "action_performed": false}));
    Ok(())
}
