//! What a command that changes fleet state says of itself, whoever runs it: a
//! line in the tick log of this host for this repo as it acts — who did what,
//! on which issue, and how it came out — and then a wake of that repo's fleet
//! instance, through the host runner, naming the command, which the fleet
//! instance logs as `woken by <command> #N`.
//!
//! Both are soft: a command acts the same whether either can be done, and
//! with no fleet instance to wake its line is in the log all the same.

use afkconvoy_core::patrol::PATROL_ENV;
use afkconvoy_core::precedent::PROPOSAL_ENV;
use afkconvoy_core::role::{ISSUE_ENV, ROLE_ENV};
use serde_json::{Value, json};

use crate::git::Repo;
use crate::log::{Entry, Log, What};
use crate::wake;

/// What the manual adds to the text of each command that acts: what it logs
/// and whom it wakes, said whole once, in the topic it links to.
pub const HELP: &str = "## What it logs\n\nAs it acts it appends one line to the tick log \
     of this host for this repo, then wakes this host's fleet instance of this repo, through the \
     host runner; its answer says whether it woke one, as \"woke\": [`afkconvoy help \
     tick-log`](help:tick-log).";

/// One command that acts, as it starts.
pub struct Acting {
    /// The command's name, as its command line says it.
    pub command: &'static str,
    /// What it acts on: `#46`, `run sweep`, `from <instance>`.
    pub on: Option<String>,
    /// The fleet instance it acts as, where it names one.
    pub instance: Option<String>,
    /// Whether it says the start of what it does, as well as its end: one
    /// that runs as long as the gate does.
    pub says_start: bool,
    /// The checkout it acts in: whose tick log it writes to, and whose `run`
    /// it wakes. `None`: it was run in no checkout, and does neither.
    pub checkout: Option<Repo>,
}

impl Acting {
    pub fn new(command: &'static str) -> Acting {
        Acting { command, on: None, instance: None, says_start: false, checkout: None }
    }

    pub fn in_checkout(self, checkout: Option<Repo>) -> Acting {
        Acting { checkout, ..self }
    }

    pub fn on_issue(self, issue: Option<u64>) -> Acting {
        Acting { on: issue.map(|issue| format!("#{issue}")), ..self }
    }

    pub fn on(self, on: String) -> Acting {
        Acting { on: Some(on), ..self }
    }

    pub fn as_instance(self, instance: &str) -> Acting {
        Acting { instance: Some(instance.to_string()), ..self }
    }

    pub fn saying_start(self) -> Acting {
        Acting { says_start: true, ..self }
    }

    /// The issue a worker is on, from its environment: what a command that
    /// names none acts for.
    pub fn on_own_issue(self) -> Acting {
        let issue = std::env::var(ISSUE_ENV).ok().and_then(|issue| issue.parse().ok());
        self.on_issue(issue)
    }

    /// The command and what it acts on, as the wake names them: `park #46`.
    pub fn named(&self) -> String {
        match &self.on {
            Some(on) => format!("{} {on}", self.command),
            None => self.command.to_string(),
        }
    }

    /// Says in the log that it is starting, if it says that.
    pub fn start(&self, now: u64) {
        if self.says_start {
            self.log(now, "started".to_string(), false);
        }
    }

    /// Says `what` in the log, as one step of what it does: what a command
    /// that runs as long as `exclusive` does says as it goes.
    pub fn say(&self, now: u64, what: String) {
        self.log(now, what, false);
    }

    /// Says in the log how it ended, then wakes the `run` → `ended`, an
    /// object's with whether the wake was sent, as `woke`. What `wake`
    /// answers is the wake's whole answer.
    pub fn end(&self, now: u64, ended: anyhow::Result<Value>) -> anyhow::Result<Value> {
        match &ended {
            Ok(answer) if self.command == "switch" => self.log_switch(now, answer),
            Ok(answer) => self.log(now, outcome(self.command, answer), false),
            Err(error) => self.log(now, format!("{error:#}"), true),
        }
        let woke = self.in_one().and_then(|repo| wake::wake(repo, &self.named()));
        let woke =
            woke.unwrap_or_else(|error| json!({"woke": false, "detail": format!("{error:#}")}));
        let mut answer = ended?;
        if self.command == "wake" {
            return Ok(woke);
        }
        if let Some(answer) = answer.as_object_mut() {
            answer.insert("woke".to_string(), woke["woke"].clone());
        }
        Ok(answer)
    }

    /// A switch that crosses checkouts leaves each moved worker's explanation
    /// in that worker's own tick log, not only in the invoking checkout's.
    fn log_switch(&self, now: u64, answer: &Value) {
        let moved = answer["switched"].as_array().cloned().unwrap_or_default();
        if moved.is_empty() {
            self.log(now, switch_outcome(answer), false);
            return;
        }
        for one in moved {
            let checkout = one["checkout"]
                .as_str()
                .map(|path| Repo::new(path.into()))
                .or_else(|| self.checkout.clone());
            let mut line = Acting::new("switch").in_checkout(checkout);
            line.instance = self.instance.clone();
            line.on = one["issue"]
                .as_u64()
                .map(|issue| format!("#{issue}"))
                .or_else(|| one["patrol"].as_str().map(|name| format!("patrol {name}")))
                .or_else(|| self.on.clone());
            line.log(now, switch_outcome(&json!({"switched": [one]})), false);
        }
    }

    fn log(&self, now: u64, outcome: String, failed: bool) {
        let outcome = if self.command == "switch" {
            outcome.split_whitespace().collect::<Vec<_>>().join(" ")
        } else {
            one_line(&outcome)
        };
        let what = What::Command {
            command: self.command.to_string(),
            on: self.on.clone(),
            by: by(self.command).to_string(),
            outcome,
            failed,
        };
        let instance = self.instance.clone().unwrap_or_default();
        let entry = Entry { at: now, instance, host: crate::this_host(), what };
        // Soft: a command that cannot log acts all the same.
        if let Err(error) = self.in_one().and_then(|repo| Log::of(repo)?.append(&entry)) {
            eprintln!("warning: this command left no line in the tick log: {error:#}");
        }
    }

    /// The checkout it acts in; an error where it was run in none.
    fn in_one(&self) -> anyhow::Result<&Repo> {
        self.checkout.as_ref().ok_or_else(|| anyhow::anyhow!("it was run in no checkout"))
    }
}

/// The commands only a worker runs, including the train's, which carries
/// no issue in its environment.
const WORKERS_OWN: [&str; 4] = ["gate", "land", "over", "wake"];

/// Who ran `command`: a worker — started in a role, on an issue, a patrol or
/// a proposal, or running a command only a worker runs — or the operator.
fn by(command: &str) -> &'static str {
    let started = [ROLE_ENV, ISSUE_ENV, PATROL_ENV, PROPOSAL_ENV];
    let in_worker = started.iter().any(|name| std::env::var_os(name).is_some());
    if in_worker || WORKERS_OWN.contains(&command) { "worker" } else { "operator" }
}

/// `text` on one line, and not past a line's length.
fn one_line(text: &str) -> String {
    const LONGEST: usize = 300;
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    match line.char_indices().nth(LONGEST) {
        Some((cut, _)) => format!("{}…", &line[..cut]),
        None => line,
    }
}

/// Every worker a switch moved, with every candidate skipped before its
/// successor and what would make that candidate eligible.
fn switch_outcome(answer: &Value) -> String {
    let mut moved = vec![];
    for one in answer["switched"].as_array().into_iter().flatten() {
        let worker = one["issue"]
            .as_u64()
            .map(|issue| format!("#{issue}"))
            .or_else(|| one["patrol"].as_str().map(|name| format!("patrol {name}")))
            .unwrap_or_else(|| one["worktree"].as_str().unwrap_or("worker").to_string());
        let mut said = format!(
            "{worker}: {} to {}",
            one["from"].as_str().unwrap_or_default(),
            one["to"].as_str().unwrap_or_default()
        );
        for skipped in one["skipped"].as_array().into_iter().flatten() {
            said.push_str(&format!(
                "; skipped {}: {} (to make it eligible: {})",
                skipped["harness"].as_str().unwrap_or_default(),
                skipped["why"].as_str().unwrap_or_default(),
                skipped["fix"].as_str().unwrap_or_default()
            ));
        }
        moved.push(said);
    }
    moved.join("; ")
}

/// How a command that acted came out, in a few words, from what it answers.
pub fn outcome(command: &str, answer: &Value) -> String {
    let word = |key: &str| answer[key].as_str().map(str::to_string);
    let issues = |key: &str| {
        let numbers = answer[key].as_array().into_iter().flatten().filter_map(Value::as_u64);
        numbers.map(|number| format!("#{number}")).collect::<Vec<_>>().join(", ")
    };
    let said = match command {
        "switch" => Some(switch_outcome(answer)),
        "park" => Some(format!("blocked by {}", issues("blocked_by"))),
        "escalate" | "close" | "nudge" | "restart" | "abandon" => word("action"),
        "dispatch" => Some(match answer["started"].as_bool() {
            Some(true) => "started".to_string(),
            _ => match (word("gone"), word("skipped"), word("claim")) {
                (Some(gone), ..) => format!("not started: gone, {gone}"),
                (_, Some(skipped), _) => format!("not started: {skipped}"),
                (_, _, Some(claim)) => format!("not started: claim {claim}"),
                _ => "not started".to_string(),
            },
        }),
        "takeover" => Some(format!(
            "{}: took [{}], lost [{}]",
            word("action").unwrap_or_default(),
            issues("taken"),
            issues("lost")
        )),
        "claim" => Some(match answer["won"].as_bool() {
            Some(true) => "won".to_string(),
            _ => format!("lost to {}", answer["owner"]),
        }),
        "heartbeat" => Some(match answer["refreshed"].as_bool() {
            Some(true) => "refreshed".to_string(),
            _ => format!("not refreshed: {}", word("reason").unwrap_or_default()),
        }),
        "verdict" => word("verdict").map(|verdict| match answer["declared"].as_bool() {
            Some(false) => format!("{verdict}: not declared, {} stands", answer["standing"]),
            _ => verdict,
        }),
        "rule" => Some(format!("ruled, at {}", word("stage").unwrap_or_default())),
        "send" => Some(format!("sent to {}", word("stage").unwrap_or_default())),
        "intake" => Some(match answer["taken"].as_bool() {
            Some(true) => format!("taken in to {}", word("workflow").unwrap_or_default()),
            _ => "not taken in".to_string(),
        }),
        "lock" => Some(match answer["raised"].as_bool() {
            Some(true) => "raised".to_string(),
            _ => "a lock already".to_string(),
        }),
        "land" => word("outcome"),
        "gate" => word("status"),
        "tick" => {
            let did = answer["did"].as_array().map_or(0, Vec::len);
            Some(format!(
                "{did} step(s) · holding {} · frontier {}",
                answer["held"], answer["frontier_remaining"]
            ))
        }
        _ => None,
    };
    said.filter(|said| !said.is_empty()).unwrap_or_else(|| "done".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_comes_out_in_a_few_words() {
        let park = json!({"issue": 46, "action": "parked", "blocked_by": [99, 100]});
        assert_eq!(outcome("park", &park), "blocked by #99, #100");
        assert_eq!(outcome("escalate", &json!({"action": "escalated"})), "escalated");
        assert_eq!(
            outcome("verdict", &json!({"verdict": "escalate", "declared": true})),
            "escalate"
        );
        let standing = json!({"verdict": "pass", "declared": false, "standing": "reject"});
        assert_eq!(outcome("verdict", &standing), "pass: not declared, \"reject\" stands");
        let took = json!({"action": "taken", "taken": [4, 7], "lost": []});
        assert_eq!(outcome("takeover", &took), "taken: took [#4, #7], lost []");
        assert_eq!(outcome("land", &json!({"outcome": "joined"})), "joined");
        assert_eq!(outcome("gate", &json!({"status": "green"})), "green");
        assert_eq!(outcome("release", &json!({"released": true})), "done");
    }

    #[test]
    fn what_a_command_acts_on_is_in_its_wake() {
        assert_eq!(Acting::new("park").on_issue(Some(46)).named(), "park #46");
        assert_eq!(Acting::new("over").on("run sweep".into()).named(), "over run sweep");
        assert_eq!(Acting::new("abandon").named(), "abandon");
    }

    #[test]
    fn a_line_is_one_line_and_not_a_page() {
        assert_eq!(one_line("cannot\n  push:\trejected"), "cannot push: rejected");
        let long = "é".repeat(400);
        assert_eq!(one_line(&long).chars().count(), 301);
    }
}
