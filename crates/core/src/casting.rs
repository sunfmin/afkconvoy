//! An issue's casting (docs/adr/0020): the harness and model its `harness:`
//! and `model:` labels name — and what a worker runs by it, within its range:
//! a role's roster entry cut to the host kit (`range`), and in an unbound
//! repo the host kit whole.

use flux_rs::attrs::*;
use serde::Serialize;

use crate::harness::Harness;
use crate::team::{Entry, Performer, Role, Team, Workflow};

/// What the label that names an issue's harness starts with.
pub const HARNESS_LABEL: &str = "harness:";
/// What the label that names its model starts with: the model is any of its
/// names on its harness (`Held::model`); the fleet writes its id.
pub const MODEL_LABEL: &str = "model:";

/// One harness of a range, with the models it holds, in order of preference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub harness: Harness,
    pub models: Vec<Model>,
}

/// One model a harness holds: its id — the model its probe answered as,
/// which a worker is started on — and the other names it goes by on that
/// harness, each of which answered as it, its display names among them.
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
        let whole = |model: &&Model| model.every_name().any(|n| n.eq_ignore_ascii_case(name));
        let unprefixed = |model: &&Model| {
            !name.contains('/')
                && model.every_name().any(|n| {
                    n.split_once('/').is_some_and(|(_, rest)| rest.eq_ignore_ascii_case(name))
                })
        };
        for matches in [&whole as &dyn Fn(&&Model) -> bool, &unprefixed] {
            let found: Vec<&Model> = self.models.iter().filter(matches).collect();
            match found.as_slice() {
                [] => continue,
                [one] => return Ok(one),
                [one, two, ..] => return Err(Unmatched::Two(one.id.clone(), two.id.clone())),
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
    row[two.len()]
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
    /// A bound repo's issue has no casting: its role's range's first.
    RangeFirst,
    /// Its role's range does not hold the casting: the range's first.
    Outside,
    /// Its role's range holds one harness and model, whatever the casting.
    Only,
}

impl Why {
    /// What a human is told of it.
    pub fn said(self) -> &'static str {
        match self {
            Why::Casting => "its casting labels",
            Why::First => "no casting: the host kit's first",
            Why::Switch => "this host's switch, over any casting",
            Why::Uncast => "the caster did not cast it in ten minutes: its range's first",
            Why::RangeFirst => "no casting: its role's range's first",
            Why::Outside => "its role's range does not hold its casting: the range's first",
            Why::Only => "its role's range holds one harness and model",
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

/// A roster `entry` cut to the host kit `kit`, in the roster's order, each
/// harness once, with the models of it the entry allows and the host kit
/// holds — by any of its names (`Held::model`) — and a harness the entry
/// allows on any model, with every model the host kit holds for it. A role
/// the roster names none for has the host kit whole. What the host kit does
/// not hold is cut.
fn cut(entry: Option<&Entry>, kit: &[Held]) -> Vec<Held> {
    let Some(entry) = entry else { return kit.to_vec() };
    let mut range: Vec<Held> = vec![];
    for allowed in &entry.run {
        let Some(held) = kit.iter().find(|held| held.harness == allowed.harness) else {
            continue;
        };
        let models: Vec<Model> = match &allowed.model {
            Some(model) => match held.model(model) {
                Ok(model) => vec![model.clone()],
                Err(_) => continue,
            },
            None => held.models.clone(),
        };
        match range.iter_mut().find(|had| had.harness == allowed.harness) {
            Some(had) => {
                for model in models {
                    if !had.models.contains(&model) {
                        had.models.push(model);
                    }
                }
            }
            None => range.push(Held { harness: allowed.harness, models }),
        }
    }
    range
}

/// One model a roster entry names that matches no model the host kit holds
/// for its harness (`Held::model`): what `check`, `plan` and `doctor` name,
/// and what a tick opens an issue for in the team's repo. A harness the host
/// kit does not hold is no name's fault, and none.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UnmatchedEntry {
    pub role: String,
    pub harness: Harness,
    pub model: String,
    /// What the host kit holds for the harness (`holds`).
    pub holds: String,
    /// Why it matched nothing (`Unmatched::said`).
    pub said: String,
}

impl UnmatchedEntry {
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
pub fn unmatched_entries(team: &Team, kit: &[Held]) -> Vec<UnmatchedEntry> {
    let mut unmatched = vec![];
    for role in &team.roles {
        let Some(entry) = &role.roster else { continue };
        for allowed in &entry.run {
            let Some(model) = &allowed.model else { continue };
            let Some(held) = kit.iter().find(|held| held.harness == allowed.harness) else {
                continue;
            };
            if let Err(why) = held.model(model) {
                unmatched.push(UnmatchedEntry {
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

/// What keeps `team`'s workers from starting on a host whose host kit holds
/// `kit`: each role whose range there holds nothing, with what its roster
/// entry allows, what the host kit holds, and what the range leaves out
/// (`left_out`).
pub fn range_faults(team: &Team, kit: &[Held]) -> Vec<String> {
    let mut faults = vec![];
    for role in &team.roles {
        if !range(role, kit).is_empty() {
            continue;
        }
        let mut sides = format!(
            "the roster allows it {}, and the host kit holds {}",
            allows(role.roster.as_ref()),
            holds(kit)
        );
        let left: Vec<String> = left_out(role, kit)
            .into_iter()
            .map(|harness| format!("{} ({})", harness.key(), left_out_why(harness)))
            .collect();
        if !left.is_empty() {
            sides.push_str(&format!("; its range leaves out {}", left.join(", ")));
        }
        faults.push(format!(
            "the role `{}` has an empty range on this host: {sides}. Add one it allows to the \
host kit with `afkconvoy kit`, or allow it one the host kit holds in the team's `roster`, or \
write the role's limits for one it allows",
            role.name
        ));
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
fn unmatched_in(ranges: &[&[Held]], harness: Harness, model: &str) -> Option<String> {
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

/// What a bound repo's worker of an issue labelled `labels` runs within its
/// role's `range`: a harness `chosen` by a switch, whatever the casting; else
/// a range of one, whatever the casting; else the casting, where the range
/// holds it; else the range's first. An error is an issue that is not
/// dispatched, with why: labels that are no casting, or an empty range.
pub fn cast_within(
    labels: &[String],
    range: &[Held],
    chosen: Option<Harness>,
) -> Result<Cast, String> {
    if chosen.is_some() {
        return cast(labels, range, chosen);
    }
    let named = casting(labels)?;
    let first = range.first().ok_or("its role's range holds nothing on this host")?;
    let model = first.models.first().map(|model| model.id.clone());
    let first = |why| Cast { harness: first.harness, model: model.clone(), why };
    if !choice(range) {
        return Ok(first(Why::Only));
    }
    let Some((harness, model)) = named else { return Ok(first(Why::RangeFirst)) };
    match held_cast(range, harness, model.as_ref()) {
        Some(model) => Ok(Cast { harness, model, why: Why::Casting }),
        None => Ok(first(Why::Outside)),
    }
}

/// Why an issue labelled `labels` is not dispatched where `ranges` are its
/// workflow's stages' — each with what names it — and none holds its
/// casting. `None` where one does, where it has no casting, and under a
/// switch, which chooses whatever the casting.
pub fn held_by_none(
    labels: &[String],
    ranges: &[(String, Vec<Held>)],
    chosen: Option<Harness>,
) -> Option<String> {
    if chosen.is_some() || ranges.is_empty() {
        return None;
    }
    let (harness, model) = match casting(labels) {
        Ok(Some(named)) => named,
        Ok(None) => return None,
        Err(why) => return Some(why),
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
    let unmatched = model
        .as_deref()
        .and_then(|model| unmatched_in(&all, harness, model))
        .map_or(String::new(), |said| format!(" ({said})"));
    Some(format!(
        "its casting `{named}` is held by no range of its workflow's stages{unmatched}: {}",
        each.join(" · ")
    ))
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

/// What an issue labelled `labels` on the frontier waits for, within `range`,
/// under the harness a switch `chosen`, with `caster` the last one started,
/// `now`. An issue that has a casting, a range with no choice to make and a
/// switch, which chooses whatever the casting, leave nothing to cast.
pub fn for_caster(
    number: u64,
    labels: &[String],
    choice: bool,
    chosen: Option<Harness>,
    caster: Option<&Caster>,
    now: u64,
) -> ForCaster {
    if chosen.is_some() || !uncast(labels) || !choice {
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
    !uncast(labels) && cast(labels, kit, None).is_err()
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
        // Whatever name it answered with, the label names the model's id.
        let cast = cast(&labels, range, None)?;
        labels.truncate(1);
        labels.extend(cast.model.iter().map(|model| format!("{MODEL_LABEL}{model}")));
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
         the harness and model whose worker suits the work best: how hard the work is and what \
         kind it is. Choose only among those listed, which are in order of preference: prefer \
         the earlier where the work does not call for another.\n\nHarnesses and models:\n",
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
            "\nEach issue below lists its stages and what each one's role may run. A stage \
             whose role may not run the casting runs its own first: choose what suits the \
             stages that have a choice to make.\n",
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

/// What the worker of an issue labelled `labels` runs within `range`, the
/// host kit's harnesses in order: a harness `chosen` by a switch, whatever the
/// casting; else the casting, where the range holds it; else, with no casting,
/// the range's first. An error is an issue that is not dispatched, with why:
/// a casting the range does not hold, or labels that are none.
pub fn cast(labels: &[String], range: &[Held], chosen: Option<Harness>) -> Result<Cast, String> {
    let named = casting(labels);
    if let Some(chosen) = chosen {
        // The model of another harness is not this one's; one of this harness
        // is started on the id of what it names.
        let named = match &named {
            Ok(Some((harness, Some(model)))) if *harness == chosen => {
                held_cast(range, chosen, Some(model)).flatten()
            }
            _ => None,
        };
        let model = named.or_else(|| {
            let first = range.first().filter(|held| held.harness == chosen)?;
            first.models.first().map(|model| model.id.clone())
        });
        return Ok(Cast { harness: chosen, model, why: Why::Switch });
    }
    let Some((harness, model)) = named? else {
        let first = range.first().ok_or("the host kit holds no harness")?;
        let model = first.models.first().map(|model| model.id.clone());
        return Ok(Cast { harness: first.harness, model, why: Why::First });
    };
    let Some(held) = range.iter().find(|held| held.harness == harness) else {
        return Err(format!(
            "its label `{HARNESS_LABEL}{}` names a harness the host kit does not hold; it holds: {}",
            harness.key(),
            holds(range)
        ));
    };
    let model = match &model {
        Some(model) => match held.model(model) {
            Ok(held) => Some(held.id.clone()),
            Err(why) => {
                return Err(format!(
                    "its label `{MODEL_LABEL}{model}`: {}; it holds: {}",
                    why.said(model, harness),
                    holds(range)
                ));
            }
        },
        None => None,
    };
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
    fn a_casting_names_its_model_by_any_name_and_runs_its_id() {
        let range = vec![
            Held { harness: Harness::ClaudeCode, models: vec![opus()] },
            Held { harness: Harness::Pi, models: vec!["openai-codex/gpt-6-sol".into()] },
        ];
        let cast = |names: &[&str]| cast(&labels(names), &range, None);
        for name in ["model:opus", "model:Opus 5.5", "model:claude-opus-5-5"] {
            assert_eq!(
                cast(&["harness:claude-code", name]),
                ran(Harness::ClaudeCode, Some("claude-opus-5-5"), Why::Casting)
            );
        }
        assert_eq!(
            cast(&["harness:pi", "model:gpt-6-sol"]),
            ran(Harness::Pi, Some("openai-codex/gpt-6-sol"), Why::Casting)
        );
        assert_eq!(
            cast_within(&labels(&["harness:claude-code", "model:OPUS"]), &range, None),
            ran(Harness::ClaudeCode, Some("claude-opus-5-5"), Why::Casting)
        );
        // A caster's casting is labelled with the id.
        let casting = Casting {
            issue: 7,
            harness: "claude-code".into(),
            model: Some("Opus 5.5".into()),
            why: "hard".into(),
        };
        assert_eq!(
            casting.labels(&range),
            Ok(labels(&["harness:claude-code", "model:claude-opus-5-5"]))
        );
        // A label that matches nothing says the nearest names.
        let said = cast(&["harness:claude-code", "model:opuss"]).unwrap_err();
        assert!(said.contains("the nearest names: `opus`"), "{said}");
        let ranges = vec![("build (dev)".to_string(), range.clone())];
        let none = held_by_none(&labels(&["harness:claude-code", "model:opuss"]), &ranges, None);
        assert!(none.unwrap().contains("the nearest names: `opus`"));
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
        let unmatched = unmatched_entries(&team, &kit);
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
            for_caster(number, &labels(names), choice(&range), None, caster, now)
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
        assert_eq!(for_caster(7, &[], choice(&alone), None, None, 0), ForCaster::Dispatch);
        assert_eq!(
            for_caster(7, &[], choice(&range), Some(Harness::Pi), None, 0),
            ForCaster::Dispatch
        );
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
    fn a_role_s_range_is_its_roster_entry_cut_to_the_host_kit_in_the_roster_s_order() {
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
                held(Harness::Codex, &["gpt-x"]),
                held(Harness::Pi, &["c/d", "a/b"]),
                held(Harness::ClaudeCode, &[]),
            ]
        );
        let nothing = self::entry(&[(Harness::Dsh, None), (Harness::Codex, Some("gone"))]);
        assert_eq!(super::cut(Some(&nothing), &kit), []);
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
        assert_eq!(range_faults(&team, &kit), Vec::<String>::new());
        // A range left empty by that refuses the start, saying so.
        team.roles[0].limits = vec![Harness::Codex];
        assert_eq!(super::range(&team.roles[0], &kit), []);
        let faults = range_faults(&team, &kit);
        assert_eq!(faults.len(), 1, "{faults:?}");
        assert!(
            faults[0].starts_with(
                "the role `dev` has an empty range on this host: the roster allows it pi (any \
                 model), and the host kit holds pi (a/b, c/d); codex (gpt-x); claude-code (no \
                 model); its range leaves out pi (no limits for pi). "
            ),
            "{}",
            faults[0]
        );
    }

    #[test]
    fn a_stage_runs_its_casting_where_its_range_holds_it_and_its_range_s_first_where_not() {
        let range = range();
        let cast = |names: &[&str]| cast_within(&labels(names), &range, None);
        assert_eq!(
            cast(&["harness:codex", "model:gpt-x"]),
            ran(Harness::Codex, Some("gpt-x"), Why::Casting)
        );
        assert_eq!(cast(&["harness:pi"]), ran(Harness::Pi, None, Why::Casting));
        assert_eq!(cast(&[]), ran(Harness::Pi, Some("a/b"), Why::RangeFirst));
        assert_eq!(cast(&["harness:dsh"]), ran(Harness::Pi, Some("a/b"), Why::Outside));
        assert_eq!(
            cast(&["harness:codex", "model:a/b"]),
            ran(Harness::Pi, Some("a/b"), Why::Outside)
        );
        assert!(cast(&["harness:nope"]).unwrap_err().contains("is no harness"));
        // A range of one is run whatever the casting.
        let one = [Held { harness: Harness::Codex, models: vec!["gpt-x".into()] }];
        assert_eq!(
            cast_within(&labels(&["harness:pi"]), &one, None),
            ran(Harness::Codex, Some("gpt-x"), Why::Only)
        );
        assert!(cast_within(&[], &[], None).unwrap_err().contains("holds nothing"));
        // A switch wins over both.
        assert_eq!(
            cast_within(&labels(&["harness:pi"]), &one, Some(Harness::Dsh)),
            ran(Harness::Dsh, None, Why::Switch)
        );
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
        assert_eq!(
            said,
            "its casting `harness:dsh` is held by no range of its workflow's stages: plan (dev): \
             codex (gpt-x) · review (reviewer): pi (a/b, c/d); codex (gpt-x); claude-code (no \
             model)"
        );
        assert!(none(&["harness:pi", "model:z"]).unwrap().contains("`harness:pi` and `model:z`"));
        assert_eq!(held_by_none(&labels(&["harness:dsh"]), &ranges, Some(Harness::Pi)), None);
    }
}
