//! What a worker runs (docs/adr/0020): the harness kit order and the role's
//! range already selected, and the model its one `model:` label names of that
//! harness — or that harness's user default. A `harness:` label never selects
//! a harness.

use serde::Serialize;

use crate::harness::Harness;
use crate::team::{Entry, Role, Team};

/// What the label that names its model starts with: the model is any of its
/// names on the selected harness (`Held::model`). The label is left as written;
/// a match starts the worker on the model's id.
const MODEL_LABEL: &str = "model:";

/// One harness of a range, with the models it holds, in order of preference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub harness: Harness,
    pub models: Vec<Model>,
}

/// One model a harness holds: the id its harness lists, which a worker is
/// started on, and the display and sourced research names that match it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Model {
    pub id: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub names: Vec<String>,
}

/// A model known by its id alone.
impl From<&str> for Model {
    fn from(id: &str) -> Model {
        Model { id: id.to_string(), names: vec![] }
    }
}

impl Model {
    /// Its id, then its other names.
    fn every_name(&self) -> impl Iterator<Item = &str> {
        std::iter::once(self.id.as_str()).chain(self.names.iter().map(String::as_str))
    }
}

/// Why a name matched no one model of a harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unmatched {
    /// It matched these two, and so neither.
    Two(String, String),
    /// It matched none; these are the nearest names the harness holds.
    None(Vec<String>),
}

impl Unmatched {
    /// What a human is told of `name`, a model of `harness`.
    pub fn said(&self, name: &str, harness: Harness) -> String {
        match self {
            Unmatched::Two(one, two) => format!(
                "`{name}` names two models of {}, `{one}` and `{two}`, and so neither: name \
                 one by its id",
                harness.key()
            ),
            Unmatched::None(nearest) if nearest.is_empty() => {
                format!("`{name}` names no model the host kit holds for {}", harness.key())
            }
            Unmatched::None(nearest) => format!(
                "`{name}` names no model the host kit holds for {}; the nearest names: {}",
                harness.key(),
                nearest.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(", ")
            ),
        }
    }
}

/// How many nearest names a name that matched nothing is told of.
const NEAREST: usize = 3;

impl Held {
    /// The model `name` names here: the one whose id or other name it is,
    /// whatever the case — else, with the provider prefix left off, the one
    /// whose id or name is `<provider>/<name>`. A name that matches two
    /// matches neither.
    pub fn model(&self, name: &str) -> Result<&Model, Unmatched> {
        let name = name.trim();
        // Whole names take precedence over names without a provider prefix.
        for unprefixed in [false, true] {
            let mut first: Option<&Model> = None;
            for model in &self.models {
                let matches = model.every_name().any(|held| {
                    if unprefixed {
                        !name.contains('/')
                            && held
                                .split_once('/')
                                .is_some_and(|(_, rest)| rest.eq_ignore_ascii_case(name))
                    } else {
                        held.eq_ignore_ascii_case(name)
                    }
                });
                if matches {
                    if let Some(one) = first {
                        return Err(Unmatched::Two(one.id.clone(), model.id.clone()));
                    }
                    first = Some(model);
                }
            }
            if let Some(model) = first {
                return Ok(model);
            }
        }
        Err(Unmatched::None(self.nearest(name)))
    }

    /// The names it holds nearest `name`, nearest first: by how few letters
    /// are to be changed, whatever the case.
    fn nearest(&self, name: &str) -> Vec<String> {
        let name = name.to_lowercase();
        let mut every: Vec<(usize, &str)> = self
            .models
            .iter()
            .flat_map(Model::every_name)
            .map(|held| (distance(&name, &held.to_lowercase()), held))
            .collect();
        every.sort_by_key(|(distance, _)| *distance);
        let mut nearest: Vec<String> = vec![];
        for (_, held) in every {
            if nearest.len() < NEAREST && !nearest.iter().any(|had| had == held) {
                nearest.push(held.to_string());
            }
        }
        nearest
    }

    /// Its models' ids, in order.
    pub fn ids(&self) -> Vec<&str> {
        self.models.iter().map(|model| model.id.as_str()).collect()
    }
}

/// How many letters are to be added, taken away or changed to make `one`
/// `two`.
fn distance(one: &str, two: &str) -> usize {
    let two: Vec<char> = two.chars().collect();
    let mut row: Vec<usize> = (0..=two.len()).collect();
    for (i, a) in one.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, b) in two.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (diagonal + usize::from(a != *b)).min(above + 1).min(row[j] + 1);
            diagonal = above;
        }
    }
    row.last().copied().unwrap_or(0)
}

/// What a worker runs, and why.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Cast {
    pub harness: Harness,
    /// The model's id, where one `model:` label named it. `None`: the
    /// harness's user default, which the fleet does not name.
    pub model: Option<String>,
    pub why: Why,
    /// The label that was not followed, as the issue is told it once.
    /// `None` where none was, or where the label was followed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unmatched: Option<String>,
    /// What that once-comment says of the label: the nearest names, the two
    /// ids, the two labels, or that the role's range does not hold the match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
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

/// Why a worker runs the model it runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Why {
    /// One `model:` label named exactly one model of the selected harness
    /// that the role's range holds. The worker starts on that model's id.
    ModelLabel,
    /// Anything else: the harness's user default. A label that was not
    /// followed stays on the issue and is said once (`notice`).
    UserDefault,
}

impl Why {
    /// What a human is told of it.
    pub fn said(self) -> &'static str {
        match self {
            Why::ModelLabel => "its model label",
            Why::UserDefault => "the user's default",
        }
    }
}

/// `role`'s range on a host whose host kit holds `kit`, in its order: its
/// roster entry cut to the host kit (`cut`), less each harness the role
/// holds no limits for while it holds limits for another (`left_out`). No
/// worker of the role runs a harness it holds no limits for.
pub fn range(role: &Role, kit: &[Held]) -> Vec<Held> {
    crate::roster::allowed(role, &cut(role.roster.as_ref(), kit))
}

/// Each harness `role`'s roster entry cut to the host kit holds (`cut`) and
/// its range leaves out: one the role holds no limits for, while it holds
/// limits for another (`team::unlimited`).
pub fn left_out(role: &Role, kit: &[Held]) -> Vec<Harness> {
    let mut left = vec![];
    for held in cut(role.roster.as_ref(), kit) {
        if crate::team::unlimited(role, held.harness).is_some() {
            left.push(held.harness);
        }
    }
    left
}

/// Why a range leaves out `harness`, as a human is told it.
pub fn left_out_why(harness: Harness) -> String {
    format!("no limits for {}", harness.key())
}

/// A roster `entry` cut to the host kit `kit` (`cut_entry`). A role the
/// roster names none for has the host kit whole, and so has one whose entry
/// matches nothing in it (`unmatched_entry`): a name that matches nothing
/// never stops work.
fn cut(entry: Option<&Entry>, kit: &[Held]) -> Vec<Held> {
    let Some(entry) = entry else { return kit.to_vec() };
    let range = cut_entry(entry, kit);
    if range.is_empty() { kit.to_vec() } else { range }
}

/// Whether `role`'s roster entry names only harnesses and models the host
/// kit `kit` does not hold: its range is then the host kit whole, less what
/// it holds no limits for, as if the roster named no entry for it.
fn unmatched_entry(role: &Role, kit: &[Held]) -> bool {
    role.roster.as_ref().is_some_and(|entry| cut_entry(entry, kit).is_empty())
}

/// Each role of `team` whose roster entry matches nothing in the host kit
/// `kit` (`unmatched_entry`), as `check`, `plan` and `doctor` name it: with
/// what the entry allows and what the host kit holds.
pub fn unmatched_entries(team: &Team, kit: &[Held]) -> Vec<(String, String)> {
    let mut unmatched = vec![];
    for role in &team.roles {
        if unmatched_entry(role, kit) {
            let said = format!(
                "the roster's entry for the role `{}` allows it {}, none of which the host kit \
holds; it holds {}: the role ranges over the host kit whole, less what it holds no limits for",
                role.name,
                allows(role.roster.as_ref()),
                holds(kit)
            );
            unmatched.push((role.name.clone(), said));
        }
    }
    unmatched
}

/// A roster `entry` cut to the host kit `kit`, in the kit's order, each
/// harness once, with the models of it the entry allows and the host kit
/// holds — by any of its names (`Held::model`) — and a harness the entry
/// allows on any model, with every model the host kit holds for it. What
/// the host kit does not hold is cut.
fn cut_entry(entry: &Entry, kit: &[Held]) -> Vec<Held> {
    let mut range: Vec<Held> = vec![];
    for held in kit {
        let mut any_model = false;
        for allowed in &entry.run {
            if allowed.harness == held.harness && allowed.model.is_none() {
                any_model = true;
            }
        }
        if any_model {
            range.push(held.clone());
            continue;
        }
        let mut models = vec![];
        for model in &held.models {
            let mut permitted = false;
            for allowed in &entry.run {
                if allowed.harness == held.harness
                    && let Some(name) = &allowed.model
                    && let Ok(matched) = held.model(name)
                    && matched.id == model.id
                {
                    permitted = true;
                }
            }
            if permitted {
                models.push(model.clone());
            }
        }
        if !models.is_empty() {
            range.push(Held { harness: held.harness, models });
        }
    }
    range
}

/// One model a roster entry names that matches no model the host kit holds
/// for its harness (`Held::model`): what `check`, `plan` and `doctor` name,
/// and what a tick opens an issue for in the team's repo. A harness the host
/// kit does not hold is no name's fault, and none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnmatchedModel {
    pub role: String,
    pub harness: Harness,
    pub model: String,
    /// What the host kit holds for the harness (`holds`).
    pub holds: String,
    /// Why it matched nothing (`Unmatched::said`).
    pub said: String,
}

impl UnmatchedModel {
    /// The line that names it to a human.
    pub fn line(&self) -> String {
        format!(
            "the roster entry `{{ harness: {}, model: {} }}` of the role `{}` matches nothing: \
             {}; the host kit holds {}",
            self.harness.key(),
            self.model,
            self.role,
            self.said,
            self.holds
        )
    }
}

/// Each model `team`'s roster names that matches no model the host kit
/// `kit` holds for its harness, role by role in the roster's order.
pub fn unmatched_models(team: &Team, kit: &[Held]) -> Vec<UnmatchedModel> {
    let mut unmatched = vec![];
    for role in &team.roles {
        let Some(entry) = &role.roster else { continue };
        for allowed in &entry.run {
            let Some(model) = &allowed.model else { continue };
            let Some(held) = kit.iter().find(|held| held.harness == allowed.harness) else {
                continue;
            };
            if let Err(why) = held.model(model) {
                unmatched.push(UnmatchedModel {
                    role: role.name.clone(),
                    harness: allowed.harness,
                    model: model.clone(),
                    holds: holds(std::slice::from_ref(held)),
                    said: why.said(model, allowed.harness),
                });
            }
        }
    }
    unmatched
}

/// What `entry` allows, as a human is told it: the host kit whole where it
/// names none.
fn allows(entry: Option<&Entry>) -> String {
    let Some(entry) = entry else { return "the host kit whole".to_string() };
    let each: Vec<String> = entry
        .run
        .iter()
        .map(|allowed| match &allowed.model {
            Some(model) => format!("{} ({model})", allowed.harness.key()),
            None => format!("{} (any model)", allowed.harness.key()),
        })
        .collect();
    each.join("; ")
}

/// Why no worker of `role` can be started on a host whose host kit holds
/// `kit`, where none can: its range there holds nothing — it holds limits
/// for no harness the host kit holds, and no worker of it runs without its
/// limits. Its stages wait, and go on once the host kit holds one again;
/// nothing else waits for it.
pub fn nothing_safe(role: &Role, kit: &[Held]) -> Option<String> {
    if !range(role, kit).is_empty() {
        return None;
    }
    let limits: Vec<&str> = role.limits.iter().map(|harness| harness.key()).collect();
    let limits = if limits.is_empty() { "no harness".to_string() } else { limits.join(", ") };
    Some(format!(
        "{} has no harness on this host it holds limits for: it holds limits for {limits}, and \
the host kit holds {}. Its stages wait until the host kit holds one: add one with `afkconvoy \
kit`, or write the role's limits for one the host kit holds",
        role.name,
        holds(kit)
    ))
}

/// Each role of `team` no worker of which can be started on a host whose
/// host kit holds `kit` (`nothing_safe`), with why: what `doctor` and the
/// panel name while it lasts.
pub fn range_faults(team: &Team, kit: &[Held]) -> Vec<(String, String)> {
    let mut faults = vec![];
    for role in &team.roles {
        if let Some(why) = nothing_safe(role, kit) {
            faults.push((role.name.clone(), why));
        }
    }
    faults
}

/// What `range` holds, as a human is told it.
fn holds(range: &[Held]) -> String {
    if range.is_empty() {
        return "nothing".to_string();
    }
    let each: Vec<String> = range
        .iter()
        .map(|held| match held.models.as_slice() {
            [] => format!("{} (no model)", held.harness.key()),
            _ => format!("{} ({})", held.harness.key(), held.ids().join(", ")),
        })
        .collect();
    each.join("; ")
}

/// The selected harness's user default, with what an unfollowed label is told.
fn user_default(harness: Harness, unmatched: Option<String>, notice: Option<String>) -> Cast {
    Cast { harness, model: None, why: Why::UserDefault, unmatched, notice }
}

/// The model `labels` name of `held`, the selected harness as the host kit
/// holds it, where `allowed` is that harness in the role's range. One
/// `model:` label that names exactly one model `allowed` holds is followed
/// on its id. No label, two labels, no match, two matches, or a match the
/// range does not hold: the user default, and the label is said once.
fn decide(harness: Harness, labels: &[String], held: &Held, allowed: &Held) -> Cast {
    let names: Vec<&str> =
        labels.iter().filter_map(|label| label.strip_prefix(MODEL_LABEL)).collect();
    match names.as_slice() {
        [] => user_default(harness, None, None),
        [one, two, ..] => {
            let name = format!("`{MODEL_LABEL}{one}` and `{MODEL_LABEL}{two}`");
            let notice = format!(
                "it carries two model labels, {name}: one names its model, so neither is followed"
            );
            user_default(harness, Some(name), Some(notice))
        }
        [one] => match held.model(one) {
            Ok(model) if allowed.models.iter().any(|allowed| allowed.id == model.id) => Cast {
                harness,
                model: Some(model.id.clone()),
                why: Why::ModelLabel,
                unmatched: None,
                notice: None,
            },
            Ok(model) => {
                let name = format!("`{MODEL_LABEL}{one}`");
                let nearest = held.nearest(one);
                let nearest = if nearest.is_empty() {
                    "the selected harness holds no other name".to_string()
                } else {
                    format!(
                        "the nearest names the harness holds: {}",
                        nearest
                            .iter()
                            .map(|name| format!("`{name}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    )
                };
                let notice = format!(
                    "`{one}` matches `{}`, which the role's range does not hold; {nearest}",
                    model.id
                );
                user_default(harness, Some(name), Some(notice))
            }
            Err(why) => {
                let name = format!("`{MODEL_LABEL}{one}`");
                user_default(harness, Some(name), Some(why.said(one, harness)))
            }
        },
    }
}

/// Selects the first eligible harness in kit order. A `model:` label selects
/// a model only within that harness, and only where the role's range holds
/// it; anything else is the harness's user default. `kit` is what the host
/// kit holds, so a name can match a model the role's range does not. `range`
/// is that role's range, already cut to what can be started. The caller
/// removes paused or unstartable harnesses before asking.
pub fn preferred(labels: &[String], range: &[Held], kit: &[Held]) -> Result<Cast, String> {
    let allowed = kit
        .iter()
        .find_map(|candidate| range.iter().find(|held| held.harness == candidate.harness))
        .ok_or("no eligible harness in the host kit")?;
    let held = kit.iter().find(|held| held.harness == allowed.harness).unwrap_or(allowed);
    Ok(decide(allowed.harness, labels, held, allowed))
}

/// One issue's model-selection explanation, shared by plan, doctor and panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelStatus {
    pub number: u64,
    pub needed: bool,
    pub why: String,
    pub stages: Vec<ModelStage>,
    pub running: Option<ModelRun>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelRun {
    pub harness: Harness,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelStage {
    pub stage: String,
    pub harness: Option<Harness>,
    pub model: Option<String>,
    pub followed: bool,
    pub why: String,
    pub skipped: Vec<String>,
}

/// The name the removed caster's worktree was created under — with a `-<k>`
/// when the name was taken. One a past fleet left is the fleet's own, not a
/// worker's.
pub const CASTER_WORKTREE: &str = "convoy-caster";

/// How often a paused harness is probed, in seconds: one that answers is no
/// longer paused.
const PAUSE_PROBE_SECONDS: u64 = 3600;

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

/// What the comment opens with that tells an issue a name it was given
/// matched nothing held, and what ran instead.
const UNMATCHED_MARK: &str = "<!--convoy:unmatched-->";

/// The comment an issue gets the first time a worker is started on it while
/// `name`, a model label it carries, is not followed. `notice` says the
/// nearest names or the two ids. The worker runs the user's default of
/// `ran`'s harness in that same start. The label stays.
pub fn unmatched_comment(name: &str, ran: &Cast, notice: &str) -> String {
    let notice = notice.trim();
    let notice = if notice.is_empty() { String::new() } else { format!(" {notice}") };
    format!(
        "{UNMATCHED_MARK}\n{name} is not followed, so this worker runs the user's default of \
         `{}`.{notice} The label stays.",
        ran.harness.key()
    )
}

/// Whether one of `bodies` already told the issue that `name` matched
/// nothing (`unmatched_comment`): it is told once, not at every start.
pub fn unmatched_told<'a>(bodies: impl IntoIterator<Item = &'a str>, name: &str) -> bool {
    let head = format!("{UNMATCHED_MARK}\n{name} ");
    bodies.into_iter().any(|body| body.starts_with(&head))
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
        Ok(Cast { harness, model: model.map(str::to_string), why, unmatched: None, notice: None })
    }

    fn user(harness: Harness) -> Result<Cast, String> {
        ran(harness, None, Why::UserDefault)
    }

    fn followed(harness: Harness, model: &str) -> Result<Cast, String> {
        ran(harness, Some(model), Why::ModelLabel)
    }

    fn opus() -> Model {
        Model {
            id: "claude-opus-5-5".into(),
            names: vec!["opus".into(), "default".into(), "Opus 5.5".into()],
        }
    }

    fn entry(run: &[(Harness, Option<&str>)]) -> Entry {
        let run = run.iter().map(|(harness, model)| crate::team::Allowed {
            harness: *harness,
            model: model.map(str::to_string),
            good_at: None,
        });
        Entry { run: run.collect() }
    }

    #[test]
    fn a_model_is_matched_by_its_id_or_any_of_its_names_whatever_the_case() {
        let claude = Held { harness: Harness::ClaudeCode, models: vec![opus(), "haiku".into()] };
        for name in ["opus", "Opus 5.5", "claude-opus-5-5", "DEFAULT", " opus "] {
            assert_eq!(claude.model(name), Ok(&opus()), "{name}");
        }
        // The provider prefix may be left off where that leaves one match.
        let pi = Held {
            harness: Harness::Pi,
            models: vec!["openai-codex/gpt-6-sol".into(), "a/fast".into(), "b/fast".into()],
        };
        assert_eq!(pi.model("gpt-6-sol").map(|m| m.id.as_str()), Ok("openai-codex/gpt-6-sol"));
        assert_eq!(pi.model("A/FAST").map(|m| m.id.as_str()), Ok("a/fast"));
        // A name with two matches matches neither, and says which two.
        let two = pi.model("fast").unwrap_err();
        assert_eq!(two, Unmatched::Two("a/fast".into(), "b/fast".into()));
        assert_eq!(
            two.said("fast", Harness::Pi),
            "`fast` names two models of pi, `a/fast` and `b/fast`, and so neither: name one by \
             its id"
        );
        // One that matches nothing says the nearest names.
        let none = claude.model("opuss").unwrap_err();
        assert_eq!(none, Unmatched::None(vec!["opus".into(), "Opus 5.5".into(), "haiku".into()]));
        assert!(
            none.said("opuss", Harness::ClaudeCode)
                .ends_with("nearest names: `opus`, `Opus 5.5`, `haiku`")
        );
        let empty = Held { harness: Harness::Codex, models: vec![] };
        assert_eq!(empty.model("x"), Err(Unmatched::None(vec![])));
    }

    #[test]
    fn a_roster_entry_is_cut_by_any_name_and_one_that_names_nothing_is_said() {
        let team = crate::team::tests::rostered(
            "  dev: { run: [{ harness: claude-code, model: Opus 5.5 }, { harness: pi, model: \
             fast }, { harness: dsh, model: x }] }\n  reviewer: { run: [{ harness: \
             claude-code, model: opuss }] }\n",
        )
        .unwrap();
        let kit = vec![
            Held { harness: Harness::ClaudeCode, models: vec![opus()] },
            Held { harness: Harness::Pi, models: vec!["a/fast".into(), "b/fast".into()] },
        ];
        assert_eq!(super::range(&team.roles[0], &kit), [kit[0].clone()]);
        let unmatched = unmatched_models(&team, &kit);
        let said: Vec<(&str, &str)> =
            unmatched.iter().map(|entry| (entry.role.as_str(), entry.model.as_str())).collect();
        // A harness the host kit does not hold is no name's fault.
        assert_eq!(said, [("dev", "fast"), ("reviewer", "opuss")]);
        assert!(unmatched[0].said.contains("`a/fast` and `b/fast`"), "{}", unmatched[0].said);
        let line = unmatched[1].line();
        assert!(
            line.starts_with(
                "the roster entry `{ harness: claude-code, model: opuss }` of the role \
                 `reviewer` matches nothing: `opuss` names no model the host kit holds for \
                 claude-code; the nearest names: `opus`"
            ),
            "{line}"
        );
        assert!(line.ends_with("the host kit holds claude-code (claude-opus-5-5)"), "{line}");
    }

    #[test]
    fn a_paused_harness_is_probed_an_hour_after_it_was_paused_or_last_probed() {
        let mut pause = Paused { harness: Harness::Pi, since: 100, why: "x".into(), probed: None };
        assert!(!pause.due(100 + PAUSE_PROBE_SECONDS - 1));
        assert!(pause.due(100 + PAUSE_PROBE_SECONDS));
        pause.probed = Some(5000);
        assert!(!pause.due(5000 + PAUSE_PROBE_SECONDS - 1));
        assert!(pause.due(5000 + PAUSE_PROBE_SECONDS));
    }

    #[test]
    fn a_role_s_range_is_its_roster_entry_cut_to_the_host_kit_in_the_kit_s_order() {
        let kit = range();
        let held = |harness, models: &[&str]| Held {
            harness,
            models: models.iter().map(|model| Model::from(*model)).collect(),
        };
        // None named: the host kit whole.
        assert_eq!(super::cut(None, &kit), kit);
        let entry = entry(&[
            (Harness::Codex, Some("gpt-x")),
            (Harness::Pi, Some("c/d")),
            (Harness::Dsh, None),
            (Harness::Pi, Some("x/y")),
            (Harness::Codex, Some("gone")),
            (Harness::ClaudeCode, None),
            (Harness::Pi, None),
        ]);
        assert_eq!(
            super::cut(Some(&entry), &kit),
            [
                held(Harness::Pi, &["a/b", "c/d"]),
                held(Harness::Codex, &["gpt-x"]),
                held(Harness::ClaudeCode, &[]),
            ]
        );
        // An entry that matches nothing in the host kit: the host kit whole.
        let nothing = self::entry(&[(Harness::Dsh, None), (Harness::Codex, Some("gone"))]);
        assert_eq!(super::cut_entry(&nothing, &kit), []);
        assert_eq!(super::cut(Some(&nothing), &kit), kit);
    }

    #[test]
    fn a_roster_entry_that_matches_nothing_ranges_over_the_host_kit_and_is_named() {
        let mut team = crate::team::tests::rostered(
            "  dev: { run: [{ harness: dsh }, { harness: codex, model: gone }] }\n",
        )
        .unwrap();
        let kit = range();
        let dev = team.roles.iter().position(|role| role.name == "dev").unwrap();
        assert!(unmatched_entry(&team.roles[dev], &kit));
        assert_eq!(super::range(&team.roles[dev], &kit), kit);
        let named = unmatched_entries(&team, &kit);
        assert_eq!(named.len(), 1, "{named:?}");
        assert_eq!(named[0].0, "dev");
        assert!(
            named[0].1.starts_with(
                "the roster's entry for the role `dev` allows it dsh (any model); codex (gone), \
                 none of which the host kit holds; it holds pi (a/b, c/d)"
            ),
            "{}",
            named[0].1
        );
        // Less what it holds no limits for: still no refusal.
        team.roles[dev].limits = vec![Harness::Codex];
        assert_eq!(super::range(&team.roles[dev], &kit), kit[1..2].to_vec());
        assert!(range_faults(&team, &kit).is_empty());
        // An entry the host kit holds some of is no such entry.
        assert_eq!(unmatched_entries(&team, &[]).len(), 1);
        let held = [Held { harness: Harness::Dsh, models: vec![] }];
        assert!(!unmatched_entry(&team.roles[dev], &held));
    }

    #[test]
    fn a_range_leaves_out_a_harness_its_role_holds_no_limits_for_and_only_an_empty_one_refuses() {
        let mut team = crate::team::tests::rostered("  dev: { run: [{ harness: pi }] }\n").unwrap();
        let kit = range();
        // No limits at all: right for every harness.
        assert_eq!(super::range(&team.roles[1], &kit), kit);
        assert_eq!(left_out(&team.roles[1], &kit), []);
        // Limits for Codex and Claude Code: pi is left out of the host kit
        // whole, in the host kit's order.
        team.roles[1].limits = vec![Harness::ClaudeCode, Harness::Codex];
        assert_eq!(super::range(&team.roles[1], &kit), kit[1..].to_vec());
        assert_eq!(left_out(&team.roles[1], &kit), [Harness::Pi]);
        assert_eq!(left_out_why(Harness::Pi), "no limits for pi");
        assert!(range_faults(&team, &kit).is_empty());
        // A range left empty by that: the role's stages wait, saying so.
        team.roles[0].limits = vec![Harness::Dsh];
        assert_eq!(super::range(&team.roles[0], &kit), []);
        let faults = range_faults(&team, &kit);
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert_eq!(faults[0].0, team.roles[0].name);
        assert!(
            faults[0].1.starts_with(&format!(
                "{} has no harness on this host it holds limits for: it holds limits for dsh, \
                 and the host kit holds pi (a/b, c/d); codex (gpt-x); claude-code (no model). ",
                team.roles[0].name
            )),
            "{}",
            faults[0].1
        );
        // Once the host kit holds one, it has a range again.
        let dsh = [Held { harness: Harness::Dsh, models: vec![] }];
        assert_eq!(nothing_safe(&team.roles[0], &dsh), None);
    }

    #[test]
    fn an_issue_is_told_once_what_matched_nothing_and_what_ran_instead() {
        let ran = user(Harness::Pi).unwrap();
        let said = unmatched_comment(
            "`model:opus`",
            &ran,
            "`opus` names no model the host kit holds for pi; the nearest names: `a/b`",
        );
        assert!(
            said.starts_with(
                "<!--convoy:unmatched-->\n`model:opus` is not followed, so this worker runs the \
                 user's default of `pi`."
            ),
            "{said}"
        );
        assert!(said.contains("The label stays."), "{said}");
        assert!(said.contains("nearest names"), "{said}");
        assert!(unmatched_told(["x", said.as_str()], "`model:opus`"));
        assert!(!unmatched_told([said.as_str()], "`model:o`"));
        assert!(!unmatched_told(["`model:opus` matches nothing"], "`model:opus`"));
    }

    #[test]
    fn a_model_label_of_the_selected_harness_runs_its_id() {
        let range = vec![
            Held { harness: Harness::ClaudeCode, models: vec![opus()] },
            Held { harness: Harness::Pi, models: vec!["openai-codex/gpt-6-sol".into()] },
        ];
        let cast = |names: &[&str]| preferred(&labels(names), &range, &range);
        for name in ["model:opus", "model:Opus 5.5", "model:claude-opus-5-5", "model:OPUS"] {
            assert_eq!(
                cast(&["harness:claude-code", name]),
                followed(Harness::ClaudeCode, "claude-opus-5-5")
            );
        }
        // A harness: label does not select pi. gpt-6-sol is not of claude-code.
        let missed = cast(&["harness:pi", "model:gpt-6-sol"]).unwrap();
        assert_eq!(missed.harness, Harness::ClaudeCode);
        assert_eq!((missed.model, missed.why), (None, Why::UserDefault));
        assert_eq!(missed.unmatched.as_deref(), Some("`model:gpt-6-sol`"));
        assert!(missed.notice.unwrap().contains("names no model"));
        let pi = [Held { harness: Harness::Pi, models: vec!["a/fast".into(), "b/fast".into()] }];
        let two = preferred(&labels(&["model:fast"]), &pi, &pi).unwrap();
        assert_eq!((two.model, two.why), (None, Why::UserDefault));
        assert!(two.notice.unwrap().contains("`a/fast` and `b/fast`"));
        let same = cast(&["model:opus", "model:opus"]).unwrap();
        assert_eq!(same.model, None);
        assert_eq!(same.unmatched.as_deref(), Some("`model:opus` and `model:opus`"));
    }

    #[test]
    fn a_model_label_is_followed_only_on_the_selected_harness() {
        let range = range();
        let cast = |names: &[&str]| preferred(&labels(names), &range, &range);
        // harness: does not select codex, and gpt-x is not a model of pi.
        let missed = cast(&["bug", "harness:codex", "model:gpt-x"]).unwrap();
        assert_eq!(
            (missed.harness, missed.model, missed.why),
            (Harness::Pi, None, Why::UserDefault)
        );
        assert_eq!(missed.unmatched.as_deref(), Some("`model:gpt-x`"));
        assert_eq!(cast(&["harness:pi", "model:c/d"]), followed(Harness::Pi, "c/d"));
        // A harness label alone is not a model label.
        assert_eq!(cast(&["harness:claude-code"]), user(Harness::Pi));
    }

    #[test]
    fn an_issue_with_no_model_label_runs_the_user_default() {
        assert_eq!(preferred(&labels(&["bug"]), &range(), &range()), user(Harness::Pi));
        let bare = [Held { harness: Harness::Dsh, models: vec![] }];
        assert_eq!(preferred(&[], &bare, &bare), user(Harness::Dsh));
        assert!(preferred(&[], &[], &range()).unwrap_err().contains("no eligible harness"));
    }

    #[test]
    fn a_model_label_that_is_not_followed_names_the_label_and_runs_the_user_default() {
        let range = range();
        let cast = |names: &[&str]| preferred(&labels(names), &range, &range);
        assert_eq!(cast(&["harness:dsh"]), user(Harness::Pi));
        let opus = cast(&["harness:codex", "model:opus"]).unwrap();
        assert_eq!((opus.harness, opus.model, opus.why), (Harness::Pi, None, Why::UserDefault));
        assert_eq!(opus.unmatched.as_deref(), Some("`model:opus`"));
        assert!(opus.notice.unwrap().contains("nearest names"));
        assert_eq!(cast(&["harness:nope"]), user(Harness::Pi));
        assert_eq!(cast(&["harness:pi", "harness:codex"]), user(Harness::Pi));
        let gpt = cast(&["model:gpt-x"]).unwrap();
        assert_eq!(gpt.unmatched.as_deref(), Some("`model:gpt-x`"));
        assert_eq!(gpt.model, None);
    }

    #[test]
    fn a_stage_follows_its_model_label_only_within_its_role_s_range() {
        let range = range();
        // A range of one does not force that harness's model.
        let one = [Held { harness: Harness::Codex, models: vec!["gpt-x".into()] }];
        let within = |names: &[&str], role: &[Held]| preferred(&labels(names), role, &range);
        assert_eq!(within(&["harness:pi"], &one), user(Harness::Codex));
        assert_eq!(within(&["model:gpt-x"], &one), followed(Harness::Codex, "gpt-x"));
        // a/b is a model of pi, the selected harness. The harness label is ignored.
        assert_eq!(within(&["harness:codex", "model:a/b"], &range), followed(Harness::Pi, "a/b"));
        // A model the kit holds and the role's range does not is not followed.
        let role = [Held { harness: Harness::Pi, models: vec!["a/b".into()] }];
        let outside = within(&["model:c/d"], &role).unwrap();
        assert_eq!(
            (outside.harness, outside.model, outside.why),
            (Harness::Pi, None, Why::UserDefault)
        );
        assert_eq!(outside.unmatched.as_deref(), Some("`model:c/d`"));
        assert!(outside.notice.unwrap().contains("does not hold"));
    }
}
