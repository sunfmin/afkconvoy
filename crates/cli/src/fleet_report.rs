//! The fleet report: the facts of one fleet that `plan`, `doctor` and the
//! panel each show, read once from one look (docs/adr/0043) — intake, what
//! each role's range leaves out, the roster entries that match nothing, the
//! roles that can run nothing safely, the concurrency, the harness, the
//! tracker, the Jira deliveries, admission, and each queued issue's planned
//! launch. `read` gathers what the fleet view and the look already hold;
//! `of` makes the report of that, and is pure.

use afkconvoy_core::casting::{self, Held, UnmatchedModel, Why};
use afkconvoy_core::config::{Config, RepoName};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::plan::{Dispatchable, WorkingSet};
use afkconvoy_core::team::Team;
use anyhow::Result;
use serde_json::{Map, Value, json};

use crate::admission::{Admission, Readings};
use crate::fleet::Fleet;
use crate::git::Repo;
use crate::kit::Kit;
use crate::stage::Flows;

/// The facts of one fleet, as one look read them.
#[derive(Debug, Clone)]
pub struct FleetReport {
    /// A bound repo's issues taken in by workers that are open, and how many
    /// may be: `None` of an unbound repo.
    pub intake: Option<(usize, u64)>,
    /// What the team's roles make of the host kit: `None` of an unbound repo,
    /// or where the team or the host kit cannot be read.
    pub roles: Option<Roles>,
    /// The harness in force, as `named_harness::json_of` says it.
    pub harness: Value,
    /// The repo the fleet reads its issues in, where it is not its own: as
    /// `tracker::json_of` says it.
    pub tracker: Value,
    /// Each Jira delivery, as `delivery::reported` says it; why they cannot
    /// be read. `None` where the fleet could not be opened.
    pub deliveries: Option<Result<Vec<Value>, String>>,
    /// Admission's reading of the host from the look, and its shares; why it
    /// cannot be read.
    pub admission: Result<Admission, String>,
    /// Each queued issue whose harness starts a session, with that harness
    /// and the launch its worker is planned to start with.
    pub launches: Vec<(u64, Harness, Value)>,
}

/// What a bound repo's team makes of the host kit.
#[derive(Debug, Clone, PartialEq)]
pub struct Roles {
    /// What each role's range leaves out, and why, by role.
    pub left_out: Vec<(String, Value)>,
    /// A roster entry that matches nothing the host kit holds, by role
    /// (`casting::unmatched_entries`).
    pub unmatched: Vec<(String, String)>,
    /// A role that can run nothing safely, with why (`casting::range_faults`).
    pub nothing_safe: Vec<(String, String)>,
    pub unmatched_models: Vec<UnmatchedModel>,
}

/// What a fleet report is made of: what the fleet view, its rebuild and the
/// look read.
pub struct Seen<'a> {
    pub config: &'a Config,
    /// The tracker the fleet reads its issues in, where it is not its own.
    pub tracker: Option<&'a RepoName>,
    /// The harness the checkout names, where it names one.
    pub pinned: Option<Harness>,
    /// The team, where it loaded.
    pub team: Option<&'a Team>,
    /// The host kit as this host can start it now; why it cannot be read.
    pub kit: Result<&'a Kit, String>,
    /// The bound repo's issues taken in by workers that are open.
    pub taken_in: usize,
    pub frontier: &'a [Dispatchable],
    pub deliveries: Result<Vec<Value>, String>,
    pub admission: Result<Admission, String>,
    /// The launch a worker of `harness` would start with, on `model`, named
    /// by its issue where `explicit` (`codex_launch::planned`).
    pub planned: &'a dyn Fn(Harness, Option<&str>, bool) -> Value,
}

impl FleetReport {
    /// The report of the fleet `fleet`, after the rebuild that read `set`
    /// and `flows`, with admission read from the look's `readings`.
    pub fn read(
        fleet: &Fleet,
        flows: &Flows,
        set: &WorkingSet,
        readings: &Readings,
    ) -> FleetReport {
        // The account its next worker starts on, as its workers at work say it.
        let usage = crate::tick::usage_of(fleet, set, 0);
        let admission =
            Admission::read_from(&fleet.repo.dir, readings).map(|read| read.with_usage(usage));
        FleetReport::admitted(fleet, flows, set, admission.map_err(|e| format!("{e:#}")))
    }

    /// The same, with `admission` as the look already read it.
    pub fn admitted(
        fleet: &Fleet,
        flows: &Flows,
        set: &WorkingSet,
        admission: Result<Admission, String>,
    ) -> FleetReport {
        let kit = crate::kit::running().map(|(kit, _)| kit).map_err(|e| format!("{e:#}"));
        let prefix = fleet.repo.layout().under("delivery/");
        let records = fleet.records_at_open(&format!("{prefix}*"));
        let deliveries = crate::delivery::reported(&fleet.repo, fleet.now, &records);
        FleetReport::of(Seen {
            config: &fleet.config,
            tracker: fleet.gh.tracker.named(),
            pinned: fleet.pinned,
            team: flows.team().ok(),
            kit: kit.as_ref().map_err(String::clone),
            taken_in: flows.taken_in(),
            frontier: &set.frontier,
            deliveries: deliveries.map_err(|e| format!("{e:#}")),
            admission,
            planned: &crate::codex_launch::planned,
        })
    }

    /// The report of the checkout `repo` whose fleet could not be opened:
    /// what is read of the checkout alone, and admission from `readings`.
    pub fn unopened(repo: &Repo, readings: &Readings) -> FleetReport {
        let tracker = match crate::checkout_list::List::of_host() {
            Ok(list) => crate::tracker::json_of(&list, repo),
            Err(error) => json!({"error": format!("{error:#}")}),
        };
        FleetReport {
            intake: None,
            roles: None,
            harness: crate::named_harness::json_of(repo),
            tracker,
            deliveries: None,
            admission: Admission::read_from(&repo.dir, readings).map_err(|e| format!("{e:#}")),
            launches: vec![],
        }
    }

    /// The report of `seen`.
    pub fn of(seen: Seen) -> FleetReport {
        let bound = seen.config.bound.as_ref();
        let roles = match (bound, seen.team, &seen.kit) {
            (Some(_), Some(team), Ok(kit)) => Some(Roles::of(team, &kit.range())),
            _ => None,
        };
        let harness = match (seen.pinned, &seen.kit) {
            (Some(harness), _) => json!({"in_force": harness.key(), "said_by": "checkout"}),
            (None, Ok(kit)) => {
                json!({"in_force": kit.first().map(|held| held.harness.key()), "said_by": "host_kit"})
            }
            (None, Err(error)) => json!({"error": error}),
        };
        let launches = seen
            .frontier
            .iter()
            .filter_map(|issue| {
                let cast = issue.cast.as_ref()?;
                cast.harness.session()?;
                let explicit = cast.why == Why::ModelLabel;
                let planned = (seen.planned)(cast.harness, cast.model.as_deref(), explicit);
                Some((issue.number, cast.harness, planned))
            })
            .collect();
        FleetReport {
            intake: bound.map(|bound| (seen.taken_in, bound.intake_limit)),
            roles,
            harness,
            tracker: json!(seen.tracker),
            deliveries: Some(seen.deliveries),
            admission: seen.admission,
            launches,
        }
    }

    /// The tracker, where it is not the checkout's own repo.
    pub fn tracker_named(&self) -> Option<String> {
        self.tracker.as_str().map(str::to_string)
    }

    /// The roles that can run nothing safely: none where none was read.
    pub fn nothing_safe(&self) -> &[(String, String)] {
        self.roles.as_ref().map_or(&[], |roles| &roles.nothing_safe)
    }

    /// Admission's thresholds, reading and holds as `plan` and `doctor` say
    /// them; {error} where it cannot be read.
    pub fn admission_json(&self) -> Value {
        match &self.admission {
            Ok(admission) => admission.json(),
            Err(error) => json!({"error": error}),
        }
    }

    /// The deliveries as `doctor` says them: an unreadable one is a row of
    /// its error.
    pub fn deliveries_json(&self) -> Vec<Value> {
        match &self.deliveries {
            Some(Ok(rows)) => rows.clone(),
            Some(Err(error)) => vec![json!({"error": error})],
            None => vec![],
        }
    }

    /// What `plan` answers of the report, by its key: an error where
    /// admission or the deliveries cannot be read, which refuses the plan.
    pub fn plan_json(&self) -> Result<Map<String, Value>> {
        let mut plan = Map::new();
        if let Some((open, limit)) = self.intake {
            plan.insert("intake".into(), json!({"open": open, "limit": limit}));
        }
        if let Some(roles) = &self.roles {
            let left_out = roles.left_out.iter();
            let left_out = left_out.map(|(role, left)| json!({"role": role, "left_out": left}));
            plan.insert("left_out".into(), Value::Array(left_out.collect()));
            // Said, never refusing the plan.
            let named = |each: &[(String, String)]| -> Vec<Value> {
                each.iter().map(|(role, why)| json!({"role": role, "why": why})).collect()
            };
            if !roles.unmatched.is_empty() {
                plan.insert("unmatched".into(), Value::Array(named(&roles.unmatched)));
            }
            if !roles.nothing_safe.is_empty() {
                plan.insert("nothing_safe".into(), Value::Array(named(&roles.nothing_safe)));
            }
            plan.insert("unmatched_models".into(), serde_json::to_value(&roles.unmatched_models)?);
        }
        plan.insert("harness".into(), self.harness.clone());
        plan.insert("tracker".into(), self.tracker.clone());
        let deliveries = self.deliveries.clone().unwrap_or(Ok(vec![]));
        plan.insert("deliveries".into(), json!(deliveries.map_err(anyhow::Error::msg)?));
        let admission = self.admission.as_ref().map_err(|e| anyhow::anyhow!("{e}"))?;
        plan.insert("admission".into(), admission.json());
        Ok(plan)
    }

    /// Puts each planned launch on its issue of `frontier`, as `plan` lists
    /// it, under its harness's key.
    pub fn put_launches(&self, frontier: &mut Value) {
        for issue in frontier.as_array_mut().into_iter().flatten() {
            let number = issue["number"].as_u64();
            let planned = self.launches.iter().find(|(at, ..)| Some(*at) == number);
            if let Some((_, harness, launch)) = planned {
                issue[harness.key()] = launch.clone();
            }
        }
    }
}

impl Roles {
    /// What `team`'s roles make of the host kit's range `kit`.
    fn of(team: &Team, kit: &[Held]) -> Roles {
        let left_out = team.roles.iter();
        let left_out =
            left_out.map(|role| (role.name.clone(), crate::team::left_out_json(role, kit)));
        Roles {
            left_out: left_out.collect(),
            unmatched: casting::unmatched_entries(team, kit),
            nothing_safe: casting::range_faults(team, kit),
            unmatched_models: casting::unmatched_models(team, kit),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use afkconvoy_core::admission::{Disk, Reading, SHARES};
    use afkconvoy_core::casting::Cast;

    fn config(bound: &str) -> Config {
        let yaml = format!("```yaml\nbase_branch: main\ngate_command: make\n{bound}```\n");
        crate::config::parse(&yaml).unwrap()
    }

    fn kit(harnesses: &[&str]) -> Kit {
        let held: Vec<Value> =
            harnesses.iter().map(|key| json!({"harness": key, "command": key})).collect();
        serde_json::from_value(json!({"harnesses": held})).unwrap()
    }

    fn admission(load: u64) -> Admission {
        let reading = Reading {
            cores: 4,
            load: Some(load),
            memory_free: Some(50),
            disk: Some(Disk { free: 1 << 40, size: 1 << 41 }),
        };
        Admission {
            shares: SHARES,
            said: vec![],
            reading,
            total_load: reading.load,
            queue_load: Some(0),
            span: 600,
            queue_drain: None,
            queue_bound: afkconvoy_core::admission::QUEUE_BOUND,
            usage: None,
            cap: Default::default(),
            own: None,
            memory: None,
        }
    }

    fn queued(number: u64, cast: Option<Cast>) -> Dispatchable {
        Dispatchable { number, title: format!("#{number}"), stage: None, cast }
    }

    fn cast(harness: Harness, model: Option<&str>, why: Why) -> Cast {
        let model = model.map(str::to_string);
        Cast { harness, model, why, unmatched: None, notice: None }
    }

    fn planned(harness: Harness, model: Option<&str>, explicit: bool) -> Value {
        json!({"harness": harness.key(), "model": model, "explicit": explicit})
    }

    /// A fleet of an unbound repo whose checkout names no harness, reading
    /// its issues in `tracker`.
    fn seen<'a>(
        config: &'a Config,
        kit: Result<&'a Kit, String>,
        tracker: Option<&'a RepoName>,
        frontier: &'a [Dispatchable],
    ) -> Seen<'a> {
        Seen {
            config,
            tracker,
            pinned: None,
            team: None,
            kit,
            taken_in: 0,
            frontier,
            deliveries: Ok(vec![json!({"delivery": "d1", "detail": "not ready"})]),
            admission: Ok(admission(100)),
            planned: &planned,
        }
    }

    #[test]
    fn an_unbound_fleet_reports_its_concurrency_tracker_harness_admission_and_deliveries() {
        let (config, kit) = (config(""), kit(&["codex", "claude-code"]));
        let tracker = RepoName::read("acme/issues").unwrap();
        let report = FleetReport::of(seen(&config, Ok(&kit), Some(&tracker), &[]));
        assert_eq!(report.intake, None, "an unbound repo takes in nothing");
        assert_eq!(report.roles, None);
        assert_eq!(report.harness, json!({"in_force": "codex", "said_by": "host_kit"}));
        assert_eq!(report.tracker_named().as_deref(), Some("acme/issues"));
        assert_eq!(report.admission_json()["admits"], true);
        let plan = report.plan_json().unwrap();
        let keys: Vec<&str> = plan.keys().map(String::as_str).collect();
        assert_eq!(keys, ["admission", "deliveries", "harness", "tracker"]);
        assert_eq!(plan["deliveries"][0]["detail"], "not ready");
        assert_eq!(report.deliveries_json(), plan["deliveries"].as_array().unwrap().clone());
    }

    #[test]
    fn the_checkouts_own_harness_holds_over_the_host_kit() {
        let config = config("");
        let mut seen = seen(&config, Err("no host kit".into()), None, &[]);
        seen.pinned = Some(Harness::ClaudeCode);
        let report = FleetReport::of(seen);
        assert_eq!(report.harness, json!({"in_force": "claude-code", "said_by": "checkout"}));
        assert_eq!(report.tracker, Value::Null, "the checkout's own repo is its tracker");
        let config = self::config("");
        let report = FleetReport::of(self::seen(&config, Err("no host kit".into()), None, &[]));
        assert_eq!(report.harness, json!({"error": "no host kit"}));
    }

    #[test]
    fn a_bound_fleet_reports_its_intake_and_what_its_roles_make_of_the_host_kit() {
        let config = config("team: acme/team\nintake_limit: 4\n");
        let kit = kit(&["claude-code"]);
        let team = Team {
            escalate_label: "ready-for-human".into(),
            rules: false,
            host_user: true,
            dsh_profile: Default::default(),
            split: true,
            roles: vec![],
            workflows: vec![],
            patrols: vec![],
        };
        let mut seen = seen(&config, Ok(&kit), None, &[]);
        seen.team = Some(&team);
        seen.taken_in = 2;
        let report = FleetReport::of(seen);
        assert_eq!(report.intake, Some((2, 4)));
        let roles = report.roles.as_ref().expect("the team and the host kit were read");
        assert_eq!((roles.left_out.len(), roles.nothing_safe.len()), (0, 0));
        let plan = report.plan_json().unwrap();
        assert_eq!(plan["intake"], json!({"open": 2, "limit": 4}));
        assert_eq!(plan["left_out"], json!([]));
        assert_eq!(plan["unmatched_models"], json!([]));
        assert!(!plan.contains_key("unmatched") && !plan.contains_key("nothing_safe"));
        // A host kit that cannot be read names no role, and refuses nothing.
        let mut seen = self::seen(&config, Err("unread".into()), None, &[]);
        seen.team = Some(&team);
        let report = FleetReport::of(seen);
        assert_eq!(report.roles, None);
        assert_eq!(report.nothing_safe(), []);
        assert!(report.plan_json().unwrap().contains_key("intake"));
    }

    #[test]
    fn a_queued_issue_whose_harness_starts_a_session_is_given_its_planned_launch() {
        let (config, kit) = (config(""), kit(&["codex"]));
        let frontier = [
            queued(7, Some(cast(Harness::Codex, Some("o3"), Why::ModelLabel))),
            queued(8, Some(cast(Harness::Codex, None, Why::UserDefault))),
            queued(9, None),
        ];
        let report = FleetReport::of(seen(&config, Ok(&kit), None, &frontier));
        let numbers: Vec<u64> = report.launches.iter().map(|(at, ..)| *at).collect();
        assert_eq!(numbers, [7, 8]);
        let mut listed = json!([{"number": 7}, {"number": 8}, {"number": 9}]);
        report.put_launches(&mut listed);
        assert_eq!(listed[0]["codex"], planned(Harness::Codex, Some("o3"), true));
        assert_eq!(listed[1]["codex"], planned(Harness::Codex, None, false));
        assert_eq!(listed[2], json!({"number": 9}));
    }

    #[test]
    fn a_plan_is_refused_where_admission_or_the_deliveries_cannot_be_read() {
        let config = config("");
        let mut seen = seen(&config, Err("unread".into()), None, &[]);
        seen.admission = Err("sysctl: <none>".into());
        let report = FleetReport::of(seen);
        assert_eq!(report.admission_json(), json!({"error": "sysctl: <none>"}));
        assert!(report.plan_json().unwrap_err().to_string().contains("sysctl"));
        let mut seen = self::seen(&config, Err("unread".into()), None, &[]);
        seen.deliveries = Err("no remote".into());
        let report = FleetReport::of(seen);
        assert_eq!(report.deliveries_json(), [json!({"error": "no remote"})]);
        assert!(report.plan_json().is_err());
    }
}
