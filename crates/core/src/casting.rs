//! What a worker runs (docs/adr/0020): the harness kit order and the role's
//! range already selected, and the model its one `model:` label names of that
//! harness — or that harness's user default. A `harness:` label never selects
//! a harness.

use flux_rs::attrs::*;
use serde::Serialize;

use crate::harness::Harness;
use crate::team::{Entry, Performer, Role, Team, Workflow};

/// What the label that names an issue's harness starts with. It never
/// selects a harness.
pub const HARNESS_LABEL: &str = "harness:";
/// What the label that names its model starts with: the model is any of its
/// names on the selected harness (`Held::model`). The label is left as written;
/// a match starts the worker on the model's id.
pub const MODEL_LABEL: &str = "model:";

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
    pub fn every_name(&self) -> impl Iterator<Item = &str> {
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

/// How long the issues a caster is to cast are held back for it, from its
/// start: after that they are dispatched uncast.
pub const CASTER_SECONDS: u64 = 600;

/// Whether the kit-selected harness of `range` has multiple models. Later
/// harnesses are fallback candidates, never alternatives for the caster.
pub fn choice(range: &[Held]) -> bool {
    range.first().is_some_and(|held| held.models.len() > 1)
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
pub fn unmatched_entry(role: &Role, kit: &[Held]) -> bool {
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

/// The range of each stage of `workflow`, one of `team`'s, that a role
/// performs, on a host whose host kit holds `kit`: each with the stage's
/// name and its role's, as `<stage> (<role>)`.
#[spec(fn(&Team[@roles], &Workflow{v: v.roles == roles}, &[Held]) -> Vec<(String, Vec<Held>)>)]
pub fn stage_ranges(team: &Team, workflow: &Workflow, kit: &[Held]) -> Vec<(String, Vec<Held>)> {
    let mut ranges = vec![];
    for stage in workflow.stages() {
        if let Performer::Role(work) = &stage.by {
            let role = team.role_of(work);
            let by = format!("{} ({})", stage.name, role.name);
            ranges.push((by, range(role, kit)));
        }
    }
    ranges
}

/// What `entry` allows, as a human is told it: the host kit whole where it
/// names none.
pub fn allows(entry: Option<&Entry>) -> String {
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

/// Whether any of `ranges` leaves a choice to make (`choice`).
pub fn any_choice(ranges: &[(String, Vec<Held>)]) -> bool {
    ranges.iter().any(|(_, range)| choice(range))
}

/// What `range` holds, as a human is told it.
pub fn holds(range: &[Held]) -> String {
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

/// The casting `harness` on `model` as `range` holds it: its harness, and
/// the id of the model `model` names there (`Held::model`). `None` where it
/// holds no such harness, or no model of it by that name.
fn held_cast(range: &[Held], harness: Harness, model: Option<&String>) -> Option<Option<String>> {
    let held = range.iter().find(|held| held.harness == harness)?;
    match model {
        Some(model) => held.model(model).ok().map(|model| Some(model.id.clone())),
        None => Some(None),
    }
}

/// Why no harness of `ranges` holds a model named `model` of `harness`, with
/// the nearest names they hold for it: what is said of a label that names
/// nothing.
fn nearest_in(ranges: &[&[Held]], harness: Harness, model: &str) -> Option<String> {
    let mut every = Held { harness, models: vec![] };
    for range in ranges {
        for held in range.iter().filter(|held| held.harness == harness) {
            for each in &held.models {
                if !every.models.contains(each) {
                    every.models.push(each.clone());
                }
            }
        }
    }
    every.model(model).err().map(|why| why.said(model, harness))
}

/// Whether `range` holds `harness`, on any model.
fn holds_harness(range: &[Held], harness: Harness) -> bool {
    range.iter().any(|held| held.harness == harness)
}

/// A name an issue or a host was given that matches nothing held: a
/// casting label, or a switch — with why, as a human is told it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unheld {
    /// The name, as a human is told it: `` `model:opus` ``.
    pub name: String,
    pub why: String,
}

/// How a switch to `chosen` that matches nothing held is named.
fn switch_named(chosen: Harness) -> String {
    format!("this host's switch to `{}`", chosen.key())
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

/// The model labels select within this harness alone. Labels for other
/// harnesses remain on the issue and resume matching when priority returns.
/// Two distinct matching models are ambiguous, so neither outranks kit order.
pub fn labelled_model(labels: &[String], held: &Held) -> Option<String> {
    let mut selected: Option<String> = None;
    for label in labels {
        let Some(name) = label.strip_prefix(MODEL_LABEL) else { continue };
        if let Ok(model) = held.model(name) {
            if selected.as_ref().is_some_and(|id| id != &model.id) {
                return None;
            }
            selected = Some(model.id.clone());
        }
    }
    selected
}

/// Whether any casting label was put on by a human. A caster may add a
/// different harness's model beside its own labels, but never replaces or
/// recasts human labels, including legacy harness labels.
pub fn human_casting(labels: &[String], bodies: &[&str]) -> bool {
    for label in labels {
        if !is_casting(label) {
            continue;
        }
        let named = format!("`{label}`");
        let mut owned = false;
        for body in bodies {
            if body.starts_with("Cast ")
                && let Some((head, _)) = body.split_once(": ")
                && head.contains(&named)
            {
                owned = true;
            }
        }
        if !owned {
            return true;
        }
    }
    false
}

/// The caster has work only where a selected harness offers multiple models
/// and this issue has no matching label of that harness already.
pub fn needs_model(labels: &[String], bodies: &[&str], ranges: &[(String, Vec<Held>)]) -> bool {
    !human_casting(labels, bodies)
        && ranges.iter().any(|(_, range)| {
            choice(range)
                && range.first().is_some_and(|held| labelled_model(labels, held).is_none())
        })
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

/// The harness a switch named, where `range` holds it, else the first
/// harness `range` holds. The model is the label or the user default of
/// that harness: a switch reorders the kit and does not name the model.
/// A switch to a harness the range does not hold leaves the label's notice
/// in place when there is one.
fn switched(
    labels: &[String],
    range: &[Held],
    kit: &[Held],
    chosen: Harness,
) -> Result<Cast, String> {
    if let Some(allowed) = range.iter().find(|held| held.harness == chosen) {
        let held = kit.iter().find(|held| held.harness == chosen).unwrap_or(allowed);
        return Ok(decide(chosen, labels, held, allowed));
    }
    let first = range.first().ok_or("its role's range holds nothing on this host")?;
    let held = kit.iter().find(|held| held.harness == first.harness).unwrap_or(first);
    let mut cast = decide(first.harness, labels, held, first);
    if cast.unmatched.is_none() {
        cast.unmatched = Some(switch_named(chosen));
        cast.notice = Some(format!(
            "{} names a harness the role's range does not hold",
            switch_named(chosen)
        ));
    }
    Ok(cast)
}

/// What a bound repo's worker of an issue labelled `labels` runs within its
/// role's `range`, on a host whose host kit holds `kit`. The harness is one
/// `chosen` by a switch where the kit holds it, else the first harness of
/// `range` that `kit` lists. The model is the issue's one `model:` label
/// where that harness holds it inside the range, else the user default.
/// An error is a range that holds nothing.
pub fn cast_within(
    labels: &[String],
    range: &[Held],
    kit: &[Held],
    chosen: Option<Harness>,
) -> Result<Cast, String> {
    if let Some(chosen) = chosen {
        return switched(labels, range, kit, chosen);
    }
    preferred(labels, range, kit)
        .map_err(|_| "its role's range holds nothing on this host".to_string())
}

/// The model was already chosen by `preferred`. Other stages do not recast
/// a label the selected harness did not follow.
pub fn unmatched_in(cast: Cast, _labels: &[String], _ranges: &[(String, Vec<Held>)]) -> Cast {
    cast
}

/// The name an issue labelled `labels` was given that matches nothing where
/// `ranges` are its workflow's stages' — each with what names it — and none
/// holds its casting, with why. `None` where one does, where it has no
/// casting, and under a switch, which chooses whatever the casting. It is
/// not followed while it matches nothing: each stage runs its range's
/// first.
pub fn held_by_none(
    labels: &[String],
    ranges: &[(String, Vec<Held>)],
    chosen: Option<Harness>,
) -> Option<Unheld> {
    if chosen.is_some() || ranges.is_empty() {
        return None;
    }
    let (harness, model) = match casting(labels) {
        Ok(Some(named)) => named,
        Ok(None) => return None,
        Err(unheld) => return Some(unheld),
    };
    if ranges.iter().any(|(_, range)| held_cast(range, harness, model.as_ref()).is_some()) {
        return None;
    }
    let mut named = format!("{HARNESS_LABEL}{}", harness.key());
    if let Some(model) = &model {
        named.push_str(&format!("` and `{MODEL_LABEL}{model}"));
    }
    let each: Vec<String> =
        ranges.iter().map(|(by, range)| format!("{by}: {}", holds(range))).collect();
    let all: Vec<&[Held]> = ranges.iter().map(|(_, range)| range.as_slice()).collect();
    let nearest = model
        .as_deref()
        .and_then(|model| nearest_in(&all, harness, model))
        .map_or(String::new(), |said| format!(" ({said})"));
    // The label that matched nothing: the model's, where a range holds the
    // harness.
    let name = match &model {
        Some(model) if ranges.iter().any(|(_, range)| holds_harness(range, harness)) => {
            format!("`{MODEL_LABEL}{model}`")
        }
        _ => format!("`{HARNESS_LABEL}{}`", harness.key()),
    };
    let why = format!(
        "its casting `{named}` is held by no range of its workflow's stages{nearest}: {}",
        each.join(" · ")
    );
    Some(Unheld { name, why })
}

/// Whether `labels` name no casting: no `harness:` label and no `model:`
/// one. An issue with either is a human's to change, never a caster's.
pub fn uncast(labels: &[String]) -> bool {
    !labels.iter().any(|label| is_casting(label))
}

/// Whether `label` is one of a casting: a `harness:` or a `model:` label.
pub fn is_casting(label: &str) -> bool {
    label.starts_with(HARNESS_LABEL) || label.starts_with(MODEL_LABEL)
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

/// What a frontier issue waits for, given its model-casting eligibility and
/// the last caster started on these candidates. Ineligible issues dispatch.
pub fn for_caster(number: u64, needed: bool, caster: Option<&Caster>, now: u64) -> ForCaster {
    if !needed {
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
    let mut kept = vec![];
    for label in labels {
        if !is_casting(label) {
            kept.push(label.clone());
        }
    }
    kept
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
    let mut casting = vec![];
    for label in labels {
        if is_casting(label) {
            casting.push(label.clone());
        }
    }
    if casting.is_empty() {
        return false;
    }
    let head = format!("Cast {}: ", said_labels(&casting));
    bodies.into_iter().any(|body| body.starts_with(&head))
}

/// Whether the casting `labels` carry names what `kit` does not hold: a
/// harness it holds none of, or a model it holds by no such name.
pub fn names_nothing(labels: &[String], kit: &[Held]) -> bool {
    !uncast(labels) && held_casting(labels, kit).is_err()
}

/// Whether an issue labelled `labels` is to be cast again: a caster cast it
/// to a harness that is now `paused`, or to what the host kit `kit` holds
/// nothing by (`names_nothing`), and no worker has `started` on it.
pub fn to_recast<'a>(
    labels: &[String],
    bodies: impl IntoIterator<Item = &'a str>,
    started: bool,
    paused: &[Paused],
    kit: &[Held],
) -> bool {
    !started
        && (paused_of(labels, paused).is_some() || names_nothing(labels, kit))
        && by_caster(labels, bodies)
}

/// What a caster puts on one issue: a model of its selected harness, with why.
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
        if self.why.trim().is_empty() {
            return Err("it says nothing of why".to_string());
        }
        let model = self.model.as_ref().ok_or("it names no model to choose")?;
        let labels =
            vec![format!("{HARNESS_LABEL}{}", self.harness), format!("{MODEL_LABEL}{model}")];
        // Whatever name it answered with, the label names the model's id.
        let held = held_casting(&labels, range).map_err(|unheld| unheld.why)?;
        Ok(held
            .and_then(|(_, model)| model)
            .map(|model| vec![format!("{MODEL_LABEL}{model}")])
            .unwrap_or_default())
    }
}

/// One issue a caster is to cast, as its prompt gives it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToCast {
    pub number: u64,
    pub title: String,
    pub body: String,
    /// Of a bound repo's issue, each stage of its workflow a role performs,
    /// with what its role's range holds (`holds`); none in an unbound repo.
    pub stages: Vec<String>,
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
/// `offered` holds in order of preference, how the team's roster says to
/// cast among them (`how`, each a role's), and the one JSON line a casting
/// it is to answer with.
pub fn caster_prompt(issues: &[ToCast], offered: &[Offered], how: &[String]) -> String {
    let mut prompt = String::from(
        "You are the caster of a fleet of coding agents. For each GitHub issue below, choose \
         a model within each stage's kit-selected harness: how hard the work is and what \
         kind it is. The harness is already chosen by this host; never choose another. \
         Choose only among the models listed for that issue and stage, in kit order: prefer \
         the earlier where the work does not call for another.\n\nModels of selected harnesses:\n",
    );
    for each in offered {
        let model = each.model.as_deref().map_or(String::new(), |model| format!(" {model}"));
        let good = each.good_at.as_deref().map_or(String::new(), |good| format!(": {good}"));
        prompt.push_str(&format!("- {}{model}{good}\n", each.harness.key()));
    }
    if !how.is_empty() {
        prompt.push_str("\nHow the team says to cast among them, by role:\n");
        for said in how {
            prompt.push_str(&format!("- {said}\n"));
        }
    }
    if issues.iter().any(|issue| !issue.stages.is_empty()) {
        prompt.push_str(
            "\nEach issue below lists only the selected harnesses still needing a model. \
             A stage already matched by a model label needs no casting.\n",
        );
    }
    prompt.push_str("\nIssues:\n");
    for issue in issues {
        let body: String = issue.body.chars().take(BODY_CHARS).collect();
        let cut = if body.len() < issue.body.len() { " […]" } else { "" };
        prompt.push_str(&format!("\n## #{} {}\n", issue.number, issue.title));
        for stage in &issue.stages {
            prompt.push_str(&format!("Stage {stage}\n"));
        }
        prompt.push_str(&format!("{}{cut}\n", body.trim()));
    }
    prompt.push_str(
        "\nAnswer only one JSON line per issue and selected harness: \
         {\"issue\": <number>, \"harness\": \"<selected harness>\", \"model\": \"<model>\", \
         \"why\": \"<one or two sentences>\"}. Only model labels are written.\n",
    );
    prompt
}

/// The casting `labels` name: a harness, and a model of it. `None` of an
/// issue that has none; an error names the label that names what cannot be
/// one, and why.
fn casting(labels: &[String]) -> Result<Option<(Harness, Option<String>)>, Unheld> {
    let named = |prefix: &str| -> Vec<&str> {
        labels.iter().filter_map(|label| label.strip_prefix(prefix)).collect()
    };
    let (harnesses, models) = (named(HARNESS_LABEL), named(MODEL_LABEL));
    if let [one, two, ..] = harnesses.as_slice() {
        let name = format!("`{HARNESS_LABEL}{one}` and `{HARNESS_LABEL}{two}`");
        let why = format!("it carries the labels {name}: one names its harness");
        return Err(Unheld { name, why });
    }
    if let [one, two, ..] = models.as_slice() {
        let name = format!("`{MODEL_LABEL}{one}` and `{MODEL_LABEL}{two}`");
        let why = format!("it carries the labels {name}: one names its model");
        return Err(Unheld { name, why });
    }
    let model = models.first().map(|model| model.to_string());
    let Some(key) = harnesses.first() else {
        return match model {
            Some(model) => Err(Unheld {
                name: format!("`{MODEL_LABEL}{model}`"),
                why: format!(
                    "its label `{MODEL_LABEL}{model}` names a model of no harness: add a \
                     `{HARNESS_LABEL}` label"
                ),
            }),
            None => Ok(None),
        };
    };
    let harness = Harness::read(key).ok_or_else(|| Unheld {
        name: format!("`{HARNESS_LABEL}{key}`"),
        why: format!("its label `{HARNESS_LABEL}{key}`: {}", Harness::unknown(key)),
    })?;
    Ok(Some((harness, model)))
}

/// The casting `labels` name, where `range`, the host kit's harnesses in
/// order, holds it: `None` of labels that name none. An error names the
/// label that matches nothing held, and what the host kit holds.
fn held_casting(
    labels: &[String],
    range: &[Held],
) -> Result<Option<(Harness, Option<String>)>, Unheld> {
    let Some((harness, model)) = casting(labels)? else { return Ok(None) };
    let Some(held) = range.iter().find(|held| held.harness == harness) else {
        let name = format!("`{HARNESS_LABEL}{}`", harness.key());
        let why = format!(
            "its label {name} names a harness the host kit does not hold; it holds: {}",
            holds(range)
        );
        return Err(Unheld { name, why });
    };
    // Whatever name it was given, it is cast on the model's id.
    let model = match &model {
        Some(model) => match held.model(model) {
            Ok(held) => Some(held.id.clone()),
            Err(unmatched) => {
                let name = format!("`{MODEL_LABEL}{model}`");
                let why = format!(
                    "its label {name}: {}; it holds: {}",
                    unmatched.said(model, harness),
                    holds(range)
                );
                return Err(Unheld { name, why });
            }
        },
        None => None,
    };
    Ok(Some((harness, model)))
}

/// What the worker of an issue labelled `labels` runs within `range`, the
/// host kit's harnesses in order. A harness `chosen` by a switch is that
/// harness where `range` holds it; otherwise the first harness. The model
/// is the issue's one `model:` label where that harness holds it, else the
/// user default. An error is a host kit that holds no harness.
pub fn cast(labels: &[String], range: &[Held], chosen: Option<Harness>) -> Result<Cast, String> {
    cast_within(labels, range, range, chosen)
        .map_err(|_| "the host kit holds no harness".to_string())
}

/// The name of `labels` that matches nothing `range`, the host kit's
/// harnesses in order, holds, with why: what an unbound repo's issue is told
/// as its worker runs the host kit's first instead. `None` where they name
/// no casting, or one it holds.
pub fn unheld(labels: &[String], range: &[Held]) -> Option<Unheld> {
    held_casting(labels, range).err()
}

/// What the comment opens with that tells an issue a name it was given
/// matched nothing held, and what ran instead.
pub const UNMATCHED_MARK: &str = "<!--convoy:unmatched-->";

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
    fn a_model_label_of_the_selected_harness_runs_its_id() {
        let range = vec![
            Held { harness: Harness::ClaudeCode, models: vec![opus()] },
            Held { harness: Harness::Pi, models: vec!["openai-codex/gpt-6-sol".into()] },
        ];
        let cast = |names: &[&str]| cast(&labels(names), &range, None);
        for name in ["model:opus", "model:Opus 5.5", "model:claude-opus-5-5"] {
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
        assert_eq!(
            cast_within(&labels(&["model:OPUS"]), &range, &range, None),
            followed(Harness::ClaudeCode, "claude-opus-5-5")
        );
        let pi = [Held { harness: Harness::Pi, models: vec!["a/fast".into(), "b/fast".into()] }];
        let two = super::cast(&labels(&["model:fast"]), &pi, None).unwrap();
        assert_eq!((two.model, two.why), (None, Why::UserDefault));
        assert!(two.notice.unwrap().contains("`a/fast` and `b/fast`"));
        let same = super::cast(&labels(&["model:opus", "model:opus"]), &range, None).unwrap();
        assert_eq!(same.model, None);
        assert_eq!(same.unmatched.as_deref(), Some("`model:opus` and `model:opus`"));
        // A caster's casting is labelled with the id.
        let casting = Casting {
            issue: 7,
            harness: "claude-code".into(),
            model: Some("Opus 5.5".into()),
            why: "hard".into(),
        };
        assert_eq!(casting.labels(&range), Ok(labels(&["model:claude-opus-5-5"])));
        // A label that matches nothing says the nearest names.
        let said = unheld(&labels(&["harness:claude-code", "model:opuss"]), &range).unwrap().why;
        assert!(said.contains("the nearest names: `opus`"), "{said}");
        let ranges = vec![("build (dev)".to_string(), range.clone())];
        let none = held_by_none(&labels(&["harness:claude-code", "model:opuss"]), &ranges, None);
        assert!(none.unwrap().why.contains("the nearest names: `opus`"));
    }

    #[test]
    fn a_caster_s_casting_that_names_nothing_held_is_cast_again_and_a_human_s_is_not() {
        let range = range();
        let cast = labels(&["ready", "harness:pi", "model:gone/x"]);
        let said = caster_comment(&labels(&["harness:pi", "model:gone/x"]), "small");
        assert!(names_nothing(&cast, &range));
        assert!(!names_nothing(&labels(&["harness:pi", "model:C/D"]), &range));
        assert!(!names_nothing(&labels(&["ready"]), &range));
        assert!(to_recast(&cast, [said.as_str()], false, &[], &range));
        assert!(!to_recast(&cast, [said.as_str()], true, &[], &range), "a worker started on it");
        assert!(!to_recast(&cast, ["please run it on pi"], false, &[], &range));
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
    fn a_model_label_is_followed_only_on_the_selected_harness() {
        let range = range();
        let cast = |names: &[&str]| cast(&labels(names), &range, None);
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
        assert_eq!(cast(&labels(&["bug"]), &range(), None), user(Harness::Pi));
        let bare = [Held { harness: Harness::Dsh, models: vec![] }];
        assert_eq!(cast(&[], &bare, None), user(Harness::Dsh));
        assert_eq!(cast(&[], &[], None), Err("the host kit holds no harness".to_string()));
    }

    #[test]
    fn a_model_label_that_is_not_followed_names_the_label_and_runs_the_user_default() {
        let range = range();
        let cast = |names: &[&str]| cast(&labels(names), &range, None);
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
        assert_eq!(unheld(&labels(&["harness:pi"]), &range), None);
        assert_eq!(unheld(&labels(&["bug"]), &range), None);
        assert_eq!(unheld(&labels(&["harness:dsh"]), &range).unwrap().name, "`harness:dsh`");
    }

    #[test]
    fn a_casting_the_host_kit_does_not_hold_says_which_label_and_what_it_holds() {
        let range = range();
        let refused = |names: &[&str]| held_casting(&labels(names), &range).unwrap_err().why;
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
                "its label `model:a/b`: `a/b` names no model the host kit holds for codex; the \
                 nearest names: `gpt-x`; it holds: {holds}"
            )
        );
        assert!(refused(&["harness:claude-code", "model:opus"]).contains("`model:opus`"));
        assert!(refused(&["harness:nope"]).contains("`nope` is no harness"));
        assert!(refused(&["model:gpt-x"]).contains("names a model of no harness"));
        assert!(refused(&["harness:pi", "harness:codex"]).contains("one names its harness"));
        assert!(refused(&["harness:pi", "model:a/b", "model:c/d"]).contains("one names its model"));
    }

    #[test]
    fn a_switch_selects_the_harness_and_the_model_label_still_decides() {
        let range = range();
        let cast = |names: &[&str], chosen| cast(&labels(names), &range, Some(chosen));
        assert_eq!(
            cast(&["harness:codex", "model:gpt-x"], Harness::Codex),
            followed(Harness::Codex, "gpt-x")
        );
        let claude = cast(&["harness:codex", "model:gpt-x"], Harness::ClaudeCode).unwrap();
        assert_eq!(
            (claude.harness, claude.model, claude.why),
            (Harness::ClaudeCode, None, Why::UserDefault)
        );
        assert_eq!(claude.unmatched.as_deref(), Some("`model:gpt-x`"));
        assert_eq!(cast(&["harness:dsh"], Harness::Pi), user(Harness::Pi));
        assert_eq!(cast(&[], Harness::Codex), user(Harness::Codex));
        let away = cast(&["harness:codex"], Harness::Dsh).unwrap();
        assert_eq!((away.harness, away.model, away.why), (Harness::Pi, None, Why::UserDefault));
        assert_eq!(away.unmatched.as_deref(), Some("this host's switch to `dsh`"));
    }

    #[test]
    fn a_range_of_more_than_one_harness_and_model_leaves_a_choice_to_make() {
        assert!(choice(&range()));
        let one = |models: &[&str]| {
            let models = models.iter().map(|model| Model::from(*model)).collect();
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
            for_caster(number, uncast(&labels(names)) && choice(&range), caster, now)
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
        assert_eq!(for_caster(7, choice(&alone), None, 0), ForCaster::Dispatch);
        assert_eq!(for_caster(7, false, None, 0), ForCaster::Dispatch);
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
        assert_eq!(casting("pi", Some("c/d"), "small").labels(&range), Ok(labels(&["model:c/d"])));
        assert!(casting("codex", None, "big").labels(&range).is_err());
        assert!(
            casting("dsh", Some("x"), "x").labels(&range).unwrap_err().contains("does not hold")
        );
        assert!(casting("pi", Some("x/y"), "x").labels(&range).is_err());
        assert!(casting("pi", None, " ").labels(&range).unwrap_err().contains("why"));
    }

    #[test]
    fn a_caster_s_casting_to_a_paused_harness_is_cast_again_until_a_worker_started() {
        let paused =
            [Paused { harness: Harness::Pi, since: 10, why: "refused".into(), probed: None }];
        let cast = labels(&["ready", "harness:pi", "model:a/b"]);
        let said = caster_comment(&labels(&["harness:pi", "model:a/b"]), "small ");
        assert_eq!(said, "Cast `harness:pi` and `model:a/b`: small");
        assert!(to_recast(&cast, [said.as_str()], false, &paused, &range()));
        assert!(
            !to_recast(&cast, [said.as_str()], true, &paused, &range()),
            "a worker started on it"
        );
        // A human's casting is never cast again.
        assert!(!to_recast(&cast, ["Cast it to pi, please"], false, &paused, &range()));
        // Nor one to a harness that is not paused.
        let codex = labels(&["harness:codex"]);
        assert!(!to_recast(&codex, ["Cast `harness:codex`: big"], false, &paused, &range()));
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
    }

    #[test]
    fn a_caster_s_prompt_gives_each_issue_and_each_harness_and_model_in_order() {
        let issues = [ToCast {
            number: 7,
            title: "Fix it".into(),
            body: "x".repeat(4000),
            stages: vec!["build (dev): pi (a/b)".into()],
        }];
        let offered = [
            Offered {
                harness: Harness::Pi,
                model: Some("a/b".into()),
                good_at: Some("speed".into()),
            },
            Offered { harness: Harness::Codex, model: None, good_at: None },
        ];
        let prompt = caster_prompt(&issues, &offered, &["dev: small work on pi".into()]);
        assert!(prompt.contains("- pi a/b: speed\n- codex\n"), "{prompt}");
        assert!(prompt.contains("## #7 Fix it\nStage build (dev): pi (a/b)\n"), "{prompt}");
        assert!(prompt.contains("- dev: small work on pi\n"), "{prompt}");
        assert!(prompt.contains(" […]"), "a long body is cut");
        assert!(prompt.len() < 4000, "{}", prompt.len());
    }

    fn entry(run: &[(Harness, Option<&str>)]) -> Entry {
        let run = run.iter().map(|(harness, model)| crate::team::Allowed {
            harness: *harness,
            model: model.map(str::to_string),
            good_at: None,
        });
        Entry { cast: None, run: run.collect() }
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
    fn a_stage_follows_its_model_label_or_runs_the_user_default() {
        let range = range();
        let cast = |names: &[&str]| cast_within(&labels(names), &range, &range, None);
        let missed = cast(&["harness:codex", "model:gpt-x"]).unwrap();
        assert_eq!(
            (missed.harness, missed.model, missed.why),
            (Harness::Pi, None, Why::UserDefault)
        );
        assert_eq!(missed.unmatched.as_deref(), Some("`model:gpt-x`"));
        assert_eq!(cast(&["harness:pi"]), user(Harness::Pi));
        assert_eq!(cast(&[]), user(Harness::Pi));
        assert_eq!(cast(&["harness:dsh"]), user(Harness::Pi));
        // a/b is a model of pi, the selected harness. The harness label is ignored.
        assert_eq!(cast(&["harness:codex", "model:a/b"]), followed(Harness::Pi, "a/b"));
        assert_eq!(cast(&["harness:nope"]), user(Harness::Pi));
        // A range of one does not force that harness's model.
        let one = [Held { harness: Harness::Codex, models: vec!["gpt-x".into()] }];
        assert_eq!(cast_within(&labels(&["harness:pi"]), &one, &range, None), user(Harness::Codex));
        assert_eq!(
            cast_within(&labels(&["harness:nope"]), &one, &range, None),
            user(Harness::Codex)
        );
        assert_eq!(
            cast_within(&labels(&["model:gpt-x"]), &one, &range, None),
            followed(Harness::Codex, "gpt-x")
        );
        // A model the kit holds and the role's range does not is not followed.
        let role = [Held { harness: Harness::Pi, models: vec!["a/b".into()] }];
        let outside = cast_within(&labels(&["model:c/d"]), &role, &range, None).unwrap();
        assert_eq!(
            (outside.harness, outside.model, outside.why),
            (Harness::Pi, None, Why::UserDefault)
        );
        assert_eq!(outside.unmatched.as_deref(), Some("`model:c/d`"));
        assert!(outside.notice.unwrap().contains("does not hold"));
        assert!(cast_within(&[], &[], &range, None).unwrap_err().contains("holds nothing"));
        let to_pi =
            cast_within(&labels(&["harness:codex"]), &one, &range, Some(Harness::Pi)).unwrap();
        assert_eq!(to_pi.harness, Harness::Codex);
        assert_eq!(to_pi.model, None);
        assert_eq!(to_pi.unmatched.as_deref(), Some("this host's switch to `pi`"));
        let to_dsh =
            cast_within(&labels(&["harness:pi"]), &one, &range, Some(Harness::Dsh)).unwrap();
        assert_eq!(to_dsh.harness, Harness::Codex);
        assert_eq!(to_dsh.unmatched.as_deref(), Some("this host's switch to `dsh`"));
    }

    #[test]
    fn an_unfollowed_label_is_not_recast_by_another_stage() {
        let mut two = range();
        two.truncate(2);
        let ranges = vec![("plan (dev)".to_string(), range()), ("review (r)".to_string(), two)];
        let labels_cc = labels(&["harness:claude-code"]);
        let outside = cast_within(&labels_cc, &ranges[1].1, &range(), None).unwrap();
        assert_eq!(unmatched_in(outside, &labels_cc, &ranges), user(Harness::Pi).unwrap());
        let opus = labels(&["harness:pi", "model:opus"]);
        let first = cast_within(&opus, &ranges[0].1, &range(), None).unwrap();
        assert_eq!(first.unmatched.as_deref(), Some("`model:opus`"));
        assert_eq!(first.model, None);
        assert_eq!(unmatched_in(first.clone(), &opus, &ranges), first);
    }

    #[test]
    fn a_casting_no_stage_s_range_holds_is_said_with_each_range() {
        let one = vec![Held { harness: Harness::Codex, models: vec!["gpt-x".into()] }];
        let ranges =
            vec![("plan (dev)".to_string(), one), ("review (reviewer)".to_string(), range())];
        let none = |names: &[&str]| held_by_none(&labels(names), &ranges, None);
        assert_eq!(none(&[]), None);
        assert_eq!(none(&["harness:codex", "model:gpt-x"]), None);
        assert_eq!(none(&["harness:pi", "model:c/d"]), None);
        let said = none(&["harness:dsh"]).unwrap();
        assert_eq!(said.name, "`harness:dsh`");
        assert_eq!(
            said.why,
            "its casting `harness:dsh` is held by no range of its workflow's stages: plan (dev): \
             codex (gpt-x) · review (reviewer): pi (a/b, c/d); codex (gpt-x); claude-code (no \
             model)"
        );
        let model = none(&["harness:pi", "model:z"]).unwrap();
        assert!(model.why.contains("`harness:pi` and `model:z`"));
        assert_eq!(model.name, "`model:z`");
        assert_eq!(held_by_none(&labels(&["harness:dsh"]), &ranges, Some(Harness::Pi)), None);
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
}
