//! Worker health after the rules: Jev judges what a stopped worker's latest
//! screen shows, and this table turns its typed answers into the class, scope
//! and action the fleet carries out. Every question, criterion and threshold
//! is here, and nowhere else. The key command, the call to TypeSafe and the
//! worker's transitions are the effect layer's.

use serde_json::{Map, Value};

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
/// harness, or this worker alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Account,
    Worker,
}

/// What the fleet does with the worker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Look again at the next check, within the hour that bounds a wait.
    Wait,
    /// Continue it on the next harness of the kit, its work kept.
    Continue,
    /// Nothing: it is done.
    Complete,
}

/// One judgment Jev is asked, as a Noul, and the row of the table its yes
/// stands for.
struct Question {
    id: &'static str,
    instructions: &'static str,
    yes: &'static str,
    no: &'static str,
    class: &'static str,
    scope: Scope,
    action: Action,
}

/// The table, in the order it is read: the first yes decides, and an
/// unsettled answer before it leaves the whole check unsettled. A fault the
/// screen names comes before a person's choice, a choice before work going
/// on, and work going on before a finished task: a worker is continued on
/// another harness only on a settled yes that nothing before it questions.
const QUESTIONS: [Question; 6] = [
    Question {
        id: "usage_limit",
        instructions: "Has the agent stopped because its account's usage limit is used up?",
        yes: "The screen says the account has reached its usage limit or quota, so the agent \
              cannot work until the limit resets or is raised.",
        no: "The agent can still work: at most a warning says its limit is near, or how much \
             of it is left.",
        class: "quota",
        scope: Scope::Account,
        action: Action::Continue,
    },
    Question {
        id: "login_refused",
        instructions: "Has the agent stopped because its login or key was refused?",
        yes: "The screen says the login expired, authentication failed or an API key was \
              refused.",
        no: "The screen shows no refused login or key.",
        class: "authentication",
        scope: Scope::Account,
        action: Action::Continue,
    },
    Question {
        id: "trust_prompt",
        instructions: "Is the agent asking whether to trust the folder?",
        yes: "The screen asks whether to trust this folder or workspace, and offers to trust \
              it or to exit.",
        no: "The screen asks nothing about trusting the folder.",
        class: "trust_gap",
        scope: Scope::Worker,
        action: Action::Wait,
    },
    Question {
        id: "waiting_for_person",
        instructions: "Is the screen waiting for a person to choose?",
        yes: "A menu or a question on the screen waits for a person to pick an option or to \
              answer before the agent goes on.",
        no: "The screen shows no menu and asks no question: an empty input box for the next \
             instruction is no choice.",
        class: "waiting_human",
        scope: Scope::Worker,
        action: Action::Wait,
    },
    Question {
        id: "busy",
        instructions: "Is the agent busy right now?",
        yes: "The agent is working: it shows a spinner, a timer counting up or a command it \
              started still running.",
        no: "The agent is idle at its prompt, stopped, or waiting for a person.",
        class: "working",
        scope: Scope::Worker,
        action: Action::Wait,
    },
    Question {
        id: "finished",
        instructions: "Has the agent finished its task and reported the result?",
        yes: "The agent's last message reports the outcome of its task, and it is idle.",
        no: "The task is still underway, was stopped short by an error or a limit, or no \
             outcome is reported.",
        class: "complete",
        scope: Scope::Worker,
        action: Action::Complete,
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

/// The row of the table `answers` settle on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    pub class: &'static str,
    pub scope: Scope,
    pub action: Action,
    /// The line of the screen Jev selected.
    pub evidence: String,
}

/// The row of the table the answers settle on, read in its order. The
/// reason the check stays unsettled otherwise: a question before any yes was
/// unsettled, or none was a yes.
pub fn classify(answers: &Answers) -> Result<Decision, String> {
    for (question, probability) in QUESTIONS.iter().zip(&answers.nouls) {
        if *probability >= YES {
            return Ok(Decision {
                class: question.class,
                scope: question.scope,
                action: question.action,
                evidence: answers.evidence.clone(),
            });
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
            asked["questions"][EVIDENCE].as_object_mut().unwrap().remove("criteria");
            assert_eq!(asked, recorded["asked"], "{name}: asked as it was recorded");
            let decision = classify(&answers(&request, &case["response"]).unwrap())
                .unwrap_or_else(|reason| panic!("{name}: {reason}"));
            assert_eq!(decision.class, case["expected"]["class"], "{name}");
            assert_eq!(decision.evidence, case["expected"]["evidence"], "{name}");
            assert!(screen.iter().any(|line| line.contains(&decision.evidence)), "{name}");
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

    fn answered(nouls: [u64; 6]) -> Answers {
        Answers { nouls: nouls.to_vec(), evidence: "Working".into() }
    }

    #[test]
    fn the_first_settled_yes_decides_and_an_unsettled_answer_before_it_does_not() {
        let decided = |nouls| classify(&answered(nouls)).map(|d| (d.class, d.scope, d.action));
        assert_eq!(
            decided([940, 20, 10, 20, 30, 30]),
            Ok(("quota", Scope::Account, Action::Continue))
        );
        assert_eq!(
            decided([10, 900, 10, 10, 10, 10]),
            Ok(("authentication", Scope::Account, Action::Continue))
        );
        assert_eq!(
            decided([10, 10, 950, 900, 10, 10]),
            Ok(("trust_gap", Scope::Worker, Action::Wait))
        );
        assert_eq!(
            decided([10, 10, 10, 10, 960, 500]),
            Ok(("working", Scope::Worker, Action::Wait))
        );
        assert_eq!(
            decided([10, 10, 10, 10, 10, 970]),
            Ok(("complete", Scope::Worker, Action::Complete))
        );
        assert!(
            decided([500, 10, 10, 10, 960, 10])
                .unwrap_err()
                .contains("`usage_limit` unsettled, at 0.500")
        );
        assert!(decided([10, 10, 10, 10, 10, 10]).unwrap_err().contains("none of its questions"));
        assert_eq!(decided([300, 300, 300, 300, 700, 300]).map(|d| d.0), Ok("working"));
        assert!(decided([301, 10, 10, 10, 960, 10]).is_err());
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
        assert_eq!(request["questions"].as_object().unwrap().len(), 7);
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
}
