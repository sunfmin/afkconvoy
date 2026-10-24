//! A team's roster drafted from a host kit (docs/adr/0020): each role of the
//! team given the harnesses and models the host kit holds, in its order, with
//! what the host kit's first model says each is good at and how to cast among
//! them. A human reads the draft, edits it and commits it; the fleet never
//! changes a team's roster.

use serde::Deserialize;

use crate::casting::Held;
use crate::team::{ROSTER_KEY, Role, Team, unlimited};

/// What the drafting model says one harness and model is good at.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Strength {
    pub harness: String,
    /// `None` of a harness the host kit holds no model for.
    #[serde(default)]
    pub model: Option<String>,
    pub good_at: String,
}

/// What the drafting model says of how to cast an issue among what one role
/// may run.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Guidance {
    pub role: String,
    pub cast: String,
}

/// What a role is drafted to run: the host kit's `range`, in its order, less
/// each harness the role holds no limits for while it holds limits for
/// another (`team::unlimited`) — which `check` would refuse.
pub fn allowed(role: &Role, range: &[Held]) -> Vec<Held> {
    let mut allowed = vec![];
    for held in range {
        if unlimited(role, held.harness).is_none() {
            allowed.push(held.clone());
        }
    }
    allowed
}

/// Each harness and model `range` holds, as a drafter and a reader are told
/// it: `pi a/fast`, or a harness alone where it holds no model.
fn each(range: &[Held]) -> Vec<String> {
    let mut each = vec![];
    for held in range {
        if held.models.is_empty() {
            each.push(held.harness.key().to_string());
        }
        for model in held.ids() {
            each.push(format!("{} {model}", held.harness.key()));
        }
    }
    each
}

/// What the prompt's first line opens with: how a drafter's prompt is told
/// from another.
pub const DRAFT_PROMPT_OPENS: &str = "You are drafting the roster of a team of coding agents";

/// The prompt the host kit's first model is sent: every harness and model of
/// `range` in order, each role of `team` with the work the team asks of it
/// and what it is drafted to run, and the JSON lines to answer with.
pub fn draft_prompt(team: &Team, range: &[Held]) -> String {
    let mut prompt = format!(
        "{DRAFT_PROMPT_OPENS}: for each role, which harnesses and models its workers may run, \
         in order of preference, what each model is good at, and how an issue is to be cast \
         among them. A human reads the draft and edits it.\n\nHarnesses and models this host \
         holds, in order of preference:\n"
    );
    for said in each(range) {
        prompt.push_str(&format!("- {said}\n"));
    }
    prompt.push_str("\nRoles, each with the work the team asks of it and what it may run:\n");
    let duties = team.duties();
    for role in &team.roles {
        let mut asked: Vec<String> = vec![];
        for (name, by, skill) in &duties {
            if name == &role.name {
                asked.push(format!("{by}, performing the skill `{skill}`"));
            }
        }
        let asked = if asked.is_empty() { "no work yet".to_string() } else { asked.join("; ") };
        let may = each(&allowed(role, range));
        let may = if may.is_empty() { "nothing".to_string() } else { may.join(", ") };
        prompt.push_str(&format!("- {}: {asked}. It may run: {may}.\n", role.name));
    }
    prompt.push_str(
        "\nAnswer with one JSON object a line and nothing else:\n- for each harness and model \
         above, what it is good at, in one sentence: {\"harness\": \"pi\", \"model\": \
         \"a/fast\", \"good_at\": \"...\"}, with \"model\": null for a harness listed with no \
         model;\n- for each role, how to cast an issue among what it may run, in one or two \
         sentences: {\"role\": \"dev\", \"cast\": \"...\"}.\n",
    );
    prompt
}

/// `text` as a yaml scalar in double quotes, whatever it holds, on one line.
fn quoted(text: &str) -> String {
    let mut quoted = String::from("\"");
    for c in text.trim().chars() {
        match c {
            '"' => quoted.push_str("\\\""),
            '\\' => quoted.push_str("\\\\"),
            '\n' | '\r' | '\t' => quoted.push(' '),
            c if c.is_control() => {}
            c => quoted.push(c),
        }
    }
    quoted.push('"');
    quoted
}

/// What `strengths` say the harness `key` on `model` is good at: the first
/// said of it.
fn good_at<'s>(strengths: &'s [Strength], key: &str, model: Option<&str>) -> Option<&'s str> {
    strengths
        .iter()
        .find(|said| said.harness == key && said.model.as_deref() == model)
        .map(|said| said.good_at.as_str())
}

/// One harness and model of a role's draft entry, as the team file holds it.
fn allowance(key: &str, model: Option<&str>, good: Option<&str>) -> String {
    let mut line = format!("      - {{ harness: {key}");
    if let Some(model) = model {
        line.push_str(&format!(", model: {}", quoted(model)));
    }
    if let Some(good) = good {
        line.push_str(&format!(", good_at: {}", quoted(good)));
    }
    line.push_str(" }\n");
    line
}

/// The draft of `team`'s roster on a host kit of `range`, as the team file's
/// `roster` key and everything under it: each role an entry of what it is
/// drafted to run (`allowed`), with what `strengths` say each is good at and
/// the `guidance` given for it, and a comment naming each harness of the host
/// kit it is drafted none of. A role drafted nothing is named in a comment
/// alone.
pub fn draft(team: &Team, range: &[Held], strengths: &[Strength], guidance: &[Guidance]) -> String {
    let mut draft = String::new();
    for role in &team.roles {
        let allowed = allowed(role, range);
        let mut left: Vec<&str> = vec![];
        for held in range {
            if !allowed.iter().any(|kept| kept.harness == held.harness) {
                left.push(held.harness.key());
            }
        }
        if allowed.is_empty() {
            draft.push_str(&format!(
                "  # {}: drafted nothing; it holds limits for none of the host kit's harnesses \
                 ({})\n",
                role.name,
                left.join(", ")
            ));
            continue;
        }
        draft.push_str(&format!("  {}:\n", role.name));
        let cast = guidance.iter().find(|said| said.role == role.name);
        if let Some(cast) = cast {
            draft.push_str(&format!("    cast: {}\n", quoted(&cast.cast)));
        }
        draft.push_str("    run:\n");
        for held in &allowed {
            let key = held.harness.key();
            if held.models.is_empty() {
                draft.push_str(&allowance(key, None, good_at(strengths, key, None)));
            }
            for model in held.ids() {
                let good = good_at(strengths, key, Some(model));
                draft.push_str(&allowance(key, Some(model), good));
            }
        }
        if !left.is_empty() {
            draft.push_str(&format!(
                "    # left out: {}, which the role holds no limits for\n",
                left.join(", ")
            ));
        }
    }
    // A roster of no entry is an empty map, which yaml writes on its line.
    let none = draft.lines().all(|line| line.trim_start().starts_with('#'));
    format!("{ROSTER_KEY}:{}\n{draft}", if none { " {}" } else { "" })
}

/// Each harness and model of `range` the `strengths` say nothing of, and each
/// role of `team` drafted something the `guidance` says nothing for: what the
/// draft holds no words of.
pub fn unsaid(
    team: &Team,
    range: &[Held],
    strengths: &[Strength],
    guidance: &[Guidance],
) -> Vec<String> {
    let mut unsaid = vec![];
    for held in range {
        let key = held.harness.key();
        if held.models.is_empty() && good_at(strengths, key, None).is_none() {
            unsaid.push(format!("what {key} is good at"));
        }
        for model in held.ids() {
            if good_at(strengths, key, Some(model)).is_none() {
                unsaid.push(format!("what {key} {model} is good at"));
            }
        }
    }
    for role in &team.roles {
        let drafted = !allowed(role, range).is_empty();
        if drafted && !guidance.iter().any(|said| said.role == role.name) {
            unsaid.push(format!("how to cast for {}", role.name));
        }
    }
    unsaid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::Harness;
    use crate::team::tests::rostered;

    /// The usual team, of `dev` and `reviewer`.
    fn team() -> Team {
        rostered("  reviewer: { run: [{ harness: pi }] }\n").unwrap()
    }

    fn range() -> Vec<Held> {
        vec![
            Held { harness: Harness::ClaudeCode, models: vec!["opus".into(), "haiku".into()] },
            Held { harness: Harness::Pi, models: vec!["a/fast".into()] },
            Held { harness: Harness::Dsh, models: vec![] },
        ]
    }

    #[test]
    fn a_draft_gives_each_role_the_host_kit_in_order_with_what_was_said_of_each() {
        let team = team();
        let strengths = vec![
            Strength {
                harness: "claude-code".into(),
                model: Some("opus".into()),
                good_at: "hard \"wide\"\nwork".into(),
            },
            Strength {
                harness: "pi".into(),
                model: Some("a/fast".into()),
                good_at: "small fixes".into(),
            },
            Strength {
                harness: "pi".into(),
                model: Some("a/fast".into()),
                good_at: "said again".into(),
            },
            Strength { harness: "dsh".into(), model: None, good_at: "cheap work".into() },
        ];
        let guidance = vec![Guidance { role: "dev".into(), cast: "Small fixes on pi.".into() }];
        let draft = draft(&team, &range(), &strengths, &guidance);
        let dev = concat!(
            "  dev:\n",
            "    cast: \"Small fixes on pi.\"\n",
            "    run:\n",
            "      - { harness: claude-code, model: \"opus\", good_at: \"hard \\\"wide\\\" work\" }\n",
            "      - { harness: claude-code, model: \"haiku\" }\n",
            "      - { harness: pi, model: \"a/fast\", good_at: \"small fixes\" }\n",
            "      - { harness: dsh, good_at: \"cheap work\" }\n",
        );
        assert!(draft.starts_with("roster:\n"), "{draft}");
        assert!(draft.contains(dev), "{draft}");
        for role in &team.roles {
            assert!(draft.contains(&format!("  {}:\n", role.name)), "{draft}");
        }
        // It is the team file's yaml as written, and the team loads with it.
        let roster = draft.strip_prefix("roster:\n").unwrap();
        let loaded = rostered(roster).unwrap();
        let entry = loaded.role("dev").unwrap().roster.clone().unwrap();
        assert_eq!(entry.cast.as_deref(), Some("Small fixes on pi."));
        assert_eq!(entry.run.len(), 4);
        assert_eq!(entry.run[0].good_at.as_deref(), Some("hard \"wide\" work"));
        assert_eq!(
            unsaid(&team, &range(), &strengths, &guidance)[0],
            "what claude-code haiku is good at"
        );
    }

    #[test]
    fn a_harness_a_role_holds_no_limits_for_is_left_out_and_said() {
        let mut team = team();
        team.roles[0].limits = vec![Harness::Pi];
        let name = team.roles[0].name.clone();
        let draft = draft(&team, &range(), &[], &[]);
        let entry = format!(
            "  {name}:\n    run:\n      - {{ harness: pi, model: \"a/fast\" }}\n    # left out: \
             claude-code, dsh, which the role holds no limits for\n"
        );
        assert!(draft.contains(&entry), "{draft}");
        // A role drafted nothing is named in a comment; a roster of no entry
        // is an empty map, and the team loads with it.
        for role in &mut team.roles {
            role.limits = vec![Harness::Codex];
        }
        let draft = super::draft(&team, &range(), &[], &[]);
        assert!(
            draft.starts_with(&format!("roster: {{}}\n  # {name}: drafted nothing")),
            "{draft}"
        );
        assert!(rostered(&format!("{}\n", draft.strip_prefix("roster:").unwrap())).is_ok());
        let prompt = draft_prompt(&team, &range());
        assert!(prompt.starts_with(DRAFT_PROMPT_OPENS), "{prompt}");
        assert!(prompt.contains("- pi a/fast\n- dsh\n"), "{prompt}");
        assert!(prompt.contains(&format!("- {name}: ")), "{prompt}");
        assert!(prompt.contains("It may run: nothing."), "{prompt}");
    }
}
