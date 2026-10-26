//! A workflow drawn as a graph, in plain text: one row per stage in the order
//! its file lists them, each `pass` named under its stage, and each `reject`
//! a line up the left margin to the stage it leads back to. It reads the same
//! in a terminal and in a code block of a GitHub comment.

use flux_rs::attrs::*;

use crate::position::Breaker;
use crate::team::{Consult, DONE, HUMAN, LAND, Pass, Performer, Stage, Team, Work, Workflow};

/// One `reject`: from the stage at `from` back to the one at `to`.
struct Back {
    from: usize,
    to: usize,
}

/// What `team`'s roster says each of its roles may run, in plain text, as
/// `show` ends: a row per role in the team's order, each harness and model
/// it allows in order of preference with what it is good at, and how the
/// team says to cast among them under it. A role the roster names none for
/// may run whatever the host kit holds.
pub fn roster(team: &Team) -> String {
    let mut width = 0;
    for role in &team.roles {
        width = width.max(role.name.chars().count());
    }
    let mut out = "roster\n\n".to_string();
    for role in &team.roles {
        let Some(entry) = &role.roster else {
            out.push_str(&format!("  {:<width$}  the host kit whole\n", role.name));
            continue;
        };
        let each: Vec<String> = entry
            .run
            .iter()
            .map(|allowed| {
                let model = allowed.model.as_deref().unwrap_or("any model");
                let good_at =
                    allowed.good_at.as_ref().map(|good| format!(" ({good})")).unwrap_or_default();
                format!("{} {model}{good_at}", allowed.harness.key())
            })
            .collect();
        out.push_str(&format!("  {:<width$}  {}\n", role.name, each.join(" · ")));
        if let Some(cast) = &entry.cast {
            out.push_str(&format!("  {:<width$}  cast: {cast}\n", ""));
        }
    }
    out
}

/// The drawing of `workflow`, one of `team`: lines, each ended by a
/// newline. What a stage's worker runs is its role's range's to say.
#[spec(fn(&Team[@roles], &Workflow{v: v.roles == roles}) -> String)]
pub fn draw(team: &Team, workflow: &Workflow) -> String {
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
        out.push_str(&rows(team, workflow, stage, &backs, width));
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
#[spec(fn(&Team[@roles], &Workflow[@w], &Stage{v: v.n == w.n && v.roles == roles}, &[Back], usize)
    -> String requires w.roles == roles)]
fn rows(team: &Team, workflow: &Workflow, stage: &Stage, backs: &[Back], width: usize) -> String {
    let mut said = vec![match &stage.by {
        Performer::Human => HUMAN.to_string(),
        Performer::Role(work) => work_said(team, work),
    }];
    if !stage.writes {
        said.push("writes nothing".to_string());
    }
    // Only a consult named by the team runs before escalation.
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
    use crate::team::tests::{FEATURE, one, rostered};

    #[test]
    fn every_stage_its_role_and_skill_and_every_edge_is_drawn() {
        let team = one(FEATURE);
        let drawn = draw(&team, team.workflow("feature").unwrap());
        let expected = "\
feature — flow:feature — max_rework 3, max_consecutive_rework 2

┌───▶ plan      dev: plan · escalate → human
│       ↓ pass → build
│ ┌─▶ build     dev: implement · escalate → reviewer: rule
│ │     ↓ pass → review
│ └── review    reviewer: review · writes nothing · escalate → human · reject → build
│       ↓ pass → sign-off
└──── sign-off  human · reject → plan
        ↓ pass → land
      land      dev: land
";
        assert_eq!(drawn, expected, "\n{drawn}");
    }

    #[test]
    fn the_roster_says_what_each_role_may_run_and_how_it_is_cast() {
        let team = rostered(
            "  dev:\n    cast: Hard work on opus.\n    run:\n      - { harness: claude-code, \
             model: opus, good_at: hard work }\n      - { harness: pi }\n",
        )
        .unwrap();
        let drawn = roster(&team);
        let expected = "\
roster

  dev       claude-code opus (hard work) · pi any model
            cast: Hard work on opus.
  reviewer  the host kit whole
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
        let drawn = draw(&team, team.workflow("feature").unwrap());
        let expected = "\
feature — flow:feature — max_rework 5, max_consecutive_rework 2

  ┌─┬─▶ a     dev: plan · escalate → human
  │ │     ↓ pass → b
  │ └── b     dev: plan · escalate → human · reject → a
  │       ↓ pass → c
┌─┴───▶ c     dev: plan · escalate → human · reject → a
│         ↓ pass → d
└────── d     dev: plan · escalate → human · reject → c
          ↓ pass → done
        done
";
        assert_eq!(drawn, expected, "\n{drawn}");
    }
}
