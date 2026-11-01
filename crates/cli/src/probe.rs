//! What `run`, `tick` and `plan` check before anything else: a host or a repo
//! the fleet cannot work is refused here, not at its first tick or landing.
//!
//! A probe that passed is noted on the host that made it (`Passed`). `run`
//! probes at every start; `tick` and `plan` trust a note that still stands
//! (`standing`) and probe only without one.
//!
//! A check that found the checkout's tracker private is noted beside it
//! (`Private`): a running fleet instance asks again once it no longer stands
//! (`public_tracker`), and a probe stands only while it does.

use std::path::Path;

use afkconvoy_core::config::{Config, GateNeed};
use afkconvoy_core::gate::outlived;
use afkconvoy_core::layout::{
    Answer, BaseCheck, GATE_KIND, Layout, PROBE_STANDS, answer, base_check, no_base_branch,
    no_gate_records, probe_stands, settle,
};
use afkconvoy_core::tools::{Found, Tool};
use afkconvoy_core::tracker::{Tracker, is_public, public};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use crate::checkout_list::{named_repo, tracker_of};
use crate::config;
use crate::gh::GitHub;
use crate::git::{REMOTE, Repo};
use crate::launch::Launches;
use crate::remote::{Fresh, Push};
use crate::role;
use crate::tools;
use crate::topic::{Explain, See};

/// What a probe's commit says it is.
const PROBE_RECORD: &str = "convoy:probe";

/// The note of the last startup probe that passed (`Repo::note`).
const PASSED_NOTE: &str = "convoy-probe";

/// The note of the last check that found the checkout's tracker private
/// (`Private`).
const TRACKER_NOTE: &str = "convoy-tracker";
const NEEDS_NOTE: &str = "convoy-gate-needs";

#[derive(Serialize, Deserialize)]
struct NeedsPassed {
    at: u64,
    checks: Vec<GateNeed>,
}

/// A check's output is private to its caller. Running instances report only
/// the name and the command to fix it, never output in their tick log.
fn gate_needs(
    repo: &Repo,
    config: &Config,
    now: u64,
    trusting: bool,
) -> Result<Option<String>> {
    let note = repo.note(NEEDS_NOTE)?;
    if trusting {
        let passed = std::fs::read_to_string(&note)
            .ok()
            .and_then(|text| serde_json::from_str::<NeedsPassed>(&text).ok());
        if passed.is_some_and(|p| p.checks == config.gate_needs && probe_stands(p.at, now)) {
            return Ok(None);
        }
    }
    let _ = std::fs::remove_file(&note);
    for check in &config.gate_needs {
        use std::hash::{Hash, Hasher};
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        use std::io::Write;
        let mut hash = std::collections::hash_map::DefaultHasher::new();
        repo.common_dir()?.hash(&mut hash);
        check.name.hash(&mut hash);
        let dir = crate::copies::cache()?.join("gate-needs");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join(format!("{:016x}.stderr", hash.finish()));
        let script = format!("exec 1>/dev/null\n{}", check.command);
        let mut command = std::process::Command::new("sh");
        command.args(["-e", "-c", &script]).current_dir(&repo.dir);
        let (output, timed_out) = crate::process::output_within_with_timeout(
            &mut command, std::time::Duration::from_secs(check.seconds),
        )?;
        // A successful next run also clears the previous failure's output.
        if !timed_out && output.status.success() {
            let _ = std::fs::remove_file(&path);
            continue;
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let lines: Vec<_> = stderr.lines().collect();
        let tail = lines[lines.len().saturating_sub(20)..].join("\n");
        let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&path)?;
        file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        file.write_all(tail.as_bytes())?;
        let reason = if timed_out { format!("timed out after {}s", check.seconds) } else { output.status.to_string() };
        return Ok(Some(format!("gate need {} failed ({reason}); stderr: {}; fix it in {}: {}", check.name, path.display(), repo.dir.display(), check.command)));
    }
