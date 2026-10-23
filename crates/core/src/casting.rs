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
    /// The issue was a caster's to cast, and is not: the range's first.
    Uncast,
}

impl Why {
    /// What a human is told of it.
    pub fn said(self) -> &'static str {
        match self {
            Why::Casting => "its casting labels",
            Why::First => "no casting: the host kit's first",
            Why::Switch => "this host's switch, over any casting",
            Why::Uncast => "the caster did not cast it in ten minutes: the host kit's first",
        }
    }
}

/// How long the issues a caster is to cast are held back for it, from its
/// start: after that they are dispatched uncast.
pub const CASTER_SECONDS: u64 = 600;

/// Whether `range` leaves a choice to make: it holds more than one harness
/// and model, a harness held with no model being one.
pub fn choice(range: &[Held]) -> bool {
    let options: usize = range.iter().map(|held| held.models.len().max(1)).sum();
    options > 1
}

/// Whether `labels` name no casting: no `harness:` label and no `model:`
/// one. An issue with either is a human's to change, never a caster's.
pub fn uncast(labels: &[String]) -> bool {
    !labels.iter().any(|label| label.starts_with(HARNESS_LABEL) || label.starts_with(MODEL_LABEL))
}

/// The last caster started for a repo on this host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Caster {
    /// When it was started, in epoch seconds.
    pub since: u64,
    /// The issues it was given to cast.
    pub given: Vec<u64>,
    /// Its terminal is still live.
    pub live: bool,
}

impl Caster {
    /// Whether it still holds back the issues it is to cast: live, and
    /// started less than `CASTER_SECONDS` ago. No second one starts while it
    /// does.
    pub fn running(&self, now: u64) -> bool {
        self.live && now.saturating_sub(self.since) < CASTER_SECONDS
    }
}

/// An unbound repo's caster as a tick sees it, while one runs or is due.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Standing {
    /// When the caster that runs was started; `None`: none runs.
    pub running_since: Option<u64>,
    /// A caster is to be started: the pass starts one.
    pub due: bool,
    /// The frontier's issues held back for it, lowest number first.
    pub held: Vec<u64>,
    /// Every issue of the repo a caster started now is given, lowest number
    /// first: open, with the ready label, no epic label and no casting.
    pub to_cast: Vec<u64>,
}

/// Why a frontier issue is not dispatched while it waits for the caster.
pub const HELD_FOR_CASTER: &str =
    "held for the caster: dispatched once it is cast, or uncast after ten minutes";

/// The name a caster's worktree is created under — one per repo on a
/// machine — and so, with a `-<k>` when the name was taken, the name of its
/// directory.
pub const CASTER_WORKTREE: &str = "convoy-caster";

/// Whether an unbound repo's issue labelled `labels` is a caster's to cast,
/// given a choice to make: it carries the `ready` label, none of `epics`,
/// and no casting.
pub fn to_cast(labels: &[String], ready: &str, epics: &[String]) -> bool {
    labels.iter().any(|label| label == ready)
        && !labels.iter().any(|label| epics.contains(label))
        && uncast(labels)
}

/// What a tick does with an issue on the frontier, for the caster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForCaster {
    /// Nothing: it is dispatched as its casting says.
    Dispatch,
    /// It is held back while the caster runs.
    Wait,
    /// It is held back, and a caster is to be started.
    Due,
    /// The caster had it and did not cast it: it is dispatched uncast.
    Late,
}

/// What an issue labelled `labels` on the frontier waits for, within `range`,
/// under the harness a switch `chosen`, with `caster` the last one started,
/// `now`. An issue that has a casting, a range with no choice to make and a
/// switch, which chooses whatever the casting, leave nothing to cast.
pub fn for_caster(
    number: u64,
    labels: &[String],
    range: &[Held],
    chosen: Option<Harness>,
    caster: Option<&Caster>,
    now: u64,
) -> ForCaster {
    if chosen.is_some() || !uncast(labels) || !choice(range) {
        return ForCaster::Dispatch;
    }
    match caster {
        Some(caster) if caster.running(now) => ForCaster::Wait,
        Some(caster) if caster.given.contains(&number) => ForCaster::Late,
        _ => ForCaster::Due,
    }
}

/// How often a paused harness is probed, in seconds: one that answers is no
/// longer paused.
pub const PAUSE_PROBE_SECONDS: u64 = 3600;

/// A harness paused on the host (docs/adr/0020): left out of the host kit,
/// and so of every range, until a probe finds it working. It is the host's
/// running state; the host kit's file is not changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Paused {
    #[serde(deserialize_with = "harness_key")]
    pub harness: Harness,
    /// When it was paused, in epoch seconds.
    pub since: u64,
    /// What the fleet found of it.
    pub why: String,
    /// Its last probe, which it did not answer; `None`: none yet.
    #[serde(default)]
    pub probed: Option<u64>,
}

impl Paused {
    /// Whether it is to be probed `now`: an hour after it was paused, or
    /// after its last probe.
    pub fn due(&self, now: u64) -> bool {
        now.saturating_sub(self.probed.unwrap_or(self.since)) >= PAUSE_PROBE_SECONDS
    }
}

fn harness_key<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Harness, D::Error> {
    let key = <String as serde::Deserialize>::deserialize(deserializer)?;
    Harness::read(&key).ok_or_else(|| serde::de::Error::custom(Harness::unknown(&key)))
}

/// The pause of the harness `labels` cast an issue to, where it is paused.
pub fn paused_of<'a>(labels: &[String], paused: &'a [Paused]) -> Option<&'a Paused> {
    let key = labels.iter().find_map(|label| label.strip_prefix(HARNESS_LABEL))?;
    paused.iter().find(|pause| pause.harness.key() == key)
}

/// `labels` without the casting: what an issue cast again carries.
pub fn without_casting(labels: &[String]) -> Vec<String> {
    labels.iter().filter(|label| uncast(std::slice::from_ref(*label))).cloned().collect()
}

/// What a caster says on an issue it cast to `labels`, with why.
pub fn caster_comment(labels: &[String], why: &str) -> String {
    format!("Cast {}: {}", said_labels(labels), why.trim())
}

fn said_labels(labels: &[String]) -> String {
    let named: Vec<String> = labels.iter().map(|label| format!("`{label}`")).collect();
    named.join(" and ")
}

/// Whether the casting `labels` carry is a caster's: a comment of `bodies`
/// is the one a caster leaves as it puts that casting on. A human's casting
/// is never cast again.
pub fn by_caster<'a>(labels: &[String], bodies: impl IntoIterator<Item = &'a str>) -> bool {
    let casting: Vec<String> =
        labels.iter().filter(|label| !uncast(std::slice::from_ref(*label))).cloned().collect();
    if casting.is_empty() {
        return false;
    }
    let head = format!("Cast {}: ", said_labels(&casting));
    bodies.into_iter().any(|body| body.starts_with(&head))
}

/// Whether an issue labelled `labels` is to be cast again: a caster cast it
/// to a harness that is now `paused`, and no worker has `started` on it.
pub fn to_recast<'a>(
    labels: &[String],
    bodies: impl IntoIterator<Item = &'a str>,
    started: bool,
    paused: &[Paused],
) -> bool {
    !started && paused_of(labels, paused).is_some() && by_caster(labels, bodies)
}

/// What a caster puts on one issue: its `harness:` label, and its `model:`
/// one where it names a model, with why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, serde::Deserialize)]
pub struct Casting {
    pub issue: u64,
    pub harness: String,
    #[serde(default)]
    pub model: Option<String>,
    pub why: String,
}

impl Casting {
    /// Its labels, where `range` holds what it names; an error says what it
    /// does not.
    pub fn labels(&self, range: &[Held]) -> Result<Vec<String>, String> {
        let mut labels = vec![format!("{HARNESS_LABEL}{}", self.harness)];
        labels.extend(self.model.iter().map(|model| format!("{MODEL_LABEL}{model}")));
        cast(&labels, range, None)?;
        if self.why.trim().is_empty() {
            return Err("it says nothing of why".to_string());
        }
        Ok(labels)
    }
}

/// One issue a caster is to cast, as its prompt gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToCast {
    pub number: u64,
    pub title: String,
    pub body: String,
}

/// One model of a range as a caster's prompt gives it, with what the host
/// kit says it is good at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Offered {
    pub harness: Harness,
    pub model: Option<String>,
    pub good_at: Option<String>,
}

/// How much of an issue's body a caster's prompt gives: the prompt is one
/// argument of a command line.
const BODY_CHARS: usize = 3000;

/// The prompt a caster's model is sent: every issue in `issues`, what
/// `offered` holds in order of preference, and the one JSON line a casting
/// it is to answer with.
pub fn caster_prompt(issues: &[ToCast], offered: &[Offered]) -> String {
    let mut prompt = String::from(
        "You are the caster of a fleet of coding agents. For each GitHub issue below, choose \
         the harness and model whose worker suits the work best: how hard the work is and what \
         kind it is. Choose only among those listed, which are in order of preference: prefer \
         the earlier where the work does not call for another.\n\nHarnesses and models:\n",
    );
    for each in offered {
        let model = each.model.as_deref().map_or(String::new(), |model| format!(" {model}"));
        let good = each.good_at.as_deref().map_or(String::new(), |good| format!(": {good}"));
        prompt.push_str(&format!("- {}{model}{good}\n", each.harness.key()));
    }
    prompt.push_str("\nIssues:\n");
    for issue in issues {
        let body: String = issue.body.chars().take(BODY_CHARS).collect();
        let cut = if body.len() < issue.body.len() { " […]" } else { "" };
        prompt.push_str(&format!("\n## #{} {}\n{}{cut}\n", issue.number, issue.title, body.trim()));
    }
    prompt.push_str(
        "\nAnswer with one line for each issue and nothing else, each one JSON object: \
         {\"issue\": <number>, \"harness\": \"<harness>\", \"model\": \"<model>\" or null where \
         the harness is listed with none, \"why\": \"<one or two sentences>\"}\n",
    );
    prompt
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

    #[test]
    fn a_range_of_more_than_one_harness_and_model_leaves_a_choice_to_make() {
        assert!(choice(&range()));
        let one = |models: &[&str]| {
            let models = models.iter().map(|model| model.to_string()).collect();
            vec![Held { harness: Harness::Pi, models }]
        };
        assert!(!choice(&one(&["a/b"])));
        assert!(!choice(&one(&[])), "a harness on no model is one");
        assert!(choice(&one(&["a/b", "c/d"])));
        assert!(!choice(&[]));
    }

    #[test]
    fn an_uncast_issue_waits_for_the_caster_for_ten_minutes_from_its_start() {
        let range = range();
        let given = Caster { since: 1000, given: vec![7], live: true };
        let held = |number, names: &[&str], caster: Option<&Caster>, now| {
            for_caster(number, &labels(names), &range, None, caster, now)
        };
        assert_eq!(held(7, &["bug"], None, 1000), ForCaster::Due);
        assert_eq!(held(7, &["bug"], Some(&given), 1599), ForCaster::Wait);
        // One it was not given waits too: no second starts while it runs.
        assert_eq!(held(8, &[], Some(&given), 1599), ForCaster::Wait);
        assert_eq!(held(7, &[], Some(&given), 1600), ForCaster::Late);
        assert_eq!(held(8, &[], Some(&given), 1600), ForCaster::Due);
        // A caster that ended had its chance at what it was given.
        let ended = Caster { live: false, ..given.clone() };
        assert_eq!(held(7, &[], Some(&ended), 1001), ForCaster::Late);
        assert_eq!(held(8, &[], Some(&ended), 1001), ForCaster::Due);
        // A casting a human put on, no choice and a switch hold nothing back.
        assert_eq!(held(7, &["harness:pi"], None, 1000), ForCaster::Dispatch);
        assert_eq!(held(7, &["model:a/b"], None, 1000), ForCaster::Dispatch);
        let alone = [Held { harness: Harness::Pi, models: vec!["a/b".into()] }];
        assert_eq!(for_caster(7, &[], &alone, None, None, 0), ForCaster::Dispatch);
        assert_eq!(for_caster(7, &[], &range, Some(Harness::Pi), None, 0), ForCaster::Dispatch);
    }

    #[test]
    fn a_caster_casts_open_ready_issues_with_no_casting_and_no_epic_label() {
        let epics = ["epic".to_string()];
        let to = |names: &[&str]| to_cast(&labels(names), "ready", &epics);
        assert!(to(&["ready", "bug"]));
        assert!(!to(&["bug"]));
        assert!(!to(&["ready", "epic"]));
        assert!(!to(&["ready", "harness:pi"]));
    }

    #[test]
    fn a_casting_is_put_on_only_within_the_range_and_with_why() {
        let casting = |harness: &str, model: Option<&str>, why: &str| Casting {
            issue: 7,
            harness: harness.to_string(),
            model: model.map(str::to_string),
            why: why.to_string(),
        };
        let range = range();
        assert_eq!(
            casting("pi", Some("c/d"), "small").labels(&range),
            Ok(labels(&["harness:pi", "model:c/d"]))
        );
        assert_eq!(casting("codex", None, "big").labels(&range), Ok(labels(&["harness:codex"])));
        assert!(casting("dsh", None, "x").labels(&range).unwrap_err().contains("does not hold"));
        assert!(casting("pi", Some("x/y"), "x").labels(&range).is_err());
        assert!(casting("pi", None, " ").labels(&range).unwrap_err().contains("why"));
    }

    #[test]
    fn a_caster_s_casting_to_a_paused_harness_is_cast_again_until_a_worker_started() {
        let paused = [Paused { harness: Harness::Pi, since: 10, why: "refused".into(), probed: None }];
        let cast = labels(&["ready", "harness:pi", "model:a/b"]);
        let said = caster_comment(&labels(&["harness:pi", "model:a/b"]), "small ");
        assert_eq!(said, "Cast `harness:pi` and `model:a/b`: small");
        assert!(to_recast(&cast, [said.as_str()], false, &paused));
        assert!(!to_recast(&cast, [said.as_str()], true, &paused), "a worker started on it");
        // A human's casting is never cast again.
        assert!(!to_recast(&cast, ["Cast it to pi, please"], false, &paused));
        // Nor one to a harness that is not paused.
        let codex = labels(&["harness:codex"]);
        assert!(!to_recast(&codex, ["Cast `harness:codex`: big"], false, &paused));
        assert_eq!(paused_of(&cast, &paused), Some(&paused[0]));
        assert_eq!(without_casting(&cast), labels(&["ready"]));
    }

    #[test]
    fn a_paused_harness_is_probed_an_hour_after_it_was_paused_or_last_probed() {
        let mut pause = Paused { harness: Harness::Pi, since: 100, why: "x".into(), probed: None };
        assert!(!pause.due(100 + PAUSE_PROBE_SECONDS - 1));
        assert!(pause.due(100 + PAUSE_PROBE_SECONDS));
        pause.probed = Some(5000);
        assert!(!pause.due(5000 + PAUSE_PROBE_SECONDS - 1));
        assert!(pause.due(5000 + PAUSE_PROBE_SECONDS));
        let text = serde_json::to_string(&pause).unwrap();
        assert_eq!(serde_json::from_str::<Paused>(&text).unwrap(), pause);
    }

    #[test]
    fn a_caster_s_prompt_gives_each_issue_and_each_harness_and_model_in_order() {
        let issues = [ToCast { number: 7, title: "Fix it".into(), body: "x".repeat(4000) }];
        let offered = [
            Offered {
                harness: Harness::Pi,
                model: Some("a/b".into()),
                good_at: Some("speed".into()),
            },
            Offered { harness: Harness::Codex, model: None, good_at: None },
        ];
        let prompt = caster_prompt(&issues, &offered);
        assert!(prompt.contains("- pi a/b: speed\n- codex\n"), "{prompt}");
        assert!(prompt.contains("## #7 Fix it\n"), "{prompt}");
        assert!(prompt.contains(" […]"), "a long body is cut");
        assert!(prompt.len() < 4000, "{}", prompt.len());
    }
}
