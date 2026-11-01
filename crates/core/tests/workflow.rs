//! What a workflow promises about the paths through its stages, held on
//! generated definitions: Flux states a rule of one stage, and these are rules
//! of the stages together. Every definition that loads is held to them, and
//! every list of verdicts to what its workflow allows.

use std::fmt::Write as _;

use afkconvoy_core::config::{RepoDir, RepoLabels};
use afkconvoy_core::position::{Breaker, ConsultOn, Declared, Place, Position};
use afkconvoy_core::team::{
    DONE, Fault, File, LAND, Pass, RoleDir, Sources, TEAM_FILE, Team, Workflow, load,
};
use proptest::prelude::*;
use proptest::strategy::ValueTree;
use proptest::test_runner::{Config, TestRunner};

/// The workflow every generated definition is the file of.
const NAME: &str = "generated";
/// A target that names no stage, whatever the stages are.
const NOWHERE: &str = "nowhere";

/// Where a generated `pass` or `reject` leads.
#[derive(Debug, Clone, Copy)]
enum Target {
    /// The stage after this one, or the end of the workflow from the last.
    Next,
    Land,
    Done,
    /// A stage before this one, chosen by this number; its own from the first.
    Before(usize),
    /// Any stage, chosen by this number: its own, an earlier or a later one.
    At(usize),
    Nowhere,
}

/// A skill of a role, as a definition names it: either may not exist.
type Named = (&'static str, &'static str);

#[derive(Debug, Clone)]
struct StageDefinition {
    name: &'static str,
    /// `None` for a human's stage.
    by: Option<Named>,
    writes: Option<bool>,
    consult: Option<Named>,
    pass: Target,
    reject: Option<Target>,
}

/// One workflow file, as it would be written.
#[derive(Clone)]
struct Definition {
    /// Each breaker as its key's value; `None` where the key is left out.
    max_rework: Option<&'static str>,
    max_consecutive_rework: Option<&'static str>,
    stages: Vec<StageDefinition>,
    /// Where the last stage's `pass` leads when it leads on: `land` or `done`.
    end: &'static str,
    land: Option<Named>,
}

impl Definition {
    /// What `target` of the stage at `at` is written as.
    fn written(&self, at: usize, target: Target) -> &'static str {
        let name = |at: usize| self.stages.get(at).map_or(NOWHERE, |stage| stage.name);
        match target {
            Target::Next if at + 1 < self.stages.len() => name(at + 1),
            Target::Next => self.end,
            Target::Land => LAND,
            Target::Done => DONE,
            Target::Before(pick) => name(pick % at.max(1)),
            Target::At(pick) => name(pick % self.stages.len().max(1)),
            Target::Nowhere => NOWHERE,
        }
    }

    /// The file's yaml.
    fn yaml(&self) -> String {
        let mut yaml = String::new();
        if let Some(max) = self.max_rework {
            writeln!(yaml, "max_rework: {max}").unwrap();
        }
        if let Some(max) = self.max_consecutive_rework {
            writeln!(yaml, "max_consecutive_rework: {max}").unwrap();
        }
        yaml.push_str("stages:\n");
        for (at, stage) in self.stages.iter().enumerate() {
            writeln!(yaml, "  - name: {}", stage.name).unwrap();
            match stage.by {
                Some((role, skill)) => writeln!(yaml, "    by: {role}\n    do: {skill}").unwrap(),
                None => writeln!(yaml, "    by: human").unwrap(),
            }
            if let Some(writes) = stage.writes {
                writeln!(yaml, "    writes: {writes}").unwrap();
            }
            if let Some((role, skill)) = stage.consult {
                writeln!(yaml, "    escalate: {{ to: {role}, do: {skill} }}").unwrap();
            }
            let pass = self.written(at, stage.pass);
            match stage.reject {
                Some(reject) => {
                    let reject = self.written(at, reject);
                    writeln!(yaml, "    on: {{ pass: {pass}, reject: {reject} }}").unwrap();
                }
                None => writeln!(yaml, "    on: {{ pass: {pass} }}").unwrap(),
            }
        }
        if let Some((role, skill)) = self.land {
            writeln!(yaml, "land: {{ by: {role}, do: {skill} }}").unwrap();
        }
        yaml
    }
}

/// A counterexample is shown as the file it is.
impl std::fmt::Debug for Definition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "\n{}", self.yaml())
    }
}

/// A breaker as written: left out, a small count, or no count at all.
fn breaker() -> impl Strategy<Value = Option<&'static str>> {
    prop_oneof![
        16 => prop::sample::select(vec!["0", "1", "2", "3"]).prop_map(Some),
        7 => Just(None),
        1 => prop::sample::select(vec!["-1", "many"]).prop_map(Some),
    ]
}

/// Who performs something: a skill of a role that has it, mostly.
fn named(skills: &'static [Named]) -> impl Strategy<Value = Named> {
    prop_oneof![
        39 => prop::sample::select(skills),
        1 => prop::sample::select(vec![("ghost", "plan"), ("dev", "juggle")]),
    ]
}

/// A stage's name: its own, mostly — or one another stage may have, or a word
/// that means something in every workflow.
fn name(at: usize) -> impl Strategy<Value = &'static str> {
    const OWN: [&str; 5] = ["a", "b", "c", "d", "e"];
    prop_oneof![
        39 => Just(OWN[at]),
        1 => prop::sample::select(vec!["a", "b", LAND, DONE, "human"]),
    ]
}

fn pass() -> impl Strategy<Value = Target> {
    prop_oneof![
        24 => Just(Target::Next),
        2 => Just(Target::Land),
        2 => Just(Target::Done),
        2 => (0..6usize).prop_map(Target::At),
        1 => Just(Target::Nowhere),
    ]
}

/// The `reject` of the stage at `at`: the first stage has none before it to
/// lead to, so it seldom has one.
fn reject(at: usize) -> impl Strategy<Value = Option<Target>> {
    let before = if at == 0 { 1 } else { 16 };
    prop_oneof![
        12 => Just(None),
        before => (0..6usize).prop_map(|pick| Some(Target::Before(pick))),
        2 => (0..6usize).prop_map(|pick| Some(Target::At(pick))),
        1 => Just(Some(Target::Nowhere)),
    ]
}

fn stage(at: usize) -> impl Strategy<Value = StageDefinition> {
    const STAGE: &[Named] = &[("dev", "plan"), ("dev", "implement"), ("reviewer", "review")];
    const CONSULT: &[Named] = &[("reviewer", "rule")];
    let by = prop_oneof![5 => named(STAGE).prop_map(Some), 1 => Just(None)];
    let writes = prop_oneof![3 => Just(None), 1 => any::<bool>().prop_map(Some)];
    let consult = prop_oneof![3 => Just(None), 1 => named(CONSULT).prop_map(Some)];
    (name(at), by, writes, consult, pass(), reject(at)).prop_map(
        |(name, by, writes, consult, pass, reject)| StageDefinition {
            name,
            by,
            writes,
            consult,
            pass,
            reject,
        },
    )
}

/// A workflow file: well formed or not, with targets that name no stage,
/// rejects that lead forward, stages nothing reaches, and `pass`es that come
/// round.
fn definition() -> impl Strategy<Value = Definition> {
    const LANDING: &[Named] = &[("dev", "land")];
    let count = prop_oneof![1 => Just(0usize), 24 => 1..=5usize];
    let stages = count.prop_flat_map(|count| (0..count).map(stage).collect::<Vec<_>>());
    // Whether `land` is set exactly when a `pass` leads there, as it must be.
    let fits = prop::bool::weighted(0.95);
    let end = prop::sample::select(vec![LAND, DONE]);
    (breaker(), breaker(), stages, end, named(LANDING), fits).prop_map(
        |(max_rework, max_consecutive_rework, stages, end, landing, fits)| {
            let mut definition =
                Definition { max_rework, max_consecutive_rework, stages, end, land: None };
            let lands = (0..definition.stages.len())
                .any(|at| definition.written(at, definition.stages[at].pass) == LAND);
            definition.land = (lands == fits).then_some(landing);
            definition
        },
    )
}

/// One thing declared on an issue.
#[derive(Debug, Clone, Copy)]
enum Step {
    /// This, whether or not it may be declared where the issue is.
    Just(Declared),
    /// Whatever makes a stage run again from where the issue is: few lists of
    /// verdicts chosen blind come to a breaker.
    Rework,
}

impl Step {
    /// What is declared on an issue at `place`.
    fn declared(self, place: &Place) -> Declared {
        match self {
            Step::Just(declared) => declared,
            Step::Rework => [Declared::Reject, Declared::Ruling]
                .into_iter()
                .find(|declared| place.takes(*declared))
                .unwrap_or(Declared::Pass),
        }
    }
}

/// Everything declared on one issue, oldest first.
fn verdicts() -> impl Strategy<Value = Vec<Step>> {
    let step = prop_oneof![
        6 => Just(Step::Just(Declared::Pass)),
        20 => Just(Step::Rework),
        1 => Just(Step::Just(Declared::Reject)),
        1 => Just(Step::Just(Declared::Ruling)),
        1 => Just(Step::Just(Declared::Escalate)),
        1 => Just(Step::Just(Declared::Blocked)),
        1 => Just(Step::Just(Declared::AlreadySatisfied)),
    ];
    prop::collection::vec(step, 0..48)
}

/// The team whose one workflow file is `definition`, or everything wrong with it.
fn team_of(definition: &Definition) -> Result<Team, Vec<Fault>> {
    let file = |path: &str, yaml: &str| File {
        path: path.to_string(),
        prose: String::new(),
        yaml: serde_yaml::from_str(yaml).map_err(|error| error.to_string()),
    };
    let repo = |name: &str, skills: &[&str]| {
        let skills = skills.iter().map(|skill| skill.to_string()).collect();
        (
            RepoDir::read(name).unwrap(),
            Ok(RoleDir { commit: format!("{name}@1"), skills, ..RoleDir::default() }),
        )
    };
    load(&Sources {
        team: file(TEAM_FILE, "roles:\n  dev: acme/dev\n  reviewer: acme/reviewer\n"),
        workflows: vec![file(&format!("workflows/{NAME}.md"), &definition.yaml())],
        patrols: vec![],
        at: None,
        rules: false,
        roles: vec![
            repo("acme/dev", &["plan", "implement", "land"]),
            repo("acme/reviewer", &["review", "rule"]),
        ],
        labels: RepoLabels::default(),
    })
}

/// The workflow `team` loaded from a generated definition; `None` for a
/// definition that did not load.
fn loaded(team: &Result<Team, Vec<Fault>>) -> Option<&Workflow> {
    team.as_ref().ok()?.workflow(NAME)
}

/// The places of the stages that can be reached from the first.
fn reachable(workflow: &Workflow) -> Vec<usize> {
    let mut found: Vec<usize> = vec![];
    let mut todo = vec![0];
    while let Some(at) = todo.pop() {
        if found.contains(&at) {
            continue;
        }
        found.push(at);
        let Some(stage) = workflow.stages().get(at) else { continue };
        if let Pass::Stage(next) = stage.pass() {
            todo.push(next);
        }
        todo.extend(stage.reject());
    }
    found
}

/// How many `pass`es from the stage at `from` come to the landing train or to
/// nothing more to do; `None` when as many as there are stages do not.
fn passes_to_an_end(workflow: &Workflow, from: usize) -> Option<usize> {
    let mut at = from;
    for steps in 1..=workflow.stages().len() {
        match workflow.stages().get(at)?.pass() {
            Pass::Stage(next) => at = next,
            Pass::Land | Pass::Done => return Some(steps),
        }
    }
    None
}

/// Whether an issue at `position` has come to a stage that is to run.
fn runs(position: Position) -> bool {
    matches!(position, Position::Perform(..) | Position::Waiting(_))
}

proptest! {
    // A failure is shown, shrunk, by the run that found it: nothing is kept
    // in the tree for the next run to read.
    #![proptest_config(Config { cases: 1024, failure_persistence: None, ..Config::default() })]

    #[test]
    fn a_workflow_that_loads_keeps_its_graph_rules(definition in definition()) {
        let team = team_of(&definition);
        let Some(workflow) = loaded(&team) else { return Ok(()) };
        let stages = workflow.stages();
        prop_assert!(!stages.is_empty(), "it has no first stage");
        let reachable = reachable(workflow);
        for (at, stage) in stages.iter().enumerate() {
            let name = &stage.name;
            prop_assert_eq!(stage.at(), at, "`{}` is not at its own place", name);
            prop_assert!(reachable.contains(&at), "no path from the first stage reaches `{}`", name);
            prop_assert!(
                passes_to_an_end(workflow, at).is_some(),
                "following `pass` from `{}` comes to no end within {} steps", name, stages.len()
            );
            if let Some(back) = stage.reject() {
                prop_assert!(back < at, "the `reject` of `{}` leads to no earlier stage", name);
            }
        }
        let lands = stages.iter().any(|stage| stage.pass() == Pass::Land);
        prop_assert_eq!(workflow.land.is_some(), lands, "`land` is set, and a `pass` leads there");
    }

    #[test]
    fn a_definition_that_does_not_load_reports_a_fault(definition in definition()) {
        match team_of(&definition) {
            Ok(team) => prop_assert!(
                team.workflow(NAME).is_some(),
                "the definition did not load, and no fault says why"
            ),
            Err(faults) => prop_assert!(!faults.is_empty(), "refused with no fault"),
        }
    }

    #[test]
    fn no_list_of_verdicts_takes_an_issue_past_a_breaker_or_back_from_an_end(
        definition in definition(),
        verdicts in verdicts(),
    ) {
        let team = team_of(&definition);
        let Some(workflow) = loaded(&team) else { return Ok(()) };
        let stages = workflow.stages();
        // The rework counted here from what the issue was seen to do: each
        // time a stage ran again, since a human last ruled.
        let mut rework = 0;
        let mut in_a_row = vec![0; stages.len()];
        let mut place = Place::enter(workflow);
        for (nth, step) in verdicts.iter().enumerate() {
            let before = place.clone();
            let declared = step.declared(&before);
            place = place.after(declared);
            let said = format!("after verdict {nth}, `{}`", declared.word());
            match (before.position, declared) {
                (Position::Ended, _) => {
                    prop_assert_eq!(&place, &before, "{}, an issue left its end", said);
                }
                (Position::Landing | Position::LandingParked | Position::LandingEscalating, _) => {
                    prop_assert!(place.position.landing(), "{}, an issue left landing", said);
                    prop_assert!(place.position.stage().is_none(), "{}, a stage ran again", said);
                }
                (Position::Escalating(..), Declared::Ruling) => {
                    rework = 0;
                    in_a_row.fill(0);
                }
                (Position::Consult(_, _, ConsultOn::Escalate), Declared::Ruling)
                    if runs(place.position) =>
                {
                    rework += 1
                }
                (
                    Position::Perform(stage, _) | Position::Waiting(stage) | Position::Parked(stage),
                    Declared::Reject,
                ) if runs(place.position) => {
                    rework += 1;
                    in_a_row[stage.at()] += 1;
                }
                (
                    Position::Perform(stage, _) | Position::Waiting(stage) | Position::Parked(stage),
                    Declared::Pass,
                ) => in_a_row[stage.at()] = 0,
                _ => {}
            }
            prop_assert!(
                rework <= workflow.max_rework,
                "{}, the issue's rework is {} and `max_rework` is {}",
                said, rework, workflow.max_rework
            );
            prop_assert_eq!(place.rework(), rework, "{}, its rework is miscounted", said);
            for (stage, in_a_row) in stages.iter().zip(&in_a_row) {
                prop_assert!(
                    *in_a_row <= workflow.max_consecutive_rework,
                    "{}, `{}` rejected {} times in a row and `max_consecutive_rework` is {}",
                    said, stage.name, in_a_row, workflow.max_consecutive_rework
                );
                prop_assert_eq!(
                    place.consecutive_rework(stage), *in_a_row,
                    "{}, the rejections in a row of `{}` are miscounted", said, stage.name
                );
            }
        }
    }
}

/// The properties above say nothing of a definition that does not load, or of
/// a breaker no list of verdicts comes to: what is generated is both kinds of
/// definition, and lists that come to each breaker and each end.
#[test]
fn what_is_generated_reaches_what_the_properties_are_about() {
    let mut runner = TestRunner::deterministic();
    let (mut loads, mut refused, mut cycles, mut unreached) = (0, 0, 0, 0);
    let (mut landing, mut ended, mut total, mut consecutive) = (0, 0, 0, 0);
    for _ in 0..2000 {
        let definition = definition().new_tree(&mut runner).unwrap().current();
        let verdicts = verdicts().new_tree(&mut runner).unwrap().current();
        let team = team_of(&definition);
        let Some(workflow) = loaded(&team) else {
            refused += 1;
            let problems: Vec<String> =
                team.err().into_iter().flatten().map(|fault| fault.problem).collect();
            cycles += usize::from(problems.iter().any(|problem| problem.contains("for ever")));
            unreached += usize::from(problems.iter().any(|problem| problem.contains("no path")));
            continue;
        };
        loads += 1;
        let mut place = Place::enter(workflow);
        let mut tripped = vec![];
        for step in verdicts {
            let declared = step.declared(&place);
            place = place.after(declared);
            tripped.extend(place.position.breaker());
        }
        total += usize::from(tripped.contains(&Breaker::Rework));
        consecutive += usize::from(tripped.contains(&Breaker::ConsecutiveRework));
        landing += usize::from(place.position == Position::Landing);
        ended += usize::from(place.position == Position::Ended);
    }
    let seen = format!(
        "of 2000: {loads} load, {refused} do not ({cycles} for a cycle, {unreached} for a stage \
         nothing reaches); {landing} issues come to the landing train, {ended} to the end, \
         {total} to the breaker on rework in total, {consecutive} to the one on rejections in a row"
    );
    for count in [loads, refused] {
        assert!(count >= 400, "{seen}");
    }
    for count in [cycles, unreached, landing, ended, total, consecutive] {
        assert!(count >= 40, "{seen}");
    }
}
