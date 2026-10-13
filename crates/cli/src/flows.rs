//! `afkconvoy flows`: what whoever hands the fleet work needs to open an issue
//! the fleet will work — each workflow of the team and what it is for, the
//! labels to keep off an issue, and the line that opens one — with the labels
//! that line names created on the repo where they are missing. In an unbound
//! repo the one label is the ready label.

use afkconvoy_core::config::Config;
use afkconvoy_core::line::Line;
use afkconvoy_core::team::{DONE, HUMAN, LAND, Pass, Performer, Team, Workflow};
use anyhow::Result;
use serde_json::{Value, json};

use crate::config;
use crate::gh::GitHub;
use crate::git::Repo;
use crate::intake::open_taken_in;
use crate::team;

/// What GitHub shows beside a flow label this command created, and beside a
/// ready label.
const FLOW_LABEL_SAYS: &str = "afkconvoy: the workflow an issue that carries this follows";
const READY_LABEL_SAYS: &str = "afkconvoy: the fleet works an issue that carries this";

/// The line that opens an issue carrying `label`, its title and body left to
/// whoever runs it.
fn line(label: &str) -> String {
    let line = Line::new().fixed("gh issue create --label ").word(label);
    line.fixed(" --title '<title>' --body-file <file>").own()
}

/// Creates each of `labels` the repo lacks, as one that says `says` → the
/// ones created. GitHub tells labels apart by their letters, not their case.
fn create_missing(gh: &GitHub, labels: &[String], says: &str) -> Result<Vec<String>> {
    let has = gh.labels()?;
    let mut created = vec![];
    for label in labels {
        if !has.iter().any(|had| had.eq_ignore_ascii_case(label)) {
            gh.create_label(label, says)?;
            created.push(label.clone());
        }
    }
    Ok(created)
}

/// One workflow, as whoever opens an issue into it reads it.
fn workflow(team: &Team, workflow: &Workflow) -> Value {
    let first = workflow.first();
    let (by, skill) = match &first.by {
        Performer::Human => (HUMAN, None),
        Performer::Role(work) => (team.role_of(work).name.as_str(), Some(&work.skill)),
    };
    // Every stage is one an issue can come to, so every end a pass leads to
    // is one the workflow has.
    let leads_to =
        |end: fn(&Pass) -> bool| workflow.stages().iter().any(|stage| end(&stage.pass()));
    let mut ends = vec![];
    if leads_to(|pass| matches!(pass, Pass::Land)) {
        ends.push(LAND);
    }
    if leads_to(|pass| matches!(pass, Pass::Done)) {
        ends.push(DONE);
    }
    let flow_label = workflow.flow_label();
    json!({
        "name": workflow.name,
        "flow_label": flow_label,
        "description": workflow.description,
        "first": {"stage": first.name, "by": by, "do": skill},
        "ends": ends,
        "line": line(&flow_label),
    })
}

/// The labels that keep an issue from being worked, with `escalate_label` the
/// one the fleet swaps in: the team's in a bound repo, the config's in an
/// unbound one.
fn keep_off(config: &Config, escalate_label: &str) -> Value {
    json!({"epic_labels": config.epic_labels, "escalate_label": escalate_label,
        "lock_label": config.lock_label})
}

/// `afkconvoy flows`, in the checkout `repo`. Everything is read before a
/// label is created, so a team with a fault writes nothing.
///
///   {"team", "workflows": [{"name", "flow_label", "description", "first", "ends", "line"}],
///    "keep_off", "intake": {"open", "limit"}, "created"}
///   {"team": null, "ready_label", "line", "keep_off", "created"}
pub fn flows(repo: &Repo) -> Result<Value> {
    let config = config::load(&repo.dir.join(config::FILE))?;
    let gh = GitHub { dir: repo.dir.clone() };
    let Some(bound) = &config.bound else {
        let created =
            create_missing(&gh, std::slice::from_ref(&config.ready_label), READY_LABEL_SAYS)?;
        return Ok(json!({"team": null, "ready_label": config.ready_label,
            "line": line(&config.ready_label),
            "keep_off": keep_off(&config, &config.escalate_label), "created": created}));
    };
    let team = team::load(team::From::Config)?.team;
    let open = open_taken_in(&gh.open_issues()?);
    let flow_labels: Vec<String> = team.workflows.iter().map(Workflow::flow_label).collect();
    let created = create_missing(&gh, &flow_labels, FLOW_LABEL_SAYS)?;
    Ok(json!({
        "team": bound.team,
        "workflows": team.workflows.iter().map(|w| workflow(&team, w)).collect::<Vec<_>>(),
        "keep_off": keep_off(&config, &team.escalate_label),
        "intake": {"open": open, "limit": bound.intake_limit},
        "created": created,
    }))
}
