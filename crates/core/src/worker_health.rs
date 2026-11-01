//! Worker health: the one decision for a stopped worker. Whatever the
//! evidence — the harness's own words, a structured error code, or Jev's
//! answers on its latest screen — it is turned into a class, and the class
//! alone gives the scope and the action the fleet carries out. Every code,
//! question, criterion and threshold is here, and nowhere else. The key
//! command, the call to TypeSafe and the worker's transitions are the effect
//! layer's.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::harness::Harness;

/// The System One model asked.
pub const MODEL: &str = "jev-latest";

/// The question that selects the evidence: the one Choice of a request.
pub const EVIDENCE: &str = "evidence";

/// A Noul at or above it reads as yes, at or below `NO` as no, and between
/// them the question is unsettled: thousandths of a probability.
pub const YES: u64 = 700;
pub const NO: u64 = 300;

/// The most options TypeSafe takes for one Choice.
const MOST_OPTIONS: usize = 255;

/// Where the fault's evidence leaves the worker: an account and its
/// harness, one model of it, or this worker alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    Account,
    Model,
    Worker,
}

impl Scope {
    /// The word it is written as.
    pub fn key(self) -> &'static str {
        match self {
            Scope::Account => "account",
            Scope::Model => "model",
            Scope::Worker => "worker",
        }
    }
}

/// What the fleet does with the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Look again at the next check, within the hour that bounds a wait.
    Wait,
    /// Tell it to carry on, a bounded number of times.
    Retry,
    /// Continue it on the next harness of the kit, its work kept.
    Continue,
    /// What an older book recorded where a harness's diagnosis knew nothing:
    /// read as a wait. No decision gives it.
    Unknown,
    /// Nothing: it is done.
    Complete,
    /// Continue the same worker through its existing nudge.
    Nudge,
}

/// What a stopped worker is: each class has one scope and one action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Its account's usage limit is used up.
    Quota,
    /// Its login or key was refused.
    Authentication,
    /// Its account, organization or service is disabled.
    Disabled,
    /// Its model is not found or not available.
    Model,
    /// It was rate limited.
    RateLimit,
    /// Its session broke.
    Session,
    /// Its harness refuses this issue: a task refusal.
    Refusal,
    /// Its selected model is at capacity: a capacity stop, its own.
    Capacity,
    /// It asks whether to trust its folder.
    TrustGap,
    /// It waits for a person to choose.
    WaitingHuman,
    /// It is busy.
    Working,
    /// It finished and reported.
    Complete,
    /// Its turn ended with the task still incomplete and no question.
    Short,
    /// Its session exited with a failure.
    Failed,
}

impl Class {
    /// The word a book, a log line and `doctor` say the class by.
    pub fn key(self) -> &'static str {
        match self {
            Class::Quota => "quota",
            Class::Authentication => "authentication",
            Class::Disabled => "disabled",
            Class::Model => "model",
            Class::RateLimit => "rate_limit",
            Class::Session => "session",
            Class::Refusal => "refusal",
            Class::Capacity => "capacity",
            Class::TrustGap => "trust_gap",
            Class::WaitingHuman => "waiting_human",
            Class::Working => "working",
            Class::Complete => "complete",
            Class::Short => "short",
            Class::Failed => "failed",
        }
    }

    /// The table: the scope and the action of each class.
    pub fn row(self) -> (Scope, Action) {
        match self {
            Class::Quota | Class::Authentication | Class::Disabled => {
                (Scope::Account, Action::Continue)
            }
            Class::Model => (Scope::Model, Action::Continue),
            Class::RateLimit | Class::Capacity => (Scope::Worker, Action::Retry),
            Class::Session | Class::Refusal | Class::Failed => (Scope::Worker, Action::Continue),
            Class::TrustGap | Class::WaitingHuman | Class::Working => (Scope::Worker, Action::Wait),
            Class::Complete => (Scope::Worker, Action::Complete),
            Class::Short => (Scope::Worker, Action::Nudge),
        }
    }
}

/// The structured error codes the fleet recognizes, and the class of each.
const CODES: [(&str, Class); 16] = [
    ("insufficient_quota", Class::Quota),
    ("usage_limit_reached", Class::Quota),
    ("quota_exceeded", Class::Quota),
    ("authentication_error", Class::Authentication),
    ("invalid_api_key", Class::Authentication),
    ("login_required", Class::Authentication),
    ("token_expired", Class::Authentication),
    ("organization_disabled", Class::Disabled),
    ("account_disabled", Class::Disabled),
    ("service_disabled", Class::Disabled),
    ("model_not_found", Class::Model),
    ("model_unavailable", Class::Model),
    ("rate_limit_exceeded", Class::RateLimit),
    ("too_many_requests", Class::RateLimit),
    ("session_error", Class::Session),
    ("session_corrupted", Class::Session),
];

/// One judgment Jev is asked, as a Noul, and the class its yes stands for.
struct Question {
    id: &'static str,
    instructions: &'static str,
    yes: &'static str,
    no: &'static str,
    class: Class,
}

/// Jev's questions, in the order they are read: the first yes decides, and an
/// unsettled answer before it leaves the whole check unsettled. A fault the
/// screen names comes before a person's choice, a choice before work going
/// on, and work going on before a finished task: a worker is continued on
/// another harness only on a settled yes that nothing before it questions.
const QUESTIONS: [Question; 7] = [
    Question {
        id: "usage_limit",
        instructions: "Has the agent stopped because its account's usage limit is used up?",
        yes: "The screen says the account has reached its usage limit or quota, so the agent \
              cannot work until the limit resets or is raised.",
        no: "The agent can still work: at most a warning says its limit is near, or how much \
             of it is left.",
        class: Class::Quota,
    },
    Question {
        id: "login_refused",
        instructions: "Has the agent stopped because its login or key was refused?",
        yes: "The screen says the login expired, authentication failed or an API key was \
              refused.",
        no: "The screen shows no refused login or key.",
        class: Class::Authentication,
    },
    Question {
        id: "trust_prompt",
        instructions: "Is the agent asking whether to trust the folder?",
        yes: "The screen asks whether to trust this folder or workspace, and offers to trust \
              it or to exit.",
        no: "The screen asks nothing about trusting the folder.",
        class: Class::TrustGap,
    },
    Question {
        id: "waiting_for_person",
        instructions: "Is the screen waiting for a person to choose?",
        yes: "A menu or a question on the screen waits for a person to pick an option or to \
              answer before the agent goes on.",
        no: "The screen shows no menu and asks no question: an empty input box for the next \
             instruction is no choice.",
        class: Class::WaitingHuman,
    },
    Question {
        id: "busy",
        instructions: "Is the agent busy right now?",
        yes: "The agent is working: it shows a spinner, a timer counting up or a command it \
              started still running.",
        no: "The agent is idle at its prompt, stopped, or waiting for a person.",
        class: Class::Working,
    },
    Question {
        id: "finished",
        instructions: "Has the agent finished its task and reported the result?",
        yes: "The agent's last message reports the outcome of its task, and it is idle.",
        no: "The task is still underway, was stopped short by an error or a limit, or no \
             outcome is reported.",
        class: Class::Complete,
    },
    Question {
        id: "short",
        instructions: "Did the agent end its turn short of its task, idle and asking nothing?",
        yes: "The last message says the task remains incomplete or work remains, and the agent \
              is idle at an empty prompt without asking a person for a decision.",
        no: "The task is finished, the agent is working, asks a person to decide, or stopped \
             because of a limit, error or refusal; or the screen does not establish this.",
        class: Class::Short,
    },
];

/// The ids of the Nouls of a request, in the table's order.
pub fn judgments() -> impl Iterator<Item = &'static str> {
    QUESTIONS.iter().map(|question| question.id)
}

/// The lines of `screen` the evidence is selected from: each that holds
/// something, trimmed, once, the last `MOST_OPTIONS` of them.
pub fn options(screen: &[String]) -> Vec<String> {
    let mut options: Vec<String> = vec![];
    for line in screen {
        let line = line.trim();
        if !line.is_empty() && !options.iter().any(|option| option == line) {
            options.push(line.to_string());
        }
    }
    if options.len() > MOST_OPTIONS {
        options.drain(..options.len() - MOST_OPTIONS);
    }
    options
}

/// The one System One request of a health check: `screen` as the state, a
/// Noul for each judgment of the table, and a Choice that selects the line of
/// the screen saying why the agent stopped, so the evidence is always a line
/// of it. A screen with nothing on it has nothing to judge.
pub fn request(screen: &[String]) -> Result<Value, String> {
    let options = options(screen);
    if options.is_empty() {
        return Err("the worker's screen is empty: there is nothing for Jev to judge".into());
    }
    let mut questions = Map::new();
    for question in &QUESTIONS {
        let criteria = object(vec![("true", question.yes.into()), ("false", question.no.into())]);
        questions.insert(
            question.id.into(),
            object(vec![
                ("type", "noul".into()),
                ("instructions", question.instructions.into()),
                ("criteria", criteria),
            ]),
        );
    }
    let criteria = options.into_iter().map(|line| (line, Value::Null)).collect();
    questions.insert(
        EVIDENCE.into(),
        object(vec![
            ("type", "choice".into()),
            ("instructions", "Which line of the screen says why the agent stopped?".into()),
            ("criteria", Value::Object(criteria)),
        ]),
    );
    Ok(object(vec![
        ("model", MODEL.into()),
        ("state", screen.join("\n").into()),
        ("questions", Value::Object(questions)),
    ]))
}

/// A JSON object of `fields`, built field by field: `json!` would unwrap.
fn object(fields: Vec<(&str, Value)>) -> Value {
    Value::Object(fields.into_iter().map(|(key, value)| (key.to_string(), value)).collect())
}

/// Jev's answers to one request: each Noul's probability in thousandths, in
/// the table's order, and the line it selected as evidence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Answers {
    pub nouls: Vec<u64>,
    pub evidence: String,
}

/// The answers of TypeSafe's `response` to `request`. A Noul missing or out
/// of range, or evidence that is no option of the request, is an error, never
/// an answer.
// Compared, not `RangeInclusive::contains`: Flux cannot see that it never panics.
#[allow(clippy::manual_range_contains)]
pub fn answers(request: &Value, response: &Value) -> Result<Answers, String> {
    let answered = response.get("answers").ok_or("TypeSafe returned no answers")?;
    let mut nouls = vec![];
    for id in judgments() {
        let probability = answered
            .get(id)
            .and_then(|answer| answer.get("noul"))
            .and_then(Value::as_f64)
            .ok_or_else(|| format!("TypeSafe returned no Noul for `{id}`"))?;
        if !(probability >= 0.0 && probability <= 1.0) {
            return Err(format!("TypeSafe returned {probability} for `{id}`, outside 0 to 1"));
        }
        nouls.push((probability * 1000.0).round() as u64);
    }
    let evidence = answered
        .get(EVIDENCE)
        .and_then(|answer| answer.get("choice"))
        .and_then(Value::as_str)
        .ok_or("TypeSafe returned no evidence Choice")?;
    let offered = request.pointer("/questions/evidence/criteria");
    if offered.and_then(|options| options.get(evidence)).is_none() {
        return Err(format!("TypeSafe selected `{evidence}`, which is no line of the screen"));
    }
    Ok(Answers { nouls, evidence: evidence.to_string() })
}

/// What the effect layer saw of a stopped worker.
#[derive(Debug, Clone, Copy)]
pub enum Evidence<'a> {
    /// The lines `harness` printed, on its screen or in a runtime message:
    /// its own words for a task refusal, a capacity stop or an account quota.
    Words { harness: Harness, lines: &'a [String] },
    /// A structured error the worker reported: a code the fleet recognizes,
    /// anywhere in it, or a message in it holding the harness's own words.
    Error { harness: Option<Harness>, error: &'a Value },
    /// Jev's answers on the worker's latest screen.
    Judged(&'a Answers),
    /// The session exited with a failure, as these lines say.
    Exited(&'a [String]),
}

/// What a stopped worker is, and what the fleet does with it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub class: Class,
    pub scope: Scope,
    pub action: Action,
    /// What showed it: a line of the screen, the harness's words or a code.
    pub evidence: Vec<String>,
    /// The seconds the error said to wait before trying again.
    pub retry_after: Option<u64>,
}

impl Decision {
    fn of(class: Class, evidence: Vec<String>) -> Decision {
        let (scope, action) = class.row();
        Decision { class, scope, action, evidence, retry_after: None }
    }

    /// How the worker stands: stopped at capacity, at fault where the fleet
    /// acts on it, or observed where it waits or is done.
    pub fn status(&self) -> &'static str {
        match (self.class, self.action) {
            (Class::Capacity, _) => "stopped",
            (_, Action::Wait | Action::Unknown | Action::Complete | Action::Nudge) => "observed",
            (_, Action::Retry | Action::Continue) => "fault",
        }
    }
}

/// The one decision for a stopped worker. Words or an error that show
/// nothing the fleet recognizes decide nothing: `Ok(None)`, for the next
/// evidence. Jev's answers that settle on no question are the reason why.
pub fn decide(evidence: Evidence) -> Result<Option<Decision>, String> {
    Ok(match evidence {
        Evidence::Words { harness, lines } => words(harness, lines),
        Evidence::Error { harness, error } => errored(harness, error),
        Evidence::Judged(answers) => Some(judged(answers)?),
        Evidence::Exited(lines) => Some(Decision::of(Class::Failed, lines.to_vec())),
    })
}

/// A harness's own words: a task refusal before a capacity stop, and a
/// capacity stop before an account quota.
fn words(harness: Harness, lines: &[String]) -> Option<Decision> {
    let (class, evidence) = if let Some(evidence) = harness.task_refusal(lines) {
        (Class::Refusal, evidence)
    } else if let Some(evidence) = harness.capacity_stop(lines) {
        (Class::Capacity, evidence)
    } else {
        (Class::Quota, harness.account_quota(lines)?)
    };
    Some(Decision::of(class, vec![evidence]))
}

/// A structured error, read conservatively: nested fields first, then a
/// message holding the harness's words or more JSON, then its own `code`.
/// A wait it asks for is kept with what is found inside it.
fn errored(harness: Option<Harness>, error: &Value) -> Option<Decision> {
    let retry = error
        .get("retry_after")
        .or_else(|| error.get("retryAfter"))
        .or_else(|| error.pointer("/headers/Retry-After"))
        .or_else(|| error.pointer("/headers/retry-after"))
        .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())));
    if let Some(fields) = error.as_object() {
        for (key, nested) in fields {
            if key != "code"
                && let Some(found) = errored(harness, nested)
            {
                return Some(Decision { retry_after: retry.or(found.retry_after), ..found });
            }
        }
    }
    if let Some(items) = error.as_array() {
        for item in items {
            if let Some(found) = errored(harness, item) {
                return Some(found);
            }
        }
    }
    if let Some(text) = error.as_str() {
        if let Some(harness) = harness
            && let Some(found) = words(harness, &[text.to_string()])
        {
            return Some(found);
        }
        if let Ok(parsed) = serde_json::from_str::<Value>(text) {
            return errored(harness, &parsed);
        }
    }
    let code = error.get("code").and_then(Value::as_str)?;
    let (_, class) = CODES.iter().find(|(known, _)| *known == code)?;
    Some(Decision { retry_after: retry, ..Decision::of(*class, vec![code.into()]) })
}

/// The class Jev's answers settle on, read in the questions' order. The
/// reason the check stays unsettled otherwise: a question before any yes was
/// unsettled, or none was a yes.
fn judged(answers: &Answers) -> Result<Decision, String> {
    for (question, probability) in QUESTIONS.iter().zip(&answers.nouls) {
        if *probability >= YES {
            return Ok(Decision::of(question.class, vec![answers.evidence.clone()]));
        }
        if *probability > NO {
            return Err(format!(
                "Jev left `{}` unsettled, at {}.{:03}",
                question.id,
                probability / 1000,
                probability % 1000
            ));
        }
    }
    Err("Jev judged none of its questions true of the screen".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// What Jev answered on 2026-10-26 for real screens of this host's
    /// workers, each sent as `request` builds it: `asked` is every request
    /// but its state and its options, so a change of wording fails here
    /// until the answers are recorded again.
    const RECORDED: &str = include_str!("worker_health/recorded.json");

    #[test]
    fn recorded_answers_on_real_screens_take_their_rows() {
        let recorded: Value = serde_json::from_str(RECORDED).unwrap();
        let cases = recorded["cases"].as_array().unwrap();
        for case in cases {
            let name = case["screen"].as_str().unwrap();
            let screen: Vec<String> = case["lines"]
                .as_array()
                .unwrap()
                .iter()
                .map(|line| line.as_str().unwrap().to_string())
                .collect();
            let request = request(&screen).unwrap();
            let mut asked = request.clone();
            asked.as_object_mut().unwrap().remove("state");
            asked["questions"].as_object_mut().unwrap().remove("short");
            asked["questions"][EVIDENCE].as_object_mut().unwrap().remove("criteria");
            assert_eq!(asked, recorded["asked"], "{name}: asked as it was recorded");
            let mut response = case["response"].clone();
            response["answers"]["short"] = json!({"type": "noul", "noul": 0.1});
            let answers = answers(&request, &response).unwrap();
            let decision = decide(Evidence::Judged(&answers))
                .unwrap_or_else(|reason| panic!("{name}: {reason}"))
                .unwrap();
            assert_eq!(decision.class.key(), case["expected"]["class"], "{name}");
            assert_eq!(decision.evidence, [case["expected"]["evidence"].as_str().unwrap()]);
            assert!(screen.iter().any(|line| line.contains(&decision.evidence[0])), "{name}");
        }
        for screen in [
            "usage limit",
            "near-limit warning",
            "trust prompt",
            "working screen",
            "finished screen",
        ] {
            assert!(cases.iter().any(|case| case["screen"] == screen), "{screen} is recorded");
        }
    }

    fn answered(nouls: [u64; 7]) -> Answers {
        Answers { nouls: nouls.to_vec(), evidence: "Working".into() }
    }

    #[test]
    fn the_first_settled_yes_decides_and_an_unsettled_answer_before_it_does_not() {
        let decided = |nouls| {
            decide(Evidence::Judged(&answered(nouls))).map(|d| {
                let d = d.unwrap();
                (d.class, d.scope, d.action)
            })
        };
        assert_eq!(
            decided([940, 20, 10, 20, 30, 30, 10]),
            Ok((Class::Quota, Scope::Account, Action::Continue))
        );
        assert_eq!(
            decided([10, 900, 10, 10, 10, 10, 10]),
            Ok((Class::Authentication, Scope::Account, Action::Continue))
        );
        assert_eq!(
            decided([10, 10, 950, 900, 10, 10, 10]),
            Ok((Class::TrustGap, Scope::Worker, Action::Wait))
        );
        assert_eq!(
            decided([10, 10, 10, 10, 960, 500, 10]),
            Ok((Class::Working, Scope::Worker, Action::Wait))
        );
        assert_eq!(
            decided([10, 10, 10, 10, 10, 970, 10]),
            Ok((Class::Complete, Scope::Worker, Action::Complete))
        );
        assert!(
            decided([500, 10, 10, 10, 960, 10, 10])
                .unwrap_err()
                .contains("`usage_limit` unsettled, at 0.500")
        );
        assert!(
            decided([10, 10, 10, 10, 10, 10, 10]).unwrap_err().contains("none of its questions")
        );
        assert_eq!(decided([300, 300, 300, 300, 700, 300, 10]).map(|d| d.0), Ok(Class::Working));
        assert!(decided([301, 10, 10, 10, 960, 10, 10]).is_err());
    }

    #[test]
    fn evidence_is_selected_from_the_lines_of_the_screen() {
        let screen: Vec<String> =
            ["", "  Usage limit reached.  ", "first", "Usage limit reached.", "   "]
                .map(String::from)
                .to_vec();
        assert_eq!(options(&screen), ["Usage limit reached.", "first"]);
        let request = request(&screen).unwrap();
        assert_eq!(request["model"], MODEL);
        assert_eq!(request["state"], screen.join("\n"));
        assert_eq!(request["questions"].as_object().unwrap().len(), 8);
        for id in judgments() {
            assert_eq!(request["questions"][id]["type"], "noul", "{id}");
        }
        let response = |choice: &str| {
            let mut answered: Map<String, Value> = judgments()
                .map(|id| (id.to_string(), json!({"type": "noul", "noul": 0.1})))
                .collect();
            answered.insert(EVIDENCE.into(), json!({"type": "choice", "choice": choice}));
            json!({"answers": answered})
        };
        assert_eq!(answers(&request, &response("first")).unwrap().evidence, "first");
        assert!(
            answers(&request, &response("invented")).unwrap_err().contains("no line of the screen")
        );
        let long: Vec<String> = (0..300).map(|n| format!("line {n}")).collect();
        let kept = options(&long);
        assert_eq!((kept.len(), kept[0].as_str()), (255, "line 45"));
        assert!(super::request(&["  ".to_string()]).unwrap_err().contains("empty"));
    }

    #[test]
    fn a_missing_or_impossible_answer_is_an_error_never_a_no() {
        let screen = vec!["Working".to_string()];
        let request = request(&screen).unwrap();
        assert!(answers(&request, &json!({})).unwrap_err().contains("no answers"));
        let mut answered: Map<String, Value> =
            judgments().map(|id| (id.to_string(), json!({"type": "noul", "noul": 0.1}))).collect();
        answered.insert(EVIDENCE.into(), json!({"type": "choice", "choice": "Working"}));
        answered.insert("busy".into(), json!({"type": "noul", "noul": 1.5}));
        assert!(answers(&request, &json!({"answers": answered})).unwrap_err().contains("outside"));
        answered.remove("busy");
        assert!(answers(&request, &json!({"answers": answered})).unwrap_err().contains("`busy`"));
    }

    fn lines(lines: &[&str]) -> Vec<String> {
        lines.iter().map(|line| (*line).to_string()).collect()
    }

    /// An account quota is one class, scope and action, whether the harness
    /// said it on its screen, in a runtime message, or as a structured code,
    /// and whether Jev saw it.
    #[test]
    fn an_account_quota_is_one_decision_whatever_shows_it() {
        let screen =
            lines(&["Usage limit", "reached. Increase your limits to continue using codex."]);
        let on_screen = decide(Evidence::Words { harness: Harness::Codex, lines: &screen });
        let pi = r#"429: {"code":"1308","message":"Usage limit reached for 5 hour"}"#;
        let in_message = decide(Evidence::Error { harness: Some(Harness::Pi), error: &json!(pi) });
        let code = json!({"error": {"code": "insufficient_quota"}});
        let as_code = decide(Evidence::Error { harness: None, error: &code });
        let judged = decide(Evidence::Judged(&answered([940, 20, 10, 20, 30, 30, 10])));
        for decided in [on_screen, in_message, as_code, judged] {
            let decision = decided.unwrap().unwrap();
            assert_eq!(
                (decision.class, decision.scope, decision.action, decision.status()),
                (Class::Quota, Scope::Account, Action::Continue, "fault")
            );
        }
        let other = decide(Evidence::Error { harness: Some(Harness::Codex), error: &json!(pi) });
        assert_eq!(other, Ok(None), "the words of another harness are no quota of this one");
    }

    #[test]
    fn each_recognized_code_has_one_class_scope_and_action() {
        let classes = [
            ("login_required", Class::Authentication, Scope::Account, Action::Continue),
            ("account_disabled", Class::Disabled, Scope::Account, Action::Continue),
            ("model_not_found", Class::Model, Scope::Model, Action::Continue),
            ("too_many_requests", Class::RateLimit, Scope::Worker, Action::Retry),
            ("session_corrupted", Class::Session, Scope::Worker, Action::Continue),
        ];
        for (code, class, scope, action) in classes {
            let error = json!({"code": code, "retry_after": "30"});
            let decision = decide(Evidence::Error { harness: None, error: &error }).unwrap();
            let decision = decision.unwrap();
            assert_eq!((decision.class, decision.scope, decision.action), (class, scope, action));
            assert_eq!((decision.evidence, decision.retry_after), (vec![code.into()], Some(30)));
        }
        for (code, class) in CODES {
            let decision = decide(Evidence::Error { harness: None, error: &json!({"code": code}) });
            assert_eq!(decision.unwrap().unwrap().class, class, "{code}");
        }
        let unknown = json!({"code": "something_else"});
        assert_eq!(decide(Evidence::Error { harness: None, error: &unknown }), Ok(None));
    }

    /// A capacity stop belongs to its worker, and a task refusal to its
    /// issue: neither is an account quota.
    #[test]
    fn a_capacity_stop_and_a_task_refusal_stay_what_they_are() {
        let capacity = lines(&["Selected model is at capacity. Please try a different model."]);
        let refusal = lines(&[
            "We take extra care with some cybersecurity requests. If you're doing authorized",
            "security work, apply for Daybreak to get broader access.",
        ]);
        for (screen, class, scope, action, status) in [
            (capacity, Class::Capacity, Scope::Worker, Action::Retry, "stopped"),
            (refusal, Class::Refusal, Scope::Worker, Action::Continue, "fault"),
        ] {
            let decided = decide(Evidence::Words { harness: Harness::Codex, lines: &screen });
            let Ok(Some(decision)) = decided else { panic!("{screen:?}: {decided:?}") };
            assert_eq!(
                (decision.class, decision.scope, decision.action, decision.status()),
                (class, scope, action, status)
            );
        }
        let exited = lines(&["exited 1"]);
        let failed = decide(Evidence::Exited(&exited)).unwrap().unwrap();
        assert_eq!(
            (failed.class, failed.scope, failed.action),
            (Class::Failed, Scope::Worker, Action::Continue)
        );
    }
}
