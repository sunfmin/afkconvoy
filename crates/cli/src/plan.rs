//! `afkconvoy plan`: what a tick would act on, read in one look at the
//! fleet, its host and its team, with the startup probe of this host.

use afkconvoy_core::claims::InstanceId;
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::git::Repo;
use crate::launch::Launches;
use crate::{admission, blockers, codex_launch, fleet_report, kit, probe};

/// The plan of `repo` at `now`, with `instance`'s claims its own and the
/// workers' `no_proxy` hosts as the command line names them.
pub(crate) fn plan(
    repo: &Repo,
    now: u64,
    instance: Option<&InstanceId>,
    no_proxy: &[String],
) -> anyhow::Result<Value> {
    let launches = Launches::foreseen(repo).with_no_proxy(no_proxy)?;
    let warnings = match probe::standing(repo, now, &launches) {
        Some(warnings) => warnings,
        None => probe::startup(repo, now, &launches)?,
    };
    probe::announce(&warnings);
    // `plan` is one look: its host reading is the report's.
    let readings = admission::Readings::new();
    let (mut fleet, open) = Fleet::read(repo, now)?;
    let (mut set, flows) = fleet.rebuild(&open, instance)?;
    let report = fleet_report::FleetReport::read(&fleet, &flows, &set, &readings);
    if let Ok(admission) = &report.admission {
        set.held_back = admission.holds();
    }
    let findings = blockers::findings(&set.unworked_blockers);
    let mut plan = serde_json::to_value(set)?;
    plan["findings"] = serde_json::to_value(findings)?;
    if let Some(plan) = plan.as_object_mut() {
        plan.extend(report.plan_json()?);
    }
    plan["need_installs"] = json!(
        probe::installs(&fleet.repo, &fleet.config)
            .into_iter()
            .map(|(kind, installed)| json!({
                "kind": kind,
                "check": installed.check.name,
                "at": installed.at,
                "seconds": installed.seconds,
                "status": installed.status,
                "passed": installed.passed,
                "stderr": installed.stderr,
            }))
            .collect::<Vec<_>>()
    );
    plan["targets"] = fleet.targets(&open.issues)?;
    plan["host_kit"] = kit::show(now);
    plan["selection"] = fleet.selection();
    plan["codex_workers"] = json!(codex_launch::workers(&fleet.repo));
    report.put_launches(&mut plan["frontier"]);
    if let Ok(team) = flows.team() {
        plan["selection"]["roles"] =
            json!(team.roles.iter().map(|role| fleet.role_selection(role)).collect::<Vec<_>>());
    }
    plan["worker_no_proxy"] = launches.proxy_json();
    Ok(plan)
}
