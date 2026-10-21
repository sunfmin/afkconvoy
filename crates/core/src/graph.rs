//! A workflow drawn as a graph, in plain text: one row per stage in the order
//! its file lists them, each `pass` named under its stage, and each `reject`
//! a line up the left margin to the stage it leads back to. It reads the same
//! in a terminal and in a code block of a GitHub comment.

use flux_rs::attrs::*;

use crate::harness::Harness;
use crate::position::Breaker;
use crate::team::{Consult, DONE, HUMAN, LAND, Pass, Performer, Stage, Team, Work, Workflow};

/// One `reject`: from the stage at `from` back to the one at `to`.
struct Back {
    from: usize,
    to: usize,
}

/// The drawing of `workflow`, one of `team`, in a repo whose default harness
/// is `default`: lines, each ended by a newline. A stage a worker performs
/// says its harness, named by it or not.
#[spec(fn(&Team[@roles], &Workflow{v: v.roles == roles}, Harness) -> String)]
pub fn draw(team: &Team, workflow: &Workflow, default: Harness) -> String {
    let stages = workflow.stages();
    // The reject of a later stage is drawn further out, so one that spans
    // another goes round it.
    let mut backs: Vec<Back> = vec![];
    for stage in stages.iter().rev() {
        if let Some(to) = stage.reject() {
            backs.push(Back { from: stage.at(), to });
        }
    }
    let ends_in_nothing = stages.iter().any(|stage| stage.pass() == Pass::Done);
    // Every name drawn in the column of stage names: the ends are rows too.
    let mut width = 0;
    for stage in stages {
        width = width.max(stage.name.chars().count());
    }
    if workflow.land.is_some() {
        width = width.max(LAND.len());
    }
    if ends_in_nothing {
        width = width.max(DONE.len());
    }

    let mut out = format!(
        "{} — {} — {} {}, {} {}\n\n",
        workflow.name,
        workflow.flow_label(),
        Breaker::Rework.key(),
        workflow.max_rework,
        Breaker::ConsecutiveRework.key(),
        workflow.max_consecutive_rework,
    );
    for stage in stages {
        out.push_str(&rows(team, workflow, stage, &backs, width, default));
    }
    let blank = "  ".repeat(backs.len());
    if let Some(land) = &workflow.land {
        out.push_str(&format!("{blank}  {LAND:<width$}  {}\n", work_said(team, land)));
    }
    if ends_in_nothing {
        out.push_str(&format!("{blank}  {DONE}\n"));
    }
    out
}

/// The two rows of one stage of `workflow`, one of `team`: the stage with
/// what is said of it, and its `pass` under it.
#[spec(fn(&Team[@roles], &Workflow[@w], &Stage{v: v.n == w.n && v.roles == roles}, &[Back], usize, Harness)
    -> String requires w.roles == roles)]
fn rows(
    team: &Team,
    workflow: &Workflow,
    stage: &Stage,
    backs: &[Back],
    width: usize,
    default: Harness,
) -> String {
    let mut said = vec![match &stage.by {
        Performer::Human => HUMAN.to_string(),
        Performer::Role(work) => work_said(team, work),
    }];
    if let Some(harness) = stage.harness(default) {
        said.push(format!("harness {}", harness.key()));
    }
    if let Some(model) = &stage.model {
        said.push(format!("model {model}"));
    }
    if !stage.writes {
        said.push("writes nothing".to_string());
    }
    // The stock consult is every stage's that says nothing else.
    match (&stage.consult, &stage.by) {
        (Some(Consult::Role(consult)), _) => {
            said.push(format!("escalate → {}", work_said(team, consult)));
        }
        (None, Performer::Role(_)) => said.push(format!("escalate → {HUMAN}")),
        _ => {}
    }
    if let Some(to) = stage.reject() {
        said.push(format!("reject → {}", workflow.stage(to).name));
    }
    let said = said.join(" · ");
    let mut out = format!("{}{:<width$}  {said}\n", margin(backs, stage), stage.name);
    let pass = match stage.pass() {
        Pass::Stage(to) => workflow.stage(to).name.as_str(),
        Pass::Land => LAND,
        Pass::Done => DONE,
    };
    out.push_str(&format!("{}  ↓ pass → {pass}\n", between(backs, stage)));
    out
}

#[spec(fn(&Team[@roles], &Work[roles]) -> String)]
fn work_said(team: &Team, work: &Work) -> String {
    format!("{}: {}", team.role_of(work).name, work.skill)
}

/// The left margin of a stage's row: every reject that starts, ends or
/// passes by it, and the arrow into a stage one leads back to.
fn margin(backs: &[Back], stage: &Stage) -> String {
    let at = stage.at();
    let mut out = String::new();
    // Whether a reject has turned towards the stage: the line runs on to it.
    let mut across = false;
    let mut arrives = false;
    for back in backs {
        let cell = if back.to == at {
            arrives = true;
            if across { "┬─" } else { "┌─" }
        } else if back.from == at {
            if across { "┴─" } else { "└─" }
        } else if back.to < at && at < back.from {
            if across { "┼─" } else { "│ " }
        } else if across {
            "──"
        } else {
            "  "
        };
        across = across || back.to == at || back.from == at;
        out.push_str(cell);
    }
    out.push_str(match (arrives, across) {
        (true, _) => "▶ ",
        (false, true) => "─ ",
        (false, false) => "  ",
    });
    out
}

/// The left margin of the row under a stage: every reject that passes by.
fn between(backs: &[Back], stage: &Stage) -> String {
    let at = stage.at();
    let mut out = String::new();
    for back in backs {
        out.push_str(if back.to <= at && at < back.from { "│ " } else { "  " });
    }
    out.push_str("  ");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::team::tests::{FEATURE, one};

    #[test]
    fn every_stage_its_role_and_skill_and_every_edge_is_drawn() {
        let team = one(FEATURE);
        let drawn = draw(&team, team.workflow("feature").unwrap(), Harness::DEFAULT);
        let expected = "\
feature — flow:feature — max_rework 3, max_consecutive_rework 2

┌───▶ plan      dev: plan · harness claude-code · escalate → human
│       ↓ pass → build
│ ┌─▶ build     dev: implement · harness claude-code · model opus · escalate → reviewer: rule
│ │     ↓ pass → review
│ └── review    reviewer: review · harness claude-code · writes nothing · escalate → human · reject → build
│       ↓ pass → sign-off
└──── sign-off  human · reject → plan
        ↓ pass → land
      land      dev: land
";
        assert_eq!(drawn, expected, "\n{drawn}");
    }

    #[test]
    fn rejects_that_cross_or_share_a_stage_are_each_drawn_to_their_own() {
        let team = one("\
max_rework: 5
stages:
  - { name: a, by: dev, do: plan, on: { pass: b } }
  - { name: b, by: dev, do: plan, on: { pass: c, reject: a } }
  - { name: c, by: dev, do: plan, on: { pass: d, reject: a } }
  - { name: d, by: dev, do: plan, on: { pass: done, reject: c } }
");
        let drawn = draw(&team, team.workflow("feature").unwrap(), Harness::DEFAULT);
        let expected = "\
feature — flow:feature — max_rework 5, max_consecutive_rework 2

  ┌─┬─▶ a     dev: plan · harness claude-code
  │ │     ↓ pass → b
  │ └── b     dev: plan · harness claude-code · reject → a
  │       ↓ pass → c
┌─┴───▶ c     dev: plan · harness claude-code · reject → a
│         ↓ pass → d
└────── d     dev: plan · harness claude-code · reject → c
          ↓ pass → done
        done
";
        assert_eq!(drawn, expected, "\n{drawn}");
    }
}
