//! A heavy command runs alone on its host (docs/adr/0019): the queue of
//! `afkconvoy exclusive` commands that every fleet of a host shares, held by
//! its host runner (docs/adr/0023). The order it runs in, how many are ahead
//! of a ticket, and since when a worker has waited in it are decided here;
//! holding it, and the line each asker is kept on, are the effect layer's.
//!
//! A worker's fast gate runs in a lane of its own (docs/adr/0049): no slow
//! gate, turn or other command holds a place it needs, and fast gates of one
//! repo run side by side, as many at once as the host says. A slow gate
//! runs in a slow lane of its own (docs/adr/0048): a round's slow gate is
//! counted apart from the worker lanes, where the round's turns wait.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

/// One `exclusive` command in the queue, as its ticket says it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Ticket {
    /// The order it joined the queue in, as the host runner numbers it: of
    /// two that asked the same second, the lower runs first.
    pub number: u64,
    /// The process that waits, or runs the command.
    pub pid: u32,
    /// The command it was asked to run, its arguments joined. A status
    /// answer names [`status_command`] of this.
    pub command: String,
    /// The worktree it was run in — or the directory, outside a checkout.
    pub dir: String,
    /// The issue of the worker that ran it, where its environment says one.
    pub issue: Option<u64>,
    /// A cut goes ahead of waiting worker commands so their queue cannot
    /// consume its time limit. Running commands keep their lanes.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cut: bool,
    /// A light gate's repo lane, independent of both host lanes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo_lane: Option<String>,
    /// The shared git directory, identifying a repo across its worktrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    /// A joining turn, which excludes another turn of its repo even when
    /// the two branches configure different kinds of gate.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_turn: bool,
    /// An immutable candidate worktree may overlap another candidate of
    /// its repo. Ordinary commands and serial barriers retain exclusion.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub candidate: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub validation: Option<crate::candidate::Validation>,
    /// Which of the two gates (docs/adr/0047) the command runs, where it is
    /// one run alone: a fast gate takes the fast lane.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate: Option<Gate>,
    /// When it asked to run, in epoch seconds: within each priority the one
    /// that asked first runs first, including in the next host runner.
    pub asked: u64,
    /// When it began to run; `None`: it waits.
    pub ran: Option<u64>,
}

impl Ticket {
    /// Whether its command runs: its turn was granted, and it has not left
    /// the queue. Every other ticket waits.
    pub fn running(&self) -> bool {
        self.ran.is_some()
    }
}

/// The queue's running work, as one reading of the process table finds it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Work {
    /// The process of each running ticket, and every process under one of
    /// them (`under`).
    pub pids: BTreeSet<u32>,
    /// Whether the reading holds the process of every running ticket. One
    /// that left the reading before it was taken leaves the work it ran
    /// unfound.
    pub whole: bool,
}

/// The work `queue`'s running tickets run, by the reading `table` of the
/// process table: each process's pid and its parent's.
pub fn running_work(queue: &[Ticket], table: &[(u32, u32)]) -> Work {
    let mut roots: Vec<u32> = Vec::new();
    let mut whole = true;
    for ticket in queue {
        if ticket.running() {
            roots.push(ticket.pid);
            whole = whole && table.iter().any(|(pid, _)| *pid == ticket.pid);
        }
    }
    Work { pids: under(&roots, table), whole }
}

/// `roots`, and every process of `table` under one of them: its parent is
/// one, or under one, as the reading says each process's parent. A process
/// whose parent ended was handed to another, and is under that one alone;
/// a pid used again, after the process that had it ended, is under whatever
/// started the new one. A reading taken while processes end and start may
/// say a loop of parents, which is followed once.
pub fn under(roots: &[u32], table: &[(u32, u32)]) -> BTreeSet<u32> {
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for &(pid, parent) in table {
        children.entry(parent).or_default().push(pid);
    }
    let mut found: BTreeSet<u32> = roots.iter().copied().collect();
    let mut next: Vec<u32> = roots.to_vec();
    while let Some(pid) = next.pop() {
        for &child in children.get(&pid).into_iter().flatten() {
            if found.insert(child) {
                next.push(child);
            }
        }
    }
    found
}

/// One of the two gates a repo's gate is split into, as a ticket names the
/// one its command runs alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Gate {
    Fast,
    Slow,
}

/// How many commands the host lets run at once: in the worker lanes, in
/// the fast lane, which only fast gates take, and in the slow lanes, which
/// only slow gates take.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lanes {
    pub worker: u64,
    pub fast: u64,
    pub slow: u64,
}

/// What a request is, as `queue`, doctor and the panel tell them apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    FastGate,
    SlowGate,
    Turn,
    Command,
}

impl Kind {
    /// It in words, as the panel says it.
    pub fn word(self) -> &'static str {
        match self {
            Kind::FastGate => "fast gate",
            Kind::SlowGate => "slow gate",
            Kind::Turn => "turn",
            Kind::Command => "command",
        }
    }
}

/// What `ticket` is: a fast or a slow gate run alone, a landing turn, or
/// any other command.
pub fn kind(ticket: &Ticket) -> Kind {
    if is_fast(ticket) {
        Kind::FastGate
    } else if ticket.gate == Some(Gate::Slow) {
        Kind::SlowGate
    } else if ticket.is_turn {
        Kind::Turn
    } else {
        Kind::Command
    }
}

/// Whether `ticket` runs in the fast lane: a fast gate's alone.
pub fn is_fast(ticket: &Ticket) -> bool {
    ticket.gate == Some(Gate::Fast)
}

/// Whether `ticket` runs in the slow lanes: a slow gate's alone.
pub fn is_slow(ticket: &Ticket) -> bool {
    ticket.gate == Some(Gate::Slow)
}

/// How many of `queue` are ahead of its ticket `number`: it runs once none
/// is. `None`: `number` is in it no more.
pub fn ahead(queue: &[Ticket], number: u64) -> Option<usize> {
    let mine = queue.iter().find(|ticket| ticket.number == number)?;
    Some(
        queue
            .iter()
            .filter(|ticket| same_lane(ticket, mine) && place(ticket) < place(mine))
            .count(),
    )
}

/// Commands contend only with others in the same lane. Fast gates share
/// the fast lane, whatever their repo; slow gates share the slow lanes;
/// every other command keeps its lane.
pub fn same_lane(a: &Ticket, b: &Ticket) -> bool {
    match (is_fast(a), is_fast(b)) {
        (true, true) => true,
        (false, false) => {
            a.repo_lane == b.repo_lane && (a.repo_lane.is_some() || is_slow(a) == is_slow(b))
        }
        _ => false,
    }
}

/// A repo has one identity across its worktrees. Older tickets identify
/// only their directory and still exclude another command from there.
pub fn same_repo(a: &Ticket, b: &Ticket) -> bool {
    a.repo.as_deref().unwrap_or(&a.dir) == b.repo.as_deref().unwrap_or(&b.dir)
}

/// Whether this ticket can start with the host's lane counts. Earlier
/// requests reserve available lanes in order, but a second request of a
/// running repo waits without holding another repo back from a free lane.
pub fn ready(queue: &[Ticket], number: u64, lanes: Lanes) -> bool {
    scheduling(queue, lanes, false)
        .iter()
        .any(|request| request.number == number && matches!(request.state, State::Eligible))
}

/// The scheduler's decision for a request at this observation. Eligible
/// requests reserve lanes in scheduler order; this is no future start order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Running,
    Eligible,
    Waiting,
}

/// A request occupying capacity now, or reserving it earlier in FIFO order.
#[derive(Debug, Clone, Serialize)]
pub struct Holder {
    pub number: u64,
    pub state: State,
}

/// One actual constraint, with the requests that hold or reserve it.
#[derive(Debug, Clone, Serialize)]
pub struct Constraint {
    pub kind: ConstraintKind,
    pub requests: Vec<Holder>,
    pub capacity: Option<u64>,
    pub explanation: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintKind {
    SameRepo,
    WorkerCapacity,
    RepoCapacity,
    FastCapacity,
    Paused,
}

#[derive(Debug, Clone, Serialize)]
pub struct Scheduling {
    pub number: u64,
    pub state: State,
    pub reasons: Vec<Constraint>,
}

/// Interpret every request with the very same reservations used to grant a
/// lane, with cuts before ordinary waiters. A same-repo waiter reserves nothing. Turns exclude each other
/// across worker and repo lanes too; fast gates of one repo do not exclude
/// each other. Pausing prevents every new grant.
pub fn scheduling(queue: &[Ticket], lanes: Lanes, paused: bool) -> Vec<Scheduling> {
    let ordered = in_order(queue.to_vec());
    let mut admitted: Vec<&Ticket> = ordered.iter().filter(|t| t.running()).collect();
    let mut decisions: Vec<Scheduling> = admitted
        .iter()
        .map(|ticket| Scheduling { number: ticket.number, state: State::Running, reasons: vec![] })
        .collect();
    for ticket in ordered.iter().filter(|t| !t.running()) {
        let decision = reservation(ticket, &admitted, lanes, paused);
        if matches!(decision.state, State::Eligible) {
            admitted.push(ticket);
        }
        decisions.push(decision);
    }
    decisions
}

/// One admission decision, with earlier reservations already applied.
fn reservation(ticket: &Ticket, admitted: &[&Ticket], lanes: Lanes, paused: bool) -> Scheduling {
    let limit = limit(ticket, lanes);
    let occupied: Vec<_> = admitted.iter().copied().filter(|t| same_lane(t, ticket)).collect();
    let own: Vec<_> = admitted
        .iter()
        .copied()
        .filter(|t| {
            (same_lane(t, ticket) || (t.is_turn && ticket.is_turn))
                && same_repo(t, ticket)
                && !(t.candidate && ticket.candidate)
                && !(is_fast(t) && is_fast(ticket))
        })
        .collect();
    let mut reasons = vec![];
    if paused {
        reasons.push(Constraint {
            kind: ConstraintKind::Paused,
            requests: vec![],
            capacity: None,
            explanation: "the host runner has paused new grants while moving to another release"
                .into(),
        });
    }
    if !own.is_empty() {
        reasons.push(Constraint {
            kind: ConstraintKind::SameRepo,
            requests: holders(&own),
            capacity: None,
            explanation: "this repo already holds or reserves this lane, or another turn of this repo holds or reserves a different lane".into(),
        });
    }
    if occupied.len() as u64 >= limit {
        reasons.push(Constraint {
            kind: if is_fast(ticket) {
                ConstraintKind::FastCapacity
            } else if ticket.repo_lane.is_some() {
                ConstraintKind::RepoCapacity
            } else {
                ConstraintKind::WorkerCapacity
            },
            requests: holders(&occupied),
            capacity: Some(limit),
            explanation: format!("all {limit} {} lane places are held by running requests or reserved by earlier eligible requests", lane(ticket)),
        });
    }
    let state = if reasons.is_empty() { State::Eligible } else { State::Waiting };
    Scheduling { number: ticket.number, state, reasons }
}

/// How many places the lane of `ticket` has.
pub fn limit(ticket: &Ticket, lanes: Lanes) -> u64 {
    if is_fast(ticket) {
        lanes.fast
    } else if ticket.repo_lane.is_some() {
        1
    } else if is_slow(ticket) {
        lanes.slow
    } else {
        lanes.worker
    }
}

fn holders(tickets: &[&Ticket]) -> Vec<Holder> {
    tickets
        .iter()
        .map(|ticket| Holder {
            number: ticket.number,
            state: if ticket.running() { State::Running } else { State::Eligible },
        })
        .collect()
}

/// Actual occupancy counts only running requests, never reservations. Free
/// capacity does not imply that a request of an occupied repo can use it.
#[derive(Debug, Serialize)]
pub struct Capacity {
    pub capacity: u64,
    pub used: u64,
    pub free: u64,
}

#[derive(Debug, Serialize)]
pub struct RepoLane {
    pub repo: String,
    #[serde(flatten)]
    pub occupancy: Capacity,
}

/// A ticket with durations at one observation time and its scheduler decision.
#[derive(Debug, Serialize)]
pub struct Observed {
    #[serde(flatten)]
    pub ticket: Ticket,
    pub lane: String,
    pub kind: Kind,
    pub elapsed_seconds: Option<u64>,
    pub waiting_seconds: u64,
    pub scheduling: Scheduling,
}

#[derive(Debug, Serialize)]
pub struct Snapshot {
    pub paused: bool,
    pub worker: Capacity,
    pub fast: Capacity,
    pub slow: Capacity,
    pub repos: Vec<RepoLane>,
    pub running: Vec<Observed>,
    pub waiting: Vec<Observed>,
}

/// The command a status answer names: one line, the program and the
/// arguments that say what is running.
///
/// `afkconvoy exclusive -- ./gate.sh` is returned as given. A shell script
/// (`sh -c`, `bash -lc`, or the same program with a path) is named by the
/// program that script runs — its `cargo nextest` line, else its
/// `afkconvoy exclusive` invocation, else the programs it runs — and not by
/// the script. An environment assignment is left out, on its own line or in
/// front of the program. The result contains no newline.
pub fn status_command(command: &str) -> String {
    if let Some((shell, script)) = invocation_script(command) {
        return script_line(script, &format!("{shell} -c"));
    }
    if command.contains(['\n', '\r']) || leading_assignment(command) {
        return script_line(command, &first_program(command));
    }
    command.to_string()
}

/// A shell's `-c` script, and that shell's name. The ticket joins arguments
/// with spaces, so the script keeps the newlines of its one argument.
fn invocation_script(command: &str) -> Option<(String, &str)> {
    let mut rest = command;
    let mut shell: Option<&str> = None;
    loop {
        let (word, after) = split_word(rest)?;
        if shell.is_none() {
            if is_shell(word) {
                shell = Some(base(word));
                rest = after;
                continue;
            }
            if crate::guard::is_assignment(word) || is_prefix_wrapper(word) || word.starts_with('-')
            {
                rest = after;
                continue;
            }
            return None;
        }
        if is_c_flag(word) {
            let script = after.trim_start_matches([' ', '\t', '\n', '\r']);
            return Some((shell.unwrap_or("sh").to_string(), script));
        }
        if word.starts_with('-') {
            rest = after;
            continue;
        }
        return None;
    }
}

fn script_line(script: &str, empty: &str) -> String {
    let chosen = choose(&command_words(script));
    if chosen.is_empty() { empty.to_string() } else { join_programs(&chosen) }
}

/// Words of each simple command. One quoted word that is itself a script is
/// read again, which is how a quoted `sh -c` argument is stored.
fn command_words(script: &str) -> Vec<Vec<String>> {
    let commands = crate::guard::simple_commands(script);
    if let [only] = commands.as_slice()
        && only.len() == 1
        && let Some(word) = only.first()
        && (word.contains('\n') || word.contains(';'))
    {
        return crate::guard::simple_commands(word);
    }
    commands
}

fn choose(commands: &[Vec<String>]) -> Vec<Vec<String>> {
    let real: Vec<Vec<String>> = commands.iter().filter_map(|words| program(words)).collect();
    if let Some(hit) = real.iter().find(|words| is_nextest(words)) {
        return vec![hit.clone()];
    }
    if let Some(hit) = real.iter().find(|words| is_exclusive(words)) {
        return vec![hit.clone()];
    }
    real
}

/// The program and its arguments, without a leading assignment or `env`.
/// A command that only sets up the shell is not a program.
fn program(words: &[String]) -> Option<Vec<String>> {
    let mut rest = words;
    loop {
        let word = rest.first()?;
        if word == "env" {
            rest = skip_env_flags(&rest[1..]);
            continue;
        }
        if crate::guard::is_assignment(word) {
            rest = &rest[1..];
            continue;
        }
        break;
    }
    let name = base(rest.first()?);
    if SETUP.contains(&name) { None } else { Some(rest.to_vec()) }
}

/// Flags of a leading `env`, and the value of one that takes an argument.
/// The program those flags apply to is what remains.
fn skip_env_flags(mut rest: &[String]) -> &[String] {
    while let Some((flag, tail)) = rest.split_first() {
        if !flag.starts_with('-') {
            break;
        }
        rest = tail;
        if matches!(flag.as_str(), "-u" | "--unset" | "-C" | "--chdir")
            && let Some((_, after)) = rest.split_first()
        {
            rest = after;
        }
    }
    rest
}

fn is_nextest(words: &[String]) -> bool {
    words.first().is_some_and(|word| base(word) == "cargo")
        && words.iter().any(|word| word == "nextest")
}

fn is_exclusive(words: &[String]) -> bool {
    words.first().is_some_and(|word| base(word) == "afkconvoy")
        && words.iter().any(|word| word == "exclusive")
}

fn leading_assignment(command: &str) -> bool {
    crate::guard::simple_commands(command).iter().any(|words| {
        words.first().is_some_and(|word| {
            word == "export" || word == "env" || crate::guard::is_assignment(word)
        })
    })
}

fn first_program(command: &str) -> String {
    command
        .split_whitespace()
        .find(|word| !word.contains('=') && !is_prefix_wrapper(word) && !is_shell(word))
        .unwrap_or("sh")
        .to_string()
}

fn join_programs(programs: &[Vec<String>]) -> String {
    programs.iter().map(|words| render(words)).collect::<Vec<_>>().join("; ")
}

fn render(words: &[String]) -> String {
    words
        .iter()
        .map(|word| {
            let flat = word.replace(['\n', '\r'], " ");
            if flat.contains([' ', '\'']) {
                format!("'{}'", flat.replace('\'', r"'\''"))
            } else {
                flat
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn split_word(text: &str) -> Option<(&str, &str)> {
    let text = text.trim_start_matches([' ', '\t', '\n', '\r']);
    if text.is_empty() {
        return None;
    }
    let end = text.find([' ', '\t', '\n', '\r']).unwrap_or(text.len());
    Some((&text[..end], &text[end..]))
}

fn is_shell(word: &str) -> bool {
    matches!(base(word), "sh" | "bash" | "zsh" | "dash" | "ksh")
}

fn is_c_flag(word: &str) -> bool {
    word.starts_with('-') && !word.starts_with("--") && word.contains('c')
}

fn is_prefix_wrapper(word: &str) -> bool {
    matches!(
        base(word),
        "sudo" | "env" | "command" | "exec" | "nohup" | "nice" | "time" | "builtin"
    )
}

fn base(word: &str) -> &str {
    word.rsplit(['/', '\\']).next().unwrap_or(word)
}

/// Commands that prepare a script and are not what it runs.
const SETUP: [&str; 15] = [
    "export", "set", "cd", "pushd", "popd", "umask", "hash", "local", "declare", "typeset",
    "readonly", "unset", "alias", "unalias", ":",
];

/// Common queue facts for operator output, doctor and the panel. Running
/// and waiting arrays each put cuts first, then follow ask time and request number. Repo lanes
/// are separate from worker capacity and sorted by their whole identity.
/// Each request's `command` is [`status_command`].
pub fn snapshot(queue: &[Ticket], lanes: Lanes, paused: bool, now: u64) -> Snapshot {
    let ordered = in_order(queue.to_vec());
    let decisions = scheduling(&ordered, lanes, paused);
    let mut repos: Vec<String> = ordered.iter().filter_map(|t| t.repo_lane.clone()).collect();
    repos.sort();
    repos.dedup();
    let repos = repos
        .into_iter()
        .map(|repo| RepoLane { occupancy: occupancy(&ordered, &format!("repo {repo}"), 1), repo })
        .collect();
    let mut result = Snapshot {
        paused,
        worker: occupancy(&ordered, "worker", lanes.worker),
        fast: occupancy(&ordered, "fast", lanes.fast),
        slow: occupancy(&ordered, "slow", lanes.slow),
        repos,
        running: vec![],
        waiting: vec![],
    };
    for (mut ticket, scheduling) in ordered.into_iter().zip(decisions) {
        ticket.command = status_command(&ticket.command);
        let observed = Observed {
            lane: lane(&ticket),
            kind: kind(&ticket),
            elapsed_seconds: ticket.ran.map(|ran| now.saturating_sub(ran)),
            waiting_seconds: ticket.ran.unwrap_or(now).saturating_sub(ticket.asked),
            scheduling,
            ticket,
        };
        if observed.ticket.running() {
            result.running.push(observed);
        } else {
            result.waiting.push(observed);
        }
    }
    result
}

/// How many places of the lane [`lane`] names `named` are used.
fn occupancy(queue: &[Ticket], named: &str, capacity: u64) -> Capacity {
    let used = queue.iter().filter(|t| t.running() && lane(t) == named).count() as u64;
    Capacity { capacity, used, free: capacity.saturating_sub(used) }
}

/// The lane a ticket names in the log, doctor and the panel.
pub fn lane(ticket: &Ticket) -> String {
    if is_fast(ticket) {
        return "fast".to_string();
    }
    match &ticket.repo_lane {
        Some(repo) => format!("repo {repo}"),
        None if is_slow(ticket) => "slow".to_string(),
        None => "worker".to_string(),
    }
}

/// Stable scheduler order: running first, then waiting; within each group,
/// cuts first, then ask time and request number. Position alone never
/// predicts which request starts next.
pub fn in_order(mut queue: Vec<Ticket>) -> Vec<Ticket> {
    queue.sort_by_key(place);
    queue
}

/// Running commands first, then waiting cuts, then other waiters; each
/// group keeps ask time and ticket order within its lanes.
fn place(ticket: &Ticket) -> (bool, bool, u64, u64) {
    (!ticket.running(), !ticket.cut, ticket.asked, ticket.number)
}

/// Since when a worker in the worktree `dir` has waited in the queue — or run
/// what it waited for, its wait over — by the tickets of `live`: since the
/// first of its that asked. `None`: it has nothing in the queue. A ticket
/// counts for `dir` when it was run there or in a directory under it.
pub fn queued_since(live: &[Ticket], dir: &str) -> Option<u64> {
    let dir = dir.trim_end_matches('/');
    let mut since: Option<u64> = None;
    for ticket in live {
        if !within(&ticket.dir, dir) {
            continue;
        }
        since = match since {
            Some(earlier) if earlier <= ticket.asked => Some(earlier),
            _ => Some(ticket.asked),
        };
    }
    since
}

/// Whether the directory `of` is `dir` or one under it.
fn within(of: &str, dir: &str) -> bool {
    match of.strip_prefix(dir) {
        Some(rest) => rest.is_empty() || rest.starts_with('/'),
        None => false,
    }
}

/// What the log says of a ticket that has to wait: `ahead` are before it.
pub fn waits_line(ticket: &Ticket, ahead: usize) -> String {
    let before = match ahead {
        1 => "1 exclusive command".to_string(),
        many => format!("{many} exclusive commands"),
    };
    format!("waits behind {before} on this host to run `{}`", ticket.command)
}

/// What the log says of a ticket that runs its command in no queue, as `why`
/// says: no host runner runs, or the one there cannot hold it.
pub fn at_once_line(ticket: &Ticket, why: &str) -> String {
    format!("runs `{}` at once, in no queue: {why}", ticket.command)
}

/// What the log says of a ticket whose host runner ended as it waited: it
/// waits in the next one's queue, as it first asked.
pub fn rejoins_line(ticket: &Ticket) -> String {
    format!(
        "the host runner ended as `{}` waited: it waits in the next one's queue, from when it \
         asked",
        ticket.command
    )
}

/// What the log says of a ticket that runs its command at `now`.
pub fn runs_line(ticket: &Ticket, now: u64, ahead_of: usize) -> String {
    let waited = now.saturating_sub(ticket.asked);
    let _ = ahead_of;
    let line =
        format!("runs `{}` in the {} lane, after waiting {waited}s", ticket.command, lane(ticket));
    if ticket.cut {
        format!("runs as a cut, ahead of waiting worker commands; {line}")
    } else {
        line
    }
}

/// What the log says of a ticket whose command ended at `now`, as `ended`
/// says: `exit 0`, `killed by signal 9`.
pub fn ended_line(ticket: &Ticket, ended: &str, now: u64) -> String {
    let ran = now.saturating_sub(ticket.ran.unwrap_or(ticket.asked));
    format!("`{}` ended ({ended}) after {ran}s", ticket.command)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticket(number: u64, dir: &str, asked: u64) -> Ticket {
        Ticket {
            number,
            pid: 1,
            command: "cargo test".into(),
            dir: dir.into(),
            issue: Some(7),
            cut: false,
            repo_lane: None,
            repo: None,
            is_turn: false,
            candidate: false,
            validation: None,
            gate: None,
            asked,
            ran: None,
        }
    }

    const ONE: Lanes = Lanes { worker: 1, fast: 1, slow: 1 };

    fn lanes(worker: u64) -> Lanes {
        Lanes { worker, fast: 1, slow: 1 }
    }

    #[test]
    fn a_slow_gate_runs_in_its_own_lanes_beside_the_worker_lanes() {
        let lanes = Lanes { worker: 2, fast: 1, slow: 1 };
        let ticket = |number, slow: bool, ran: Option<u64>| Ticket {
            repo: Some(format!("r{number}")),
            gate: slow.then_some(Gate::Slow),
            ran,
            ..ticket(number, &format!("/r{number}"), 0)
        };
        let busy = [ticket(1, false, Some(1)), ticket(2, false, Some(1))];
        let mut queue = busy.to_vec();
        queue.push(ticket(3, true, None));
        assert!(ready(&queue, 3, lanes), "full worker lanes leave the slow lane free");
        queue.push(ticket(4, true, None));
        assert!(!ready(&queue, 4, lanes), "one slow lane runs one slow gate");
        let seen = snapshot(&queue, lanes, false, 5);
        assert_eq!((seen.worker.used, seen.slow.capacity), (2, 1));
        assert_eq!(lane(&queue[2]), "slow");
    }

    #[test]
    fn a_ticket_runs_once_none_that_asked_before_it_is_left() {
        let queue = [ticket(4, "/a", 0), ticket(9, "/b", 0), ticket(12, "/c", 0)];
        assert_eq!(ahead(&queue, 4), Some(0));
        assert_eq!(ahead(&queue, 12), Some(2));
        assert_eq!(ahead(&queue, 5), None, "one that left");
        let order = in_order(vec![ticket(9, "/b", 0), ticket(4, "/a", 0)]);
        let numbers: Vec<u64> = order.iter().map(|ticket| ticket.number).collect();
        assert_eq!(numbers, [4, 9]);
    }

    #[test]
    fn one_that_asked_again_of_the_next_host_runner_keeps_its_place_but_one_runs_first() {
        // 1 joined first and runs; 3 asked before 2, of the host runner that ended.
        let mut runs = ticket(1, "/a", 200);
        runs.ran = Some(210);
        let queue = [runs, ticket(2, "/b", 150), ticket(3, "/c", 100), ticket(4, "/d", 100)];
        let order = in_order(queue.to_vec());
        let numbers: Vec<u64> = order.iter().map(|ticket| ticket.number).collect();
        assert_eq!(numbers, [1, 3, 4, 2]);
        assert_eq!(ahead(&queue, 3), Some(1));
        assert_eq!(ahead(&queue, 2), Some(3));
    }

    #[test]
    fn light_repos_have_independent_turns() {
        let mut a = ticket(1, "/a", 100);
        a.repo_lane = Some("a".into());
        let mut b = ticket(2, "/b", 101);
        b.repo_lane = Some("b".into());
        let mut another_a = ticket(3, "/a", 102);
        another_a.repo_lane = Some("a".into());
        let queue = [a, b, another_a, ticket(4, "/c", 103)];
        assert_eq!(ahead(&queue, 2), Some(0));
        assert_eq!(ahead(&queue, 3), Some(1));
        assert_eq!(ahead(&queue, 4), Some(0));
    }

    #[test]
    fn two_lanes_admit_different_repos_and_skip_a_second_turn_of_a_running_repo() {
        let mut a = ticket(1, "/a/one", 100);
        a.repo = Some("a".into());
        a.ran = Some(100);
        let mut another_a = ticket(2, "/a/two", 101);
        another_a.repo = Some("a".into());
        let mut b = ticket(3, "/b/one", 102);
        b.repo = Some("b".into());
        let queue = [a, another_a, b];
        assert!(!ready(&queue, 2, lanes(2)));
        assert!(ready(&queue, 3, lanes(2)));
        assert!(!ready(&queue, 3, ONE));
    }

    #[test]
    fn only_independent_candidate_worktrees_may_overlap_in_one_repo() {
        let mut a = ticket(1, "/a/source-one", 0);
        a.repo = Some("a".into());
        a.is_turn = true;
        a.candidate = true;
        a.ran = Some(0);
        let mut b = ticket(2, "/a/source-two", 1);
        b.repo = Some("a".into());
        b.is_turn = true;
        b.candidate = true;
        assert!(ready(&[a.clone(), b.clone()], 2, lanes(2)));
        assert!(!ready(&[a.clone(), b.clone()], 2, ONE));
        b.candidate = false;
        assert!(!ready(&[a.clone(), b.clone()], 2, lanes(2)), "ordinary work retains exclusion");
        a.candidate = false;
        b.candidate = true;
        assert!(!ready(&[a, b], 2, lanes(2)), "a barrier excludes speculative validation");
    }

    fn gate(number: u64, dir: &str, part: Gate, asked: u64) -> Ticket {
        let mut gate = ticket(number, dir, asked);
        gate.repo = Some("a".into());
        gate.gate = Some(part);
        gate
    }

    #[test]
    fn a_fast_gate_never_waits_on_a_slow_gate_running_or_queued() {
        let mut slow = gate(1, "/a/one", Gate::Slow, 100);
        slow.ran = Some(100);
        let mut turn = ticket(2, "/a/two", 101);
        turn.repo = Some("a".into());
        turn.is_turn = true;
        let queued_slow = gate(3, "/a/three", Gate::Slow, 102);
        let fast = gate(4, "/a/four", Gate::Fast, 103);
        let queue = [slow, turn, queued_slow, fast];
        assert!(ready(&queue, 4, ONE), "a slow gate holds no place a fast gate needs");
        assert_eq!(ahead(&queue, 4), Some(0));
        assert!(!ready(&queue, 3, ONE), "the slow gate keeps the worker lane's rules");
    }

    #[test]
    fn fast_gates_of_one_repo_run_side_by_side_within_the_host_s_bound() {
        let mut first = gate(1, "/a/one", Gate::Fast, 100);
        first.ran = Some(100);
        let second = gate(2, "/a/two", Gate::Fast, 101);
        let third = gate(3, "/a/three", Gate::Fast, 102);
        let queue = [first, second, third];
        let two = Lanes { worker: 1, fast: 2, slow: 1 };
        let decisions = scheduling(&queue, two, false);
        assert!(matches!(decisions[1].state, State::Eligible));
        assert!(matches!(decisions[2].state, State::Waiting));
        for decision in &decisions {
            assert!(
                decision.reasons.iter().all(|r| !matches!(r.kind, ConstraintKind::SameRepo)),
                "{decision:?}"
            );
        }
        assert!(matches!(decisions[2].reasons[0].kind, ConstraintKind::FastCapacity));
        assert_eq!(decisions[2].reasons[0].capacity, Some(2));
        let three = Lanes { worker: 1, fast: 3, slow: 1 };
        assert!(ready(&queue, 3, three));
    }

    #[test]
    fn a_snapshot_tells_fast_and_slow_gates_apart_and_counts_the_fast_lane_alone() {
        let mut fast = gate(1, "/a/one", Gate::Fast, 100);
        fast.ran = Some(100);
        let mut slow = gate(2, "/a/two", Gate::Slow, 100);
        slow.ran = Some(101);
        let mut turn = ticket(3, "/a/three", 102);
        turn.is_turn = true;
        let seen = snapshot(&[fast, slow, turn, ticket(4, "/b", 103)], ONE, false, 150);
        let kinds: Vec<(Kind, &str)> = seen
            .running
            .iter()
            .chain(&seen.waiting)
            .map(|request| (request.kind, request.lane.as_str()))
            .collect();
        assert_eq!(
            kinds,
            [
                (Kind::FastGate, "fast"),
                (Kind::SlowGate, "slow"),
                (Kind::Turn, "worker"),
                (Kind::Command, "worker")
            ]
        );
        assert_eq!((seen.fast.used, seen.fast.free), (1, 0));
        assert_eq!((seen.worker.used, seen.worker.free), (0, 1), "the slow gate's lane is its own");
        assert_eq!((seen.slow.used, seen.slow.free), (1, 0));
        let wire = serde_json::to_value(&seen.running[0]).unwrap();
        assert_eq!(wire["gate"], "fast");
        assert_eq!(wire["kind"], "fast_gate");
    }

    #[test]
    fn an_older_ticket_has_no_train_priority() {
        let older = r#"{"number":1,"pid":2,"command":"cargo test","dir":"/a","issue":null,"asked":100,"ran":null}"#;
        let ticket: Ticket = serde_json::from_str(older).unwrap();
        assert_eq!(lane(&ticket), "worker");
        assert_eq!(kind(&ticket), Kind::Command, "an older ticket runs no gate alone");
    }

    #[test]
    fn a_worktree_waits_since_its_first_ticket_asked_and_counts_no_others() {
        let live = [
            ticket(1, "/w/issue-7", 300),
            ticket(2, "/w/issue-7/crates/cli", 200),
            ticket(3, "/w/issue-70", 100),
        ];
        assert_eq!(queued_since(&live, "/w/issue-7"), Some(200));
        assert_eq!(queued_since(&live, "/w/issue-7/"), Some(200));
        assert_eq!(queued_since(&live, "/w/issue-70"), Some(100));
        assert_eq!(queued_since(&live, "/w/issue-8"), None);
    }

    #[test]
    fn each_line_names_the_command() {
        let mut waits = ticket(2, "/a", 100);
        assert_eq!(
            waits_line(&waits, 1),
            "waits behind 1 exclusive command on this host to run `cargo test`"
        );
        assert_eq!(
            runs_line(&waits, 130, 0),
            "runs `cargo test` in the worker lane, after waiting 30s"
        );
        assert_eq!(
            at_once_line(&waits, "no host runner runs on this host"),
            "runs `cargo test` at once, in no queue: no host runner runs on this host"
        );
        assert_eq!(
            rejoins_line(&waits),
            "the host runner ended as `cargo test` waited: it waits in the next one's queue, \
             from when it asked"
        );
        waits.ran = Some(130);
        assert_eq!(ended_line(&waits, "exit 0", 175), "`cargo test` ended (exit 0) after 45s");
    }

    #[test]
    fn a_status_command_is_one_line_and_names_the_program() {
        let gate = "afkconvoy exclusive -- ./gate.sh";
        assert_eq!(status_command(gate), gate);
        assert_eq!(status_command("cargo test"), "cargo test");
        assert_eq!(status_command("sh -c true"), "true");
        assert_eq!(status_command("sh -c touch '/tmp/ran'; sleep 30"), "touch /tmp/ran; sleep 30");
        assert_eq!(
            status_command("sh -c touch '/tmp/a ran'; sleep 30"),
            "touch '/tmp/a ran'; sleep 30"
        );

        let secret = "postgres://user:secret@db.example/app";
        let script = format!(
            "sh -c set -eu\n\
             # comment {secret}\n\
             export PATH=/usr/bin\n\
             export DATABASE_URL={secret}\n\
             cd /work\n\
             echo preparing\n\
             cargo build -p afkconvoy --quiet\n\
             cargo nextest run --workspace --test cli\n"
        );
        let named = status_command(&script);
        assert_eq!(named, "cargo nextest run --workspace --test cli");
        assert!(!named.contains('\n'), "{named}");
        assert!(!named.contains(secret), "{named}");
        assert!(!named.contains("export"), "{named}");

        let exclusive = format!(
            "bash -lc \nexport DATABASE_URL={secret}\nafkconvoy exclusive -- cargo nextest run --workspace\n"
        );
        assert_eq!(
            status_command(&exclusive),
            "afkconvoy exclusive -- cargo nextest run --workspace"
        );
        assert!(!status_command(&exclusive).contains(secret));

        let prefixed = format!("DATABASE_URL={secret} cargo nextest run --workspace");
        assert_eq!(status_command(&prefixed), "cargo nextest run --workspace");
        assert_eq!(
            status_command("/bin/bash -lc cargo nextest run -p afkconvoy"),
            "cargo nextest run -p afkconvoy"
        );
        let only_env = format!("sh -c export DATABASE_URL={secret}");
        assert_eq!(status_command(&only_env), "sh -c");
        assert!(!status_command(&only_env).contains(secret));
    }

    #[test]
    fn a_snapshot_names_the_program_and_keeps_the_scheduling() {
        let mut runs = ticket(1, "/a", 100);
        runs.ran = Some(100);
        runs.command = "afkconvoy exclusive -- ./gate.sh".into();
        let mut waits = ticket(2, "/a", 110);
        waits.command = "sh -c export DATABASE_URL=postgres://secret\ncargo nextest run\n".into();
        let seen = snapshot(&[runs, waits], ONE, false, 150);
        assert_eq!(seen.running[0].ticket.command, "afkconvoy exclusive -- ./gate.sh");
        assert_eq!(seen.waiting[0].ticket.command, "cargo nextest run");
        assert_eq!(seen.waiting[0].scheduling.state, State::Waiting);
        assert_eq!(seen.running[0].elapsed_seconds, Some(50));
        assert!(!seen.waiting[0].ticket.command.contains("postgres"));
    }

    fn sorted(pids: BTreeSet<u32>) -> Vec<u32> {
        pids.into_iter().collect()
    }

    #[test]
    fn a_tickets_work_is_its_process_and_every_one_under_it() {
        // a worker's harness 10, its gate 11, the gate's `afkconvoy exclusive`
        // 12 running cargo 13 and rustc 14; 20 is another of its commands
        let table = [(1, 0), (10, 1), (11, 10), (12, 11), (13, 12), (14, 13), (20, 10)];
        assert_eq!(sorted(under(&[12], &table)), [12, 13, 14]);
        assert_eq!(sorted(under(&[13, 20], &table)), [13, 14, 20]);
        assert!(under(&[], &table).is_empty());
        // a root the reading does not hold is still its own work
        assert_eq!(sorted(under(&[99], &table)), [99]);
    }

    #[test]
    fn a_child_handed_to_another_parent_is_under_that_one_alone() {
        // 13's parent 12 ended: 13 was handed to pid 1, with 14 under it
        let table = [(1, 0), (11, 1), (13, 1), (14, 13), (15, 11)];
        assert_eq!(sorted(under(&[11], &table)), [11, 15]);
        assert_eq!(sorted(under(&[13], &table)), [13, 14]);
    }

    #[test]
    fn a_pid_used_again_after_its_process_ended_is_under_what_started_the_new_one() {
        // 13 ran under the ticket's 12 and ended; its pid now names a process
        // a shell, 30, started; and the reading caught a loop of parents as
        // 40 and 41 ended and started
        let table = [(12, 1), (30, 1), (13, 30), (40, 41), (41, 40), (42, 41), (43, 12)];
        assert_eq!(sorted(under(&[12], &table)), [12, 43]);
        assert_eq!(sorted(under(&[40], &table)), [40, 41, 42]);
    }

    #[test]
    fn the_running_work_is_that_of_the_running_tickets_alone() {
        let mut runs = ticket(1, "/a", 100);
        (runs.pid, runs.ran) = (12, Some(100));
        let mut waits = ticket(2, "/b", 110);
        waits.pid = 20;
        assert!(runs.running() && !waits.running());
        let table = [(10, 1), (12, 10), (13, 12), (20, 10), (21, 20)];
        let work = running_work(&[runs.clone(), waits], &table);
        assert_eq!((sorted(work.pids), work.whole), (vec![12, 13], true));
        // a running ticket whose process left before the reading was taken
        let mut gone = ticket(3, "/c", 120);
        (gone.pid, gone.ran) = (50, Some(120));
        let work = running_work(&[runs, gone], &table);
        assert_eq!((sorted(work.pids), work.whole), (vec![12, 13, 50], false));
        assert_eq!(running_work(&[], &table), Work { pids: BTreeSet::new(), whole: true });
    }
}
