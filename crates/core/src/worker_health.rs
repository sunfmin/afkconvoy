//! Worker health after the rules: Jev judges the latest screen, and this
//! table turns its typed answers into the class, scope and action the fleet
//! carries out. The key command, HTTP call and worker transitions are effects.

use serde_json::{Map, Value, json};

pub const USAGE_LIMIT: &str = "usage_limit";
pub const BUSY: &str = "busy";
pub const FINISHED: &str = "finished";
pub const WAITING_FOR_PERSON: &str = "waiting_for_person";
pub const TRUST_PROMPT: &str = "trust_prompt";
pub const LOGIN_REFUSED: &str = "login_refused";
pub const EVIDENCE: &str = "evidence";

pub const NOULS: [&str; 6] =
    [USAGE_LIMIT, BUSY, FINISHED, WAITING_FOR_PERSON, TRUST_PROMPT, LOGIN_REFUSED];

const YES: u64 = 700;
const NO: u64 = 300;

/// The one System One request for a health check. Every evidence option is a
/// line actually on the screen, so Jev selects evidence rather than writing it.
pub fn request(screen: &[String]) -> Value {
    let noul = |instructions: &str, yes: &str, no: &str| {
        json!({"type": "noul", "instructions": instructions,
            "criteria": {"true": yes, "false": no}})
    };
    let mut questions = Map::new();
    questions.insert(
        USAGE_LIMIT.into(),
        noul(
            "Has the agent stopped because its account's usage limit is used up?",
            "The account cannot do more work until its usage limit resets or is increased.",
            "The agent can still do work, including when a warning says its limit is near.",
        ),
    );
    questions.insert(
        BUSY.into(),
        noul(
            "Is the agent busy right now?",
            "The agent is actively working or waiting for a command it started.",
            "The agent is idle, stopped, finished, or waiting for a person.",
        ),
    );
    questions.insert(
        FINISHED.into(),
        noul(
            "Has the agent finished its task and reported the result?",
            "The agent completed its task and reported the outcome.",
            "The task is still underway, stopped short, or has no reported outcome.",
        ),
    );
    questions.insert(
        WAITING_FOR_PERSON.into(),
        noul(
            "Is the screen waiting for a person to choose?",
            "The agent cannot continue until a person answers or selects an option.",
            "The agent needs no choice from a person to continue.",
        ),
    );
    questions.insert(
        TRUST_PROMPT.into(),
        noul(
            "Is the agent asking whether to trust the folder?",
            "The screen asks whether to trust the folder or workspace.",
            "The screen has no folder or workspace trust question.",
        ),
    );
    questions.insert(
        LOGIN_REFUSED.into(),
        noul(
            "Has the agent stopped because its login or key was refused?",
            "Authentication failed, a login expired, or an API key was refused.",
            "There is no authentication or API-key refusal.",
        ),
    );
    let criteria: Map<String, Value> = screen
        .iter()
        .map(|line| (line.clone(), json!("This exact line is the reason the agent stopped.")))
        .collect();
    questions.insert(
        EVIDENCE.into(),
        json!({
            "type": "choice",
            "instructions": "Which line of the screen says why the agent stopped?",
            "criteria": criteria,
        }),
    );
    json!({"model": "jev-latest", "state": screen.join("\n"), "questions": questions})
}

/// Jev's six Noul answers as probabilities in thousandths, and its selected
/// evidence line. The effect layer refuses missing or out-of-range answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answers {
    pub usage_limit: u64,
    pub busy: u64,
    pub finished: u64,
    pub waiting_for_person: u64,
    pub trust_prompt: u64,
    pub login_refused: u64,
    pub evidence: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Account,
    Worker,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Wait,
    Continue,
    Complete,
}

/// One settled row of the health table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub class: &'static str,
    pub scope: Scope,
    pub action: Action,
    pub evidence: String,
}

/// Maps settled judgments to the existing health actions. Any Noul between
/// 0.3 and 0.7 leaves the whole check unsettled; an all-negative screen has
/// no class the fleet can safely act on. Explicit faults take priority over
/// activity, a person waiting over completion, and completion over activity.
pub fn classify(answers: &Answers) -> Result<Decision, String> {
    let probabilities = [
        answers.usage_limit,
        answers.busy,
        answers.finished,
        answers.waiting_for_person,
        answers.trust_prompt,
        answers.login_refused,
    ];
    if probabilities.iter().any(|probability| *probability > NO && *probability < YES) {
        return Err("Jev left the worker health check unsettled".into());
    }
    let decision = |class, scope, action| Decision {
        class,
        scope,
        action,
        evidence: answers.evidence.clone(),
    };
    if answers.usage_limit >= YES {
        return Ok(decision("quota", Scope::Account, Action::Continue));
    }
    if answers.login_refused >= YES {
        return Ok(decision("authentication", Scope::Account, Action::Continue));
    }
    if answers.trust_prompt >= YES || answers.waiting_for_person >= YES {
        return Ok(decision("waiting_human", Scope::Worker, Action::Wait));
    }
    if answers.finished >= YES {
        return Ok(decision("complete", Scope::Worker, Action::Complete));
    }
    if answers.busy >= YES {
        return Ok(decision("working", Scope::Worker, Action::Wait));
    }
    Err("Jev found no worker health class the fleet can act on".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recorded(
        usage_limit: u64,
        busy: u64,
        finished: u64,
        waiting_for_person: u64,
        trust_prompt: u64,
        login_refused: u64,
        evidence: &str,
    ) -> Answers {
        Answers {
            usage_limit,
            busy,
            finished,
            waiting_for_person,
            trust_prompt,
            login_refused,
            evidence: evidence.into(),
        }
    }

    #[test]
    fn recorded_real_screen_answers_take_the_right_rows() {
        let cases = [
            (
                "usage limit",
                recorded(940, 20, 30, 40, 10, 20, "■ Usage limit reached."),
                ("quota", Scope::Account, Action::Continue),
            ),
            (
                "near-limit warning",
                recorded(150, 960, 100, 120, 10, 10, "weekly limit: only 1% left"),
                ("working", Scope::Worker, Action::Wait),
            ),
            (
                "trust prompt",
                recorded(20, 40, 30, 950, 950, 10, "Trust this folder?"),
                ("waiting_human", Scope::Worker, Action::Wait),
            ),
            (
                "working screen",
                recorded(30, 980, 200, 200, 10, 10, "Running cargo nextest"),
                ("working", Scope::Worker, Action::Wait),
            ),
            (
                "finished screen",
                recorded(20, 50, 970, 100, 10, 10, "Opened the PR and joined the landing train."),
                ("complete", Scope::Worker, Action::Complete),
            ),
        ];
        for (screen, answers, expected) in cases {
            let decided = classify(&answers).unwrap_or_else(|error| panic!("{screen}: {error}"));
            assert_eq!((decided.class, decided.scope, decided.action), expected, "{screen}");
            assert_eq!(decided.evidence, answers.evidence, "{screen}");
        }
    }

    #[test]
    fn an_unsettled_or_all_negative_answer_goes_to_the_operator() {
        let unsettled = recorded(20, 950, 500, 100, 10, 10, "Working");
        assert!(classify(&unsettled).unwrap_err().contains("unsettled"));
        let unknown = recorded(20, 50, 100, 100, 10, 10, "Shall I continue?");
        assert!(classify(&unknown).unwrap_err().contains("no worker health class"));
        let at_bounds = recorded(300, 700, 300, 300, 300, 300, "Working");
        assert_eq!(classify(&at_bounds).unwrap().class, "working");
    }

    #[test]
    fn the_request_asks_all_judgments_together_and_selects_screen_evidence() {
        let screen = vec!["first line".to_string(), "Usage limit reached.".to_string()];
        let request = request(&screen);
        assert_eq!(request["model"], "jev-latest");
        assert_eq!(request["state"], "first line\nUsage limit reached.");
        for id in NOULS {
            assert_eq!(request["questions"][id]["type"], "noul", "{id}");
        }
        assert_eq!(request["questions"][EVIDENCE]["type"], "choice");
        assert!(request["questions"][EVIDENCE]["criteria"].get("Usage limit reached.").is_some());
        assert!(!request.to_string().contains("Authorization"));
    }
}
