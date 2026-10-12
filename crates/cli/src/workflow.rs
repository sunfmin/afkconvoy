//! `show` and `rehearse`: a loaded team's workflow drawn, and walked through
//! a list of verdicts with no worker started. Both only put into words what
//! the core decides (`afkconvoy_core::graph`, `afkconvoy_core::position`).

use afkconvoy_core::graph;
use afkconvoy_core::position::{Declared, Place, Position};
use afkconvoy_core::team::{HUMAN, Workflow};
use anyhow::{Result, bail};
use serde_json::{Map, Value, json};

use crate::team::Loaded;

/// The workflow of the team that is named `name`.
fn named<'t>(loaded: &'t Loaded, name: &str) -> Result<&'t Workflow> {
    let Some(workflow) = loaded.team.workflow(name) else {
        let has: Vec<&str> = loaded.team.workflows.iter().map(|w| w.name.as_str()).collect();
        if has.is_empty() {
            bail!("the team {} has no workflow `{name}`: it has none", loaded.name);
        }
        bail!("the team {} has no workflow `{name}`; it has: {}", loaded.name, has.join(", "));
    };
    Ok(workflow)
}

/// The drawing of the workflow named `name`, or of every workflow of the
/// team, a blank line between two.
pub fn show(loaded: &Loaded, name: Option<&str>) -> Result<String> {
    let drawn: Vec<String> = match name {
        Some(name) => vec![graph::draw(named(loaded, name)?)],
        None => loaded.team.workflows.iter().map(graph::draw).collect(),
    };
    if drawn.is_empty() {
        bail!("the team {} has no workflow to draw", loaded.name);
    }
    Ok(drawn.join("\n"))
}

/// Where an issue of the workflow named `name` is after each of `declared`,
/// and where it ends.
pub fn rehearse(loaded: &Loaded, name: &str, declared: &[Declared]) -> Result<Value> {
    let workflow = named(loaded, name)?;
    let mut place = Place::enter(workflow);
    let enters = position(workflow, place.position);
    let mut steps: Vec<Value> = vec![];
    for declared in declared {
        let stage = place.position.stage().map(|stage| stage.name.as_str());
        let valid = place.takes(*declared);
        let next = place.after(*declared);
        // A breaker is tripped by what was declared, not by what follows it.
        let breaker = match next.position.breaker() {
            Some(breaker) if valid => Some(breaker.key()),
            _ => None,
        };
        let mut in_a_row = Map::new();
        for stage in workflow.stages().iter().filter(|stage| stage.reject().is_some()) {
            in_a_row.insert(stage.name.clone(), json!(next.consecutive_rework(stage)));
        }
        steps.push(json!({
            "declared": declared.word(),
            "stage": stage,
            "valid": valid,
            "breaker": breaker,
            "position": position(workflow, next.position),
            "rework": next.rework(),
            "consecutive_rework": in_a_row,
        }));
        place = next;
    }
    Ok(json!({
        "workflow": workflow.name,
        "max_rework": workflow.max_rework,
        "max_consecutive_rework": workflow.max_consecutive_rework,
        "enters": enters,
        "steps": steps,
        "ends": position(workflow, place.position),
    }))
}

/// A position as a command answers it: what it is, at which stage, and who
/// acts on it.
pub(crate) fn position(workflow: &Workflow, position: Position) -> Value {
    let is = match position {
        Position::Perform(..) => "perform",
        Position::Waiting(_) => "waiting",
        Position::Consult(..) => "consult",
        Position::Parked(_) => "parked",
        Position::Landing => "landing",
        Position::Ended => "ended",
        Position::Escalating(..) => "escalating",
    };
    let worker = match position {
        Position::Landing => workflow.land.as_ref(),
        _ => position.worker(),
    };
    let by = match position {
        Position::Waiting(_) | Position::Escalating(..) => Some(HUMAN),
        _ => worker.map(|work| work.role.as_str()),
    };
    let reason = match position {
        Position::Escalating(stage, reason) => Some(reason.said(stage)),
        _ => None,
    };
    json!({
        "is": is,
        "stage": position.stage().map(|stage| stage.name.as_str()),
        "by": by,
        "do": worker.map(|work| work.skill.as_str()),
        "reason": reason,
    })
}
