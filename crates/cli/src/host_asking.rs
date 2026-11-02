//! The host runner's resident asking session. Recorded watch human entries
//! become durable questions; one is delivered at a time to an interactive
//! Agent beside the watch, with no model call while it stands by.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use afkconvoy_core::harness::Harness;
use afkconvoy_core::host_watch;
use afkconvoy_core::prompt::brief_pointer;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::asking_terminal::{self, LaunchState};
use crate::git::Repo;
use crate::host_watch::{Attempt, gave_out, stop_marked};
use crate::orca::Worktree;
use crate::watch_notify::Entry;

const STATE: &str = "asking/state.json";

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub directory: PathBuf,
    pub stock: bool,
    #[serde(deserialize_with = "crate::kit::harness")]
    pub harness: Harness,
    pub model: Option<String>,
    #[serde(default)]
    pub model_source: Option<String>,
    pub terminal: String,
    pub transcript: PathBuf,
    pub session_started: u64,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Queued {
    pub question: Entry,
    #[serde(default)]
    pub aside: bool,
    #[serde(default)]
    pub withdrawn: bool,
    #[serde(default)]
    pub delivered: bool,
    #[serde(default)]
    pub started: Option<u64>,
    #[serde(default)]
    pub attempts: Vec<Attempt>,
}

impl Queued {
    fn new(question: Entry) -> Queued {
        Queued {
            question,
            aside: false,
            withdrawn: false,
            delivered: false,
            started: None,
            attempts: Vec::new(),
        }
    }
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct State {
    session: Option<Session>,
    current: Option<Queued>,
    pending: Vec<Queued>,
    finished: BTreeSet<String>,
    attempts: Vec<Attempt>,
    unavailable: Option<String>,
}

fn read() -> Result<State> {
    let file = crate::copies::cache()?.join(STATE);
    match std::fs::read(&file) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .with_context(|| format!("cannot read the asking session's state {}", file.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(State::default()),
        Err(error) => Err(error.into()),
    }
}

fn save(state: &State) -> Result<()> {
    let file = crate::copies::cache()?.join(STATE);
    std::fs::create_dir_all(file.parent().expect("asking state parent"))?;
    let temporary = file.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(state)?)?;
    std::fs::rename(temporary, file)?;
    Ok(())
}

pub(crate) fn state_lock() -> Result<std::fs::File> {
    let path = crate::copies::cache()?.join("asking/state.lock");
    std::fs::create_dir_all(path.parent().expect("asking state parent"))?;
    let lock = std::fs::OpenOptions::new().write(true).create(true).truncate(false).open(path)?;
    lock.lock()?;
    Ok(lock)
}

pub(crate) fn current() -> Result<Option<Queued>> {
    Ok(read()?.current)
}

/// Runs the asking scheduler for as long as its host runner holds it. The
/// session itself survives this thread and is recovered from durable state.
pub fn run(ended: &AtomicBool, say: impl Fn(String)) {
    let mut failing = None;
    while !ended.load(Ordering::Relaxed) {
        let result = step();
        let failed = result.as_ref().err().map(|error| format!("{error:#}"));
        if let Some(error) = &failed
            && failing.as_ref() != Some(error)
        {
            say(format!("asking session failed: {error}"));
        }
        if let Err(error) = result {
            let _ = note_failure(&format!("{error:#}"));
        }
        failing = failed;
        std::thread::sleep(Duration::from_millis(250));
    }
}

fn note_failure(why: &str) -> Result<()> {
    let _lock = state_lock()?;
    let mut state = read()?;
    state.unavailable = Some(why.into());
    save(&state)
}

fn step() -> Result<()> {
    let _lock = state_lock()?;
    let mut state = read()?;
    let journal = asking_terminal::read()?;
    if state.session.is_none()
        && let Some(record) = &journal
        && record.status != LaunchState::Ended
    {
        state.session = Some(record.session.clone());
    }
    if let Some(record) = &journal
        && let Some(question) = &record.question
        && state.current.as_ref().is_some_and(|current| {
            current.question.id == question.question.id && record.status != LaunchState::Idle
        })
    {
        state.current = Some(question.clone());
    } else if state.current.is_none()
        && let Some(record) = &journal
        && matches!(record.status, LaunchState::Ready | LaunchState::Sending | LaunchState::Running)
    {
        state.current = record.question.clone();
    }

    if settle_reported(&mut state)? {
        state.unavailable = None;
        return save(&state);
    }
    let (_, questions) = crate::watch_notify::questions()?;
    reconcile(&mut state, questions);
    state.unavailable = None;
    check_session(&mut state)?;
    if state.unavailable.is_some() {
        return save(&state);
    }
    if state.session.is_none() {
        start(&mut state)?;
        return save(&state);
    }
    deliver(&mut state)?;
    state.unavailable = None;
    save(&state)
}

fn settle_reported(state: &mut State) -> Result<bool> {
    let Some(current) = state.current.clone() else { return Ok(false) };
    let Some(report) = crate::asking_report::read(&current.question.id)? else { return Ok(false) };
    let Some(session) = &state.session else { bail!("a reported question has no asking session") };
    keep_usage(&current, session, crate::clock())?;
    match report.outcome {
        crate::asking_report::Outcome::Settled => {
            state.finished.insert(current.question.id.clone());
        }
        crate::asking_report::Outcome::Aside => {
            let mut aside = current;
            aside.aside = true;
            aside.delivered = false;
            aside.started = None;
            aside.attempts = Vec::new();
            state.pending.push(aside);
        }
    }
    state.current = None;
    asking_terminal::set_status(session, LaunchState::Idle)?;
    Ok(true)
}

fn reconcile(state: &mut State, questions: Vec<Entry>) {
    let present: BTreeSet<String> = questions.iter().map(|question| question.id.clone()).collect();
    state.finished.retain(|id| present.contains(id));
    state.pending.retain(|queued| {
        present.contains(&queued.question.id) && !state.finished.contains(&queued.question.id)
    });
    for question in questions {
        if state.finished.contains(&question.id)
            || state.current.as_ref().is_some_and(|current| current.question.id == question.id)
            || state.pending.iter().any(|queued| queued.question.id == question.id)
        {
            continue;
        }
        state.pending.push(Queued::new(question));
    }
    if let Some(current) = &mut state.current
        && !present.contains(&current.question.id)
        && current.delivered
    {
        current.withdrawn = true;
    }
}

fn check_session(state: &mut State) -> Result<()> {
    let Some(mut session) = state.session.take() else { return Ok(()) };
    let worktree = Worktree::at(session.directory.clone(), String::new());
    match asking_terminal::locate(&session) {
        Ok(Some(terminal)) => {
            session.terminal = terminal;
            let text = std::fs::read_to_string(&session.transcript).unwrap_or_default();
            let screen = worktree.screen(&session.terminal, host_watch::EVIDENCE_LINES)?;
            let (_, runtime) =
                worktree.terminal_health(&session.terminal, session.harness, crate::clock())?;
            let fault = crate::health::watch_diagnosis(session.harness, &screen, &runtime)
                .or_else(|| gave_out(session.harness, &text, host_watch::exited(&text)));
            if let Some(fault) = fault {
                stop_marked(&session.id)?;
                crate::health::watch_fault(session.harness, &fault, crate::clock())?;
                close(&worktree, &session.terminal, true)?;
                let attempt = Attempt {
                    harness: session.harness,
                    model: session.model.clone(),
                    model_source: session.model_source.clone(),
                    terminal: session.terminal.clone(),
                    why: fault.evidence.join("; "),
                    did: vec![],
                    screen,
                };
                if let Some(current) = &mut state.current {
                    current.attempts.push(attempt.clone());
                }
                state.attempts.push(attempt);
                asking_terminal::set_status(&session, LaunchState::Ended)?;
                reset_delivery(state);
            } else if host_watch::exited(&text).is_some() {
                stop_marked(&session.id)?;
                asking_terminal::set_status(&session, LaunchState::Ended)?;
                reset_delivery(state);
            } else {
                state.session = Some(session);
            }
        }
        Ok(None) => {
            stop_marked(&session.id)?;
            asking_terminal::set_status(&session, LaunchState::Ended)?;
            reset_delivery(state);
        }
        Err(error) => {
            state.unavailable = Some(asking_terminal::recovery(&format!("{error:#}")));
            state.session = Some(session);
        }
    }
    Ok(())
}

fn reset_delivery(state: &mut State) {
    if let Some(current) = &mut state.current {
        current.delivered = false;
    }
}

fn close(worktree: &Worktree, terminal: &str, live: bool) -> Result<()> {
    if live {
        worktree.close_terminal(terminal)?;
        if worktree.live(terminal)? {
            bail!("the asking session's terminal {terminal} is still live after it was closed");
        }
    }
    Ok(())
}

fn take_next(state: &mut State) {
    if state.current.is_some() {
        return;
    }
    if let Some(at) = state.pending.iter().position(|question| !question.aside) {
        state.current = Some(state.pending.remove(at));
    }
}

fn start(state: &mut State) -> Result<()> {
    take_next(state);
    if state.current.is_none() {
        return Ok(());
    }
    let settings = crate::host_settings::of_host()?;
    let directory = crate::host_watch::directory(&settings)?;
    if settings.watch_dir.is_some() && !directory.is_dir() {
        bail!(
            "the watch directory {} is not a directory: `afkconvoy run --watch-dir <dir>` names the operator's",
            directory.display()
        );
    }
    let stock = settings.watch_dir.is_none();
    let mut failures = vec![];
    for candidate in crate::health::watch_candidates()? {
        let (model, model_source) =
            crate::codex_launch::watch_selection(candidate.harness, candidate.model)?;
        let mut session = Session {
            id: uuid::Uuid::new_v4().to_string(),
            directory: directory.clone(),
            stock,
            harness: candidate.harness,
            model,
            model_source,
            terminal: String::new(),
            transcript: PathBuf::new(),
            session_started: crate::clock(),
        };
        if let Some(current) = &mut state.current {
            current.delivered = false;
        }
        state.session = Some(session.clone());
        save(state)?;
        match launch(&mut session, state) {
            Ok(()) => {
                state.session = Some(session);
                state.unavailable = None;
                if let Some(record) = asking_terminal::read()? {
                    state.current = record.question;
                }
                return Ok(());
            }
            Err(error) => {
                failures.push(format!("{}: {error:#}", candidate.harness.key()));
                state.session = Some(session);
                if asking_terminal::read()?.is_some_and(|record| {
                    matches!(
                        record.status,
                        LaunchState::Creating | LaunchState::Ready | LaunchState::Sending
                    )
                }) {
                    state.unavailable = Some(asking_terminal::recovery(&failures.join("; ")));
                    return Ok(());
                }
                state.session = None;
            }
        }
    }
    state.unavailable = Some(match failures {
        failures if failures.is_empty() => {
            "the host kit holds no harness free of a fault on record".into()
        }
        failures => failures.join("; "),
    });
    Ok(())
}

fn launch(session: &mut Session, state: &State) -> Result<()> {
    if session.stock {
        std::fs::create_dir_all(&session.directory)?;
        crate::orca::watch_directory(&session.directory)?;
    }
    let directory = crate::copies::cache()?.join("asking/sessions").join(&session.id);
    std::fs::create_dir_all(&directory)?;
    session.transcript = directory.join("session.log");
    let repo = Repo::new(session.directory.clone());
    let launches = crate::launch::Launches::new(&repo, true)?;
    let command = launches.command(session.harness)?;
    let line = crate::codex_launch::watch_line(
        session.harness,
        &session.directory,
        session.model.as_deref(),
        command.as_str(),
        None,
    )?;
    let line = launches.worker_line(session.harness, &line);
    let line = format!("export {}=host; {line}", host_watch::WATCH_ENV);
    let line = host_watch::session(&line, &session.transcript.display().to_string(), &session.id);
    let question = state.current.as_ref().context("no question to deliver")?;
    write_brief(session, question, &state.pending)?;
    let worktree = Worktree::at(session.directory.clone(), String::new());
    asking_terminal::prepare(session, &line, question)?;
    let line = asking_terminal::launch_line(session, &line);
    match worktree.start_watch(&line, asking_terminal::TITLE) {
        Ok(terminal) => {
            asking_terminal::created(session, &terminal)?;
            session.terminal = terminal;
        }
        Err(error) => {
            session.terminal =
                asking_terminal::locate(session)?.with_context(|| format!("{error:#}"))?;
        }
    }
    resume_delivery(session)
}

fn deliver(state: &mut State) -> Result<()> {
    take_next(state);
    let Some(session) = state.session.clone() else { return Ok(()) };
    let Some(record) = asking_terminal::read()? else { return Ok(()) };
    let Some(mut current) = state.current.clone() else {
        if record.status != LaunchState::Idle {
            asking_terminal::set_status(&session, LaunchState::Idle)?;
        }
        return Ok(());
    };
    if !current.delivered
        && !crate::watch_notify::questions()?.1.iter().any(|q| q.id == current.question.id)
    {
        state.current = None;
        asking_terminal::set_status(&session, LaunchState::Idle)?;
        return Ok(());
    }
    match record.status {
        LaunchState::Creating => bail!("the asking session's creation is not yet confirmed"),
        LaunchState::Sending => resume_delivery(&session)?,
        LaunchState::Ready => {
            if current.withdrawn && !record.question.as_ref().is_some_and(|q| q.withdrawn) {
                finish_withdrawn(state, &session, &current)?;
                return Ok(());
            }
            write_brief(&session, &current, &state.pending)?;
            resume_delivery(&session)?;
        }
        LaunchState::Running | LaunchState::Idle => {
            let delivered = record.question.as_ref().is_some_and(|question| {
                question.question.id == current.question.id && question.delivered
            });
            if current.withdrawn && !record.question.as_ref().is_some_and(|q| q.withdrawn) {
                let worktree = Worktree::at(session.directory.clone(), String::new());
                if !worktree.watch_input_ready(&session.terminal)? {
                    return Ok(());
                }
                current.delivered = delivered;
                asking_terminal::begin_question(&session, &current)?;
                write_brief(&session, &current, &state.pending)?;
                resume_delivery(&session)?;
            } else if !delivered {
                let worktree = Worktree::at(session.directory.clone(), String::new());
                if !worktree.watch_input_ready(&session.terminal)? {
                    return Ok(());
                }
                asking_terminal::begin_question(&session, &current)?;
                write_brief(&session, &current, &state.pending)?;
                resume_delivery(&session)?;
            } else if current.withdrawn {
                finish_withdrawn(state, &session, &current)?;
                return Ok(());
            }
        }
        LaunchState::Ended => {
            state.session = None;
            reset_delivery(state);
            return Ok(());
        }
    }
    if let Some(record) = asking_terminal::read()? {
        state.current = record.question;
    }
    Ok(())
}

fn finish_withdrawn(state: &mut State, session: &Session, current: &Queued) -> Result<()> {
    keep_usage(current, session, crate::clock())?;
    state.finished.insert(current.question.id.clone());
    state.current = None;
    asking_terminal::set_status(session, LaunchState::Idle)
}

fn brief_file(session: &Session, question: &Queued) -> Result<PathBuf> {
    let kind = if question.withdrawn { "withdrawal" } else { "question" };
    Ok(crate::copies::cache()?
        .join("asking/sessions")
        .join(&session.id)
        .join(format!("{kind}-{}.md", question.question.id)))
}

fn write_brief(session: &Session, question: &Queued, pending: &[Queued]) -> Result<()> {
    let path = brief_file(session, question)?;
    std::fs::create_dir_all(path.parent().expect("asking brief parent"))?;
    std::fs::write(&path, brief(session, question, pending)?)
        .with_context(|| format!("cannot write asking brief {}", path.display()))
}

fn brief(_session: &Session, question: &Queued, pending: &[Queued]) -> Result<String> {
    let entry = &question.question;
    let more = pending.iter().filter(|queued| !queued.aside).count();
    let waiting: Vec<String> = pending
        .iter()
        .map(|queued| {
            format!(
                "- `{}`{}: {}",
                queued.question.id,
                if queued.aside { " (put aside)" } else { "" },
                queued.question.human.question
            )
        })
        .collect();
    if question.withdrawn {
        let push = host_watch::asking_push(&format!("Withdrawn: {}", entry.human.question), 0);
        return Ok(format!(
            "{push}\n\n# A question withdrawn by the host runner\n\nThe condition `{}` recovered after this question was delivered. The question is withdrawn: do not investigate, act, or report it settled. Your last message is exactly the one-line withdrawal shown here, then return to interactive input and await the next question. Do not exit, poll or sleep.\n\n```text\n{push}\n```\n",
            entry.condition
        ));
    }
    let push = host_watch::asking_push(&entry.human.question, more);
    let report_file = crate::asking_report::report_file(&entry.id)?;
    let report_line = afkconvoy_core::line::Line::new()
        .fixed("afkconvoy asked --question ")
        .word(&entry.id)
        .fixed(" --text-file ")
        .word(&report_file.display().to_string())
        .own();
    Ok(format!(
        "# A human question from this host's watch\n\nYou are **afkconvoy asks**, the host runner's resident interactive asking session in the watch's directory. You are not the watch: run no watch round, take no look, and report nothing through `afkconvoy watched`; do no work of your own. The host runner delivered this question; your conversation with the operator, and what they tell you here, is the work.\n\n## Ask now, then stop on the question\n\nCondition: `{}`\nIssue: {}\nWatch: `{}`\nQuestion: {}\nAnswer line: `{}`\n{} more questions wait after this one.\n\nAsk the operator now. Your last message of this turn must be exactly this one line, with nothing after it, so Orca's phone push shows the question and opens this Terminal:\n\n```text\n{}\n```\n\n## Work with the operator\n\nClarify the question when asked, investigate and consult with the operator, and act only with their authority through the same commands a human uses: `rule`, `send`, `tell`, and issues opened as the tracker's docs say.\n\n## Carry out what the operator tells you\n\nWhen the operator tells you in this session to do something — a host fix or install, a ruling, a hand-back, a ticket — carry it out yourself now; do not leave it for a later watch round. That is the operator acting. Keep within the watch's bounds: {bounds}. Leave these with the operator, never act on them yourself:\n{kinds}\n\nCheck that it worked, then record it on the issue with the operator's quoted words and their time. The operator's actual words, quoted with their time on the issue they decide, are the human instruction and source. Your own words establish no human intent. Never invent or attribute an answer to the operator.\n\nWhen settled, record the operator's quoted words and their time on the issue, then write `{}` as {{\"outcome\":\"settled\",\"url\":\"<comment-url>\"}} and run the exact command below. To put the question aside at the operator's request, write {{\"outcome\":\"aside\"}} to that file instead; it stays pending. Only this report lets the host runner deliver the next question. If the operator asks what waits, list these questions, including those put aside:\n{}\n\nAfter the report records, return to interactive input and await the next message in this same session. Do not exit, poll, sleep in a tool loop, or call a model while idle.\n\n{}\n",
        entry.condition,
        entry.url.as_deref().unwrap_or("none (a host condition)"),
        entry.watch,
        entry.human.question,
        entry.human.answer,
        more,
        push,
        report_file.display(),
        if waiting.is_empty() { "None.".into() } else { waiting.join("\n") },
        report_line,
        bounds = crate::host_watch::BOUNDS,
        kinds = crate::host_watch::HUMAN_KINDS,
    ))
}

fn resume_delivery(session: &Session) -> Result<()> {
    let Some(mut record) =
        asking_terminal::read()?.filter(|record| record.session.id == session.id)
    else {
        return Ok(());
    };
    match record.status {
        LaunchState::Running | LaunchState::Idle | LaunchState::Ended => return Ok(()),
        LaunchState::Sending if record.request.is_none() => {
            bail!("the asking session's question has an uncertain receipt")
        }
        LaunchState::Sending => (),
        LaunchState::Creating => bail!("the asking session's creation is not yet confirmed"),
        LaunchState::Ready => (),
    }
    let worktree = Worktree::at(session.directory.clone(), String::new());
    if record.status == LaunchState::Ready {
        worktree.await_ready(&session.terminal, 60, session.harness, &record.command)?;
        if !worktree.watch_input_ready(&session.terminal)? {
            bail!("the resident asking session is busy; its question remains undelivered");
        }
        record.request = Some(uuid::Uuid::new_v4().to_string());
        record.status = LaunchState::Sending;
        record.offered = Some(crate::clock());
        asking_terminal::save(&record)?;
    }
    let question = record.question.as_ref().context("asking delivery has no question")?;
    let brief = brief_file(session, question)?;
    let earliest = record.offered.unwrap_or_else(crate::clock);
    if !crate::watch_notify::owns(&question.question, "asking question")? {
        record.question = None;
        record.status = LaunchState::Idle;
        asking_terminal::save(&record)?;
        return Ok(());
    }
    let sent = worktree.send_watch_round(
        &session.terminal,
        &brief_pointer(&brief.display().to_string()),
        record.request.as_deref().expect("saved request"),
    );
    let receipt = match sent {
        Ok(receipt) => receipt,
        Err(error) => {
            if worktree.saw_turn(session.harness, earliest)? {
                json!({"send": {"accepted": true, "prompt": {"stages": ["session_turn"]}}})
            } else {
                return Err(error);
            }
        }
    };
    record.receipt = Some(receipt.clone());
    asking_terminal::save(&record)?;
    let evidence =
        if receipt["send"]["accepted"] == true && worktree.prove(session.harness, earliest)? {
            Some(json!({"kind": "session_turn"}))
        } else {
            None
        };
    if evidence.is_none() {
        bail!(
            "asking question {} submission is unproven; retained request {} and receipt {}; no fresh input will be sent",
            question.question.id,
            record.request.as_deref().unwrap_or("unknown"),
            receipt
        );
    }
    if let Some(question) = &mut record.question {
        question.delivered = true;
        question.started = record.offered;
    }
    record.execution_evidence = evidence;
    asking_terminal::save(&record)?;
    asking_terminal::set_status(session, LaunchState::Running)
}

fn keep_usage(current: &Queued, session: &Session, ended: u64) -> Result<()> {
    use afkconvoy_core::account::Tokens;
    let start = current.started;
    let mut tokens = Tokens::new();
    let mut known = start.is_some();
    let attempts = current.attempts.iter().map(|attempt| attempt.harness).chain([session.harness]);
    for harness in attempts {
        let Some(start) = start else { continue };
        let Some(usage) = crate::session::tokens_between(
            &session.directory,
            harness,
            start,
            ended,
            &crate::copies::cache()?.join("asking/sessions"),
        )
        .ok()
        .flatten() else {
            known = false;
            break;
        };
        for (model, counts) in usage {
            tokens.entry(model).or_default().add(&counts);
        }
    }
    let path = crate::asking_report::usage_file(&current.question.id)?;
    let prior = match std::fs::read(&path) {
        Ok(bytes) => Some(serde_json::from_slice::<afkconvoy_core::accounts::Usage>(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error.into()),
    };
    let earlier_start = prior.as_ref().and_then(|prior| prior.start);
    if let Some(prior) = prior {
        tokens = match (prior.tokens, known.then_some(tokens)) {
            (Some(mut earlier), Some(later)) => {
                for (model, counts) in later {
                    earlier.entry(model).or_default().add(&counts);
                }
                earlier
            }
            _ => {
                known = false;
                Tokens::new()
            }
        };
    }
    let start = prior_start(earlier_start, start);
    let usage = afkconvoy_core::accounts::Usage {
        id: current.question.id.clone(),
        harness: Some(session.harness.key().to_string()),
        model: session.model.clone(),
        start,
        end: Some(ended),
        seconds: start.and_then(|start| ended.checked_sub(start)),
        tokens: known.then_some(tokens),
        conditions: vec![current.question.condition.clone()],
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = path.with_extension(format!("{}.tmp", crate::unique()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&usage)?)?;
    std::fs::rename(temporary, path)?;
    Ok(())
}

fn prior_start(prior: Option<u64>, current: Option<u64>) -> Option<u64> {
    match (prior, current) {
        (Some(prior), Some(current)) => Some(prior.min(current)),
        (prior, current) => prior.or(current),
    }
}

/// Every asking question's retained usage, including put-aside and withdrawn
/// questions whose later segments have been merged into the same record.
pub fn usages() -> Result<Vec<afkconvoy_core::accounts::Usage>> {
    let directory = crate::copies::cache()?.join("asking/reports");
    let entries = match std::fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut usages = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("usage.json");
        match std::fs::read(&path) {
            Ok(bytes) => usages.push(
                serde_json::from_slice(&bytes)
                    .with_context(|| format!("cannot read asking usage {}", path.display()))?,
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(usages)
}

/// What `doctor` and the panel say of the asking session and its durable queue.
pub fn json() -> Value {
    match read() {
        Ok(state) => {
            let status = if state.unavailable.is_some() {
                "unavailable"
            } else if state.current.is_some() {
                "asking"
            } else if state.session.is_some() {
                "standby"
            } else {
                "absent"
            };
            json!({
                "session": state.session.as_ref().map(|session| json!({
                    "id": session.id, "directory": session.directory,
                    "harness": session.harness, "terminal": session.terminal, "status": status,
                })),
                "status": status,
                "current": state.current,
                "pending": state.pending,
                "unavailable": state.unavailable,
                "association": asking_terminal::json(),
            })
        }
        Err(error) => json!({"status": "uncertain", "error": format!("{error:#}"),
            "association": asking_terminal::json()}),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_question_brief_starts_with_a_bounded_push_and_carries_its_context() {
        let session: Session = serde_json::from_value(json!({
            "id": "session", "directory": "/operator", "stock": false,
            "harness": "codex", "model": null, "terminal": "asks", "transcript": "",
            "session_started": 1,
        }))
        .unwrap();
        let entry: Entry = serde_json::from_value(json!({
            "id": "episode", "condition": "finding:host:spend", "url": null,
            "watch": "round", "human": {"question": "Approve this spend?", "answer": "afkconvoy kit"},
            "sequence": 1, "sent": false, "failures": 0, "attempted_look": null,
        }))
        .unwrap();
        let brief = brief(&session, &Queued::new(entry), &[]).unwrap();
        assert!(brief.starts_with("# A human question"));
        assert!(brief.contains("Approve this spend?"));
        assert!(brief.contains("```text\nApprove this spend?\n```"));
        assert!(brief.contains("afkconvoy asked --question 'episode'"));
        assert!(host_watch::asking_push("Approve this spend?", 0).chars().count() <= 180);
    }

    #[test]
    fn the_asking_session_acts_on_the_operators_instruction_and_runs_no_round() {
        let session: Session = serde_json::from_value(json!({
            "id": "session", "directory": "/operator", "stock": false,
            "harness": "codex", "model": null, "terminal": "asks", "transcript": "",
            "session_started": 1,
        }))
        .unwrap();
        let entry: Entry = serde_json::from_value(json!({
            "id": "episode", "condition": "finding:host:node", "url": null,
            "watch": "round", "human": {"question": "Install node?", "answer": "afkconvoy kit"},
            "sequence": 1, "sent": false, "failures": 0, "attempted_look": null,
        }))
        .unwrap();
        let brief = brief(&session, &Queued::new(entry), &[]).unwrap();
        assert!(brief.contains("carry it out yourself now"));
        assert!(brief.contains("a host fix or install, a ruling, a hand-back, a ticket"));
        assert!(brief.contains(crate::host_watch::BOUNDS));
        assert!(brief.contains("operator's quoted words and their time"));
        assert!(brief.contains("run no watch round, take no look"));
        assert!(brief.contains("report nothing through `afkconvoy watched`"));
        assert!(!brief.contains("host repair"));
    }
}
