//! An issue's casting (docs/adr/0020): the harness and model its `harness:`
//! and `model:` labels name — and what a worker of an unbound repo runs by
//! it, within the host kit, which is an unbound repo's whole range.

use serde::Serialize;

use crate::harness::Harness;

/// What the label that names an issue's harness starts with.
pub const HARNESS_LABEL: &str = "harness:";
/// What the label that names its model starts with: the model is its
/// harness's own name for it.
pub const MODEL_LABEL: &str = "model:";

/// One harness of a range, with the models it holds, in order of preference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub harness: Harness,
    pub models: Vec<String>,
}

/// What a worker runs, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cast {
    pub harness: Harness,
    /// The model it is started on; `None`: its command as written.
    pub model: Option<String>,
    pub why: Why,
}

impl Cast {
    /// The worker launch command `launch` of its harness, started on its
    /// model: as written where it names none (`Harness::with_model`).
    pub fn command(&self, launch: &str) -> String {
        match &self.model {
            Some(model) => self.harness.with_model(launch, model),
            None => launch.to_string(),
        }
    }
}

/// Why a worker runs what it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// The issue's casting, which the range holds.
    Casting,
    /// The issue has no casting: the range's first.
    First,
    /// A switch on this host chose the harness, whatever the casting.
    Switch,
}

impl Why {
    /// What a human is told of it.
    pub fn said(self) -> &'static str {
        match self {
            Why::Casting => "its casting labels",
            Why::First => "no casting: the host kit's first",
            Why::Switch => "this host's switch, over any casting",
        }
    }
}

/// The casting `labels` name: a harness, and a model of it. `None` of an
/// issue that has none; an error says which label names what cannot be one.
fn casting(labels: &[String]) -> Result<Option<(Harness, Option<String>)>, String> {
    let named = |prefix: &str| -> Vec<&str> {
        labels.iter().filter_map(|label| label.strip_prefix(prefix)).collect()
    };
    let (harnesses, models) = (named(HARNESS_LABEL), named(MODEL_LABEL));
    if let [one, two, ..] = harnesses.as_slice() {
        return Err(format!(
            "it carries the labels `{HARNESS_LABEL}{one}` and `{HARNESS_LABEL}{two}`: one names \
             its harness"
        ));
    }
    if let [one, two, ..] = models.as_slice() {
        return Err(format!(
            "it carries the labels `{MODEL_LABEL}{one}` and `{MODEL_LABEL}{two}`: one names its \
             model"
        ));
    }
    let model = models.first().map(|model| model.to_string());
    let Some(key) = harnesses.first() else {
        return match model {
            Some(model) => Err(format!(
                "its label `{MODEL_LABEL}{model}` names a model of no harness: add a \
                 `{HARNESS_LABEL}` label"
            )),
            None => Ok(None),
        };
    };
    let harness = Harness::read(key)
        .ok_or_else(|| format!("its label `{HARNESS_LABEL}{key}`: {}", Harness::unknown(key)))?;
    Ok(Some((harness, model)))
}

/// What the host kit holds, as a human is told it.
fn holds(range: &[Held]) -> String {
    if range.is_empty() {
        return "nothing".to_string();
    }
    let each: Vec<String> = range
        .iter()
        .map(|held| match held.models.as_slice() {
            [] => format!("{} (no model)", held.harness.key()),
            models => format!("{} ({})", held.harness.key(), models.join(", ")),
        })
        .collect();
    each.join("; ")
}

/// What the worker of an issue labelled `labels` runs within `range`, the
/// host kit's harnesses in order: a harness `chosen` by a switch, whatever the
/// casting; else the casting, where the range holds it; else, with no casting,
/// the range's first. An error is an issue that is not dispatched, with why:
/// a casting the range does not hold, or labels that are none.
pub fn cast(labels: &[String], range: &[Held], chosen: Option<Harness>) -> Result<Cast, String> {
    let named = casting(labels);
    if let Some(chosen) = chosen {
        // The model of another harness is not this one's.
        let model = match &named {
            Ok(Some((harness, model))) if *harness == chosen => model.clone(),
            _ => range
                .first()
                .filter(|held| held.harness == chosen)
                .and_then(|held| held.models.first().cloned()),
        };
        return Ok(Cast { harness: chosen, model, why: Why::Switch });
    }
    let Some((harness, model)) = named? else {
        let first = range.first().ok_or("the host kit holds no harness")?;
        let model = first.models.first().cloned();
        return Ok(Cast { harness: first.harness, model, why: Why::First });
    };
    let Some(held) = range.iter().find(|held| held.harness == harness) else {
        return Err(format!(
            "its label `{HARNESS_LABEL}{}` names a harness the host kit does not hold; it holds: {}",
            harness.key(),
            holds(range)
        ));
    };
    if let Some(model) = &model
        && !held.models.contains(model)
    {
        return Err(format!(
            "its label `{MODEL_LABEL}{model}` names a model the host kit holds for no `{}`; it \
             holds: {}",
            harness.key(),
            holds(range)
        ));
    }
    Ok(Cast { harness, model, why: Why::Casting })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn range() -> Vec<Held> {
        vec![
            Held { harness: Harness::Pi, models: vec!["a/b".into(), "c/d".into()] },
            Held { harness: Harness::Codex, models: vec!["gpt-x".into()] },
            Held { harness: Harness::ClaudeCode, models: vec![] },
        ]
    }

    fn ran(harness: Harness, model: Option<&str>, why: Why) -> Result<Cast, String> {
        Ok(Cast { harness, model: model.map(str::to_string), why })
    }

    #[test]
    fn a_casting_the_host_kit_holds_is_run() {
        let range = range();
        let cast = |names: &[&str]| cast(&labels(names), &range, None);
        assert_eq!(
            cast(&["bug", "harness:codex", "model:gpt-x"]),
            ran(Harness::Codex, Some("gpt-x"), Why::Casting)
        );
        assert_eq!(cast(&["harness:pi", "model:c/d"]), ran(Harness::Pi, Some("c/d"), Why::Casting));
        // A harness named alone is started on its command as written.
        assert_eq!(cast(&["harness:claude-code"]), ran(Harness::ClaudeCode, None, Why::Casting));
    }

    #[test]
    fn an_issue_with_no_casting_runs_the_host_kit_s_first() {
        assert_eq!(
            cast(&labels(&["bug"]), &range(), None),
            ran(Harness::Pi, Some("a/b"), Why::First)
        );
        let bare = [Held { harness: Harness::Dsh, models: vec![] }];
        assert_eq!(cast(&[], &bare, None), ran(Harness::Dsh, None, Why::First));
        assert_eq!(cast(&[], &[], None), Err("the host kit holds no harness".to_string()));
    }

    #[test]
    fn a_casting_the_host_kit_does_not_hold_says_which_label_and_what_it_holds() {
        let range = range();
        let refused = |names: &[&str]| cast(&labels(names), &range, None).unwrap_err();
        let holds = "pi (a/b, c/d); codex (gpt-x); claude-code (no model)";
        assert_eq!(
            refused(&["harness:dsh"]),
            format!(
                "its label `harness:dsh` names a harness the host kit does not hold; it holds: {holds}"
            )
        );
        assert_eq!(
            refused(&["harness:codex", "model:a/b"]),
            format!(
                "its label `model:a/b` names a model the host kit holds for no `codex`; it holds: \
                 {holds}"
            )
        );
        assert!(refused(&["harness:claude-code", "model:opus"]).contains("`model:opus`"));
        assert!(refused(&["harness:nope"]).contains("`nope` is no harness"));
        assert!(refused(&["model:gpt-x"]).contains("names a model of no harness"));
        assert!(refused(&["harness:pi", "harness:codex"]).contains("one names its harness"));
        assert!(refused(&["harness:pi", "model:a/b", "model:c/d"]).contains("one names its model"));
    }

    #[test]
    fn a_switch_wins_over_the_casting() {
        let range = range();
        let cast = |names: &[&str], chosen| cast(&labels(names), &range, Some(chosen));
        // Of the casting's harness, its model; of another, none of its.
        assert_eq!(
            cast(&["harness:codex", "model:gpt-x"], Harness::Codex),
            ran(Harness::Codex, Some("gpt-x"), Why::Switch)
        );
        assert_eq!(
            cast(&["harness:codex", "model:gpt-x"], Harness::ClaudeCode),
            ran(Harness::ClaudeCode, None, Why::Switch)
        );
        // A casting the host kit does not hold is no refusal under a switch.
        assert_eq!(cast(&["harness:dsh"], Harness::Pi), ran(Harness::Pi, Some("a/b"), Why::Switch));
        assert_eq!(cast(&[], Harness::Codex), ran(Harness::Codex, None, Why::Switch));
    }
}
