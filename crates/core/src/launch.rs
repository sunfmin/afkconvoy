//! The worker launch command: the one opaque shell string every worker of
//! one harness is started with. A fleet instance has one for each harness its
//! workers run (docs/adr/0013): `--worker-command` says how one harness is
//! started on this host (`supplied`), and a harness nobody said one for is
//! started with its stock command. A worker's shell inherits nothing of the
//! environment the fleet was started in, so a fleet started through a
//! provider wrapper must be told the command that wraps its workers the same
//! way — or it would start them on the stock one, silently. The stock
//! command, and the variable that names a provider, are each harness's own
//! (`harness`): an unbound repo's command starts whatever it starts.
//!
//! Nothing here reads the command beyond its first word, and nothing builds
//! one from parts.

use flux_rs::attrs::*;

use crate::harness::Harness;

/// The flags that let an agent work with nobody at its terminal. Only ever
/// looked for, to warn: a worker without one stops at its first permission
/// prompt, which the fleet cannot tell from a worker that finished.
const UNATTENDED: [&str; 8] = [
    "--dangerously-skip-permissions",
    "--dangerously-bypass-approvals-and-sandbox",
    "--yolo",
    "--yes-always",
    "--dangerously-allow-all",
    "--trust-all-tools",
    "--unrestricted",
    "--auto-approve",
];

/// The word whose resolving decides whether the command can run at all.
pub fn first_word(command: &str) -> Option<&str> {
    command.split_whitespace().next()
}

/// Where the command comes from, at a start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Use this one: what was passed, as it was passed, or the stock command.
    Given(String),
    /// A custom provider and nobody said: ask at the terminal.
    Ask,
    /// A custom provider, nobody said, and nobody to ask: the error.
    Refuse(String),
}

/// What `--worker-command` said, each time it was given: the command of each
/// harness it names, no harness twice. `supplied` is the only way to one.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Supplied {
    commands: Vec<(Harness, String)>,
}

impl Supplied {
    /// The command said for `harness`, as it was said; `None` when none was.
    pub fn of(&self, harness: Harness) -> Option<&str> {
        let said = self.commands.iter().find(|(named, _)| *named == harness);
        said.map(|(_, command)| command.as_str())
    }

    /// Each harness a command was said for, with that command, in the order
    /// they were given.
    pub fn each(&self) -> impl Iterator<Item = (Harness, &str)> {
        self.commands.iter().map(|(harness, command)| (*harness, command.as_str()))
    }
}

/// What the values `given` to `--worker-command` say, in a repo whose
/// default harness is `default`. A value whose first word holds an `=` is
/// `<harness>=<command>`: what is before the `=` is the harness's key, and
/// what follows is its command, as it was given. Any other value is the bare
/// form: the command of `default`. An error is a name that is no harness, or
/// a harness given a command twice — by name, bare, or once each way.
pub fn supplied(given: &[String], default: Harness) -> Result<Supplied, String> {
    let mut commands: Vec<(Harness, String)> = vec![];
    for value in given {
        let named = value.split_once('=').filter(|(name, _)| !name.contains(char::is_whitespace));
        let (harness, command) = match named {
            Some((name, command)) => match Harness::read(name) {
                Some(harness) => (harness, command),
                None => {
                    return Err(format!(
                        "--worker-command {value:?}: {}. A harness's command is given as \
                         <harness>=<command>, and the default harness's as the command alone",
                        Harness::unknown(name)
                    ));
                }
            },
            None => (default, value.as_str()),
        };
        if commands.iter().any(|(said, _)| *said == harness) {
            return Err(format!(
                "--worker-command was given twice for the harness `{}`: a harness has one \
                 worker launch command — and the bare form is that of the default harness, `{}`",
                harness.key(),
                default.key()
            ));
        }
        commands.push((harness, command.to_string()));
    }
    Ok(Supplied { commands })
}

/// Where a start gets the command of `harness`: `provider` is the custom
/// provider the fleet itself runs under (`Harness::provider_variable`),
/// `supplied` what `--worker-command` said for it, and `terminal` whether
/// there is a human to ask.
pub fn source(
    harness: Harness,
    provider: Option<&str>,
    supplied: Option<&str>,
    terminal: bool,
) -> Source {
    if let Some(supplied) = supplied {
        return Source::Given(supplied.to_string());
    }
    let Some(provider) = provider.filter(|url| !url.trim().is_empty()) else {
        return Source::Given(harness.stock().to_string());
    };
    if terminal {
        Source::Ask
    } else {
        Source::Refuse(format!(
            "this fleet runs on a custom provider ({provider}) and a worker's shell inherits \
             none of it: say what starts a {} worker with `--worker-command {}=<command>` — \
             there is no terminal to ask on",
            harness.name(),
            harness.key()
        ))
    }
}

/// `fault`, of the worker launch command of `harness`, said with the harness
/// and what calls for it (`Team::harnesses`): the stage or the patrol a human
/// changes, or the command they give, to be rid of it.
pub fn of_harness(fault: &str, harness: Harness, callers: &[String]) -> String {
    format!(
        "{fault} — it is the worker launch command of the harness `{}`, called for by {}",
        harness.key(),
        callers.join(", ")
    )
}

/// A command a worker may be started with: its first word resolves in the
/// shell a worker gets. `checked` is the only way to one, so no worker is
/// started on a command nobody resolved.
#[derive(Debug, PartialEq, Eq)]
#[refined_by(resolved: bool)]
#[invariant(resolved)]
pub struct LaunchCommand {
    command: String,
    #[field(bool[resolved])]
    // What the invariant is stated on: nothing reads it back.
    #[allow(dead_code)]
    resolved: bool,
    warning: Option<String>,
}

impl LaunchCommand {
    /// The command, exactly as it was given.
    pub fn as_str(&self) -> &str {
        &self.command
    }

    /// What the human should know about it, though it will be used.
    pub fn warning(&self) -> Option<&str> {
        self.warning.as_deref()
    }
}

/// `command` as one a worker may be started with, given what the worker's
/// shell said its first word is (`resolution`: `type`'s answer, `None` when
/// it found nothing). An error names the word that resolves to nothing: used
/// anyway it starts no worker, and each claim would be escalated for it.
pub fn checked(command: &str, resolution: Option<&str>) -> Result<LaunchCommand, String> {
    let Some(word) = first_word(command) else {
        return Err("the worker launch command is empty".to_string());
    };
    let Some(resolution) = resolution.map(str::trim).filter(|text| !text.is_empty()) else {
        return Err(format!(
            "the worker launch command {command:?} cannot start a worker: {word:?} resolves to \
             nothing in a login shell"
        ));
    };
    let warning = match unattended(command, word, resolution) {
        Some(false) => Some(format!(
            "the worker launch command {command:?} shows no unattended flag ({resolution}): a \
             worker that stops at a permission prompt looks finished to the fleet"
        )),
        _ => None,
    };
    Ok(LaunchCommand { command: command.to_string(), resolved: true, warning })
}

/// Whether an unattended flag is visible: yes; plainly not — the resolution
/// shows the whole of what runs; or unknown (`None`) — a script's path, a
/// function, could carry one inside.
fn unattended(command: &str, word: &str, resolution: &str) -> Option<bool> {
    let mut visible = false;
    for flag in UNATTENDED {
        if command.contains(flag) || resolution.contains(flag) {
            visible = true;
        }
    }
    let whole = resolution.contains(" is an alias for ")
        || resolution.contains(" is aliased to ")
        || word == Harness::DEFAULT.program();
    if visible {
        Some(true)
    } else if whole {
        Some(false)
    } else {
        None
    }
}

/// A login shell's alias that starts a harness: what a human is offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    /// What the alias stands for. Shown, never run.
    pub expansion: String,
}

/// The aliases a shell's `alias` printed that start `harness`, by name. Reads
/// zsh's `name='value'` and bash's `alias name='value'`.
pub fn candidates(aliases: &str, harness: Harness) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = vec![];
    for line in aliases.lines() {
        let line = line.trim();
        let line = line.strip_prefix("alias ").unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else { continue };
        if name.is_empty() || name.contains(char::is_whitespace) {
            continue;
        }
        let expansion = unquoted(value.trim());
        if expansion.contains(harness.program()) {
            found.push(Candidate { name: name.to_string(), expansion: expansion.to_string() });
        }
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found
}

fn unquoted(value: &str) -> &str {
    for quote in ['\'', '"'] {
        if let Some(inner) = value.strip_prefix(quote).and_then(|rest| rest.strip_suffix(quote)) {
            return inner;
        }
    }
    value
}

/// What the human is asked of the command of `harness`, with what they can
/// pick from.
pub fn question(provider: &str, harness: Harness, candidates: &[Candidate]) -> String {
    let name = harness.name();
    let mut text = format!(
        "This fleet runs on a custom provider ({provider}); a worker's shell inherits none of \
         it.\n"
    );
    if candidates.is_empty() {
        text.push_str(&format!("The login shell has no alias that starts {name}.\n"));
    } else {
        text.push_str(&format!("The login shell's aliases that start {name}:\n"));
        for (index, candidate) in candidates.iter().enumerate() {
            text.push_str(&format!(
                "  {}. {} = {}\n",
                index.saturating_add(1),
                candidate.name,
                candidate.expansion
            ));
        }
    }
    text.push_str(&format!("Start every {name} worker with (a number above, or a command): "));
    text
}

/// The command a human's reply stands for: a number picks that alias, by its
/// name; anything else is the command, as typed. `None` for no reply.
pub fn replied(reply: &str, candidates: &[Candidate]) -> Option<String> {
    let reply = reply.trim_end_matches(['\n', '\r']);
    if reply.trim().is_empty() {
        return None;
    }
    let picked = reply.trim().parse::<usize>().ok().and_then(|n| n.checked_sub(1));
    match picked.and_then(|index| candidates.get(index)) {
        Some(candidate) => Some(candidate.name.clone()),
        None => Some(reply.to_string()),
    }
}

/// Why a start stops when the human gave no reply.
pub fn no_reply() -> String {
    "no worker launch command was given: nothing was started".to_string()
}

/// What a wait on a freshly started worker came back with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Waited {
    /// Ready for a prompt.
    Ready,
    /// Asking whether the folder it was started in is trusted: every key it
    /// is sent goes to that question, not to the agent.
    AtTrustPrompt,
    NotReady,
}

/// The trust question of one start: whether it has been answered.
#[refined_by(answered: bool)]
struct Trust {
    #[field(bool[answered])]
    answered: bool,
}

impl Trust {
    #[spec(fn() -> Trust[false])]
    fn unanswered() -> Trust {
        Trust { answered: false }
    }

    /// Answers the question — once: the keys that answer it, typed again, go
    /// to whatever came after it.
    #[spec(fn(self: &mut Trust[false], answer: A) -> Result<(), E> ensures self: Trust[true])]
    fn answer<E, A: FnOnce() -> Result<(), E>>(&mut self, answer: A) -> Result<(), E> {
        self.answered = true;
        answer()
    }
}

/// Waits for a freshly started worker to be ready for a prompt, answering the
/// trust question when it asks: the fleet only starts workers in worktrees of
/// the repo it was itself started in → what the worker was at last. A worker
/// still `AtTrustPrompt` then was answered and asks again.
pub fn until_ready<E, W, A>(mut wait: W, answer: A) -> Result<Waited, E>
where
    W: FnMut() -> Result<Waited, E>,
    A: FnOnce() -> Result<(), E>,
{
    let mut trust = Trust::unanswered();
    match wait()? {
        Waited::AtTrustPrompt => {
            trust.answer(answer)?;
            wait()
        }
        waited => Ok(waited),
    }
}

#[cfg(flux)]
#[allow(dead_code)]
mod rejected {
    use super::*;

    /// A worker started on a command whose first word nobody resolved: the
    /// typo that burns every claim it is tried on.
    #[should_fail]
    fn a_command_nobody_resolved(command: String, resolution: Option<&str>) -> LaunchCommand {
        LaunchCommand { command, resolved: resolution.is_some(), warning: None }
    }

    /// A command known to resolve to nothing, used anyway.
    #[should_fail]
    fn an_unresolved_command_used(command: String) -> LaunchCommand {
        LaunchCommand { command, resolved: false, warning: None }
    }

    /// The trust question answered twice: the second answer's keys land in
    /// the agent's own prompt.
    #[should_fail]
    fn a_trust_question_answered_twice<
        A: FnOnce() -> Result<(), ()>,
        B: FnOnce() -> Result<(), ()>,
    >(
        first: A,
        second: B,
    ) {
        let mut trust = Trust::unanswered();
        let _ = trust.answer(first);
        let _ = trust.answer(second);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn waits(
        mut answers: Vec<Waited>,
        fails: bool,
    ) -> (Result<Waited, &'static str>, usize, usize) {
        answers.reverse();
        let (mut waited, mut answered) = (0, 0);
        let last = until_ready(
            || {
                waited += 1;
                Ok(answers.pop().expect("a wait nobody expected"))
            },
            || {
                answered += 1;
                if fails { Err("refused") } else { Ok(()) }
            },
        );
        (last, waited, answered)
    }

    #[test]
    fn a_worker_that_is_ready_is_asked_nothing() {
        assert_eq!(waits(vec![Waited::Ready], false), (Ok(Waited::Ready), 1, 0));
        assert_eq!(waits(vec![Waited::NotReady], false), (Ok(Waited::NotReady), 1, 0));
    }

    #[test]
    fn a_trust_question_is_answered_once_and_the_worker_waited_for_again() {
        let asked = vec![Waited::AtTrustPrompt, Waited::Ready];
        assert_eq!(waits(asked, false), (Ok(Waited::Ready), 2, 1));
        let stuck = vec![Waited::AtTrustPrompt, Waited::AtTrustPrompt];
        assert_eq!(waits(stuck, false), (Ok(Waited::AtTrustPrompt), 2, 1));
    }

    #[test]
    fn an_answer_that_fails_ends_the_wait() {
        assert_eq!(waits(vec![Waited::AtTrustPrompt], true), (Err("refused"), 1, 1));
    }

    const CLAUDE: Harness = Harness::ClaudeCode;
    const STOCK: &str = CLAUDE.stock();

    fn given(values: &[&str]) -> Result<Supplied, String> {
        let values: Vec<String> = values.iter().map(|value| value.to_string()).collect();
        supplied(&values, CLAUDE)
    }

    #[test]
    fn a_harness_s_command_is_given_by_its_key_and_the_default_s_bare() {
        assert_eq!(given(&[]).unwrap().of(CLAUDE), None, "nobody said: the stock one");
        // What follows the `=` is the command, as it was given.
        let named = given(&["claude-code=kim  --model=k 'big one' "]).unwrap();
        assert_eq!(named.of(CLAUDE), Some("kim  --model=k 'big one' "));
        // An `=` past the first word is the command's own.
        for bare in ["kim", "  kim --yolo ", "kim --model=k", "kim x=y"] {
            assert_eq!(given(&[bare]).unwrap().of(CLAUDE), Some(bare));
        }
        let each: Vec<(Harness, &str)> = named.each().collect();
        assert_eq!(each, [(CLAUDE, "kim  --model=k 'big one' ")]);
    }

    #[test]
    fn a_name_that_is_no_harness_and_a_harness_given_twice_are_errors() {
        for none in ["codex=codex", "Claude-Code=kim", "=kim", "ANTHROPIC_BASE_URL=x claude"] {
            let error = given(&[none]).unwrap_err();
            assert!(error.contains("is no harness; the harnesses are: claude-code, pi"), "{error}");
            assert!(error.contains("<harness>=<command>"), "{error}");
        }
        for twice in [
            ["claude-code=kim", "claude-code=glm"],
            ["kim", "glm"],
            ["kim", "claude-code=kim"],
            ["claude-code=kim", "kim"],
        ] {
            let error = given(&twice).unwrap_err();
            assert!(error.contains("given twice for the harness `claude-code`"), "{error}");
        }
    }

    #[test]
    fn a_fault_is_said_with_its_harness_and_what_calls_for_it() {
        let callers = ["the repo's default".to_string(), "the patrol `sweep`".to_string()];
        assert_eq!(
            of_harness("it resolves to nothing", CLAUDE, &callers),
            "it resolves to nothing — it is the worker launch command of the harness \
             `claude-code`, called for by the repo's default, the patrol `sweep`"
        );
    }

    const KIM: &str = "kim is an alias for ANTHROPIC_BASE_URL=https://kimi claude --yolo";

    #[test]
    fn with_no_custom_provider_the_stock_command_is_used_and_nobody_is_asked() {
        let stock = Source::Given(STOCK.to_string());
        assert_eq!(source(CLAUDE, None, None, true), stock);
        assert_eq!(source(CLAUDE, None, None, false), stock);
        assert_eq!(source(CLAUDE, Some("  "), None, true), stock, "a blank url is no provider");
    }

    #[test]
    fn what_was_passed_is_the_answer_as_it_was_passed() {
        let passed = "  kim  --model 'big one' ; true ";
        for provider in [None, Some("https://kimi")] {
            for terminal in [true, false] {
                assert_eq!(
                    source(CLAUDE, provider, Some(passed), terminal),
                    Source::Given(passed.to_string())
                );
            }
        }
    }

    #[test]
    fn a_custom_provider_and_no_answer_is_asked_about_or_is_an_error() {
        assert_eq!(source(CLAUDE, Some("https://kimi"), None, true), Source::Ask);
        match source(CLAUDE, Some("https://kimi"), None, false) {
            Source::Refuse(why) => {
                assert!(why.contains("--worker-command") && why.contains("https://kimi"), "{why}");
                assert!(why.contains("`--worker-command claude-code=<command>`"), "{why}");
                assert!(why.contains("no terminal"), "{why}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_command_that_resolves_is_kept_exactly() {
        let passed = "  kim  --model 'big one' ; true ";
        let command = checked(passed, Some(KIM)).unwrap();
        assert_eq!(command.as_str(), passed);
        assert_eq!(command.warning(), None);
        assert_eq!(first_word(passed), Some("kim"));
    }

    #[test]
    fn a_command_whose_first_word_resolves_to_nothing_is_refused() {
        for nothing in [None, Some(""), Some("  \n")] {
            let error = checked("ckim --yolo", nothing).unwrap_err();
            assert!(error.contains("\"ckim\" resolves to nothing"), "{error}");
        }
        assert!(checked("   ", Some("x")).unwrap_err().contains("empty"));
        assert!(checked("", None).unwrap_err().contains("empty"));
    }

    #[test]
    fn a_missing_unattended_flag_is_warned_about_only_when_it_is_plainly_missing() {
        let warned = |command: &str, resolution: &str| {
            checked(command, Some(resolution)).unwrap().warning().map(str::to_string)
        };
        // an alias shows all of what runs
        let plain = warned("kim", "kim is an alias for claude --model x").unwrap();
        assert!(plain.contains("no unattended flag") && plain.contains("\"kim\""), "{plain}");
        assert!(warned("kim", "kim is aliased to `claude'").is_some(), "bash's wording");
        // …and so does `claude` itself
        assert!(warned("claude", "claude is /usr/local/bin/claude").is_some());
        // a flag in the alias, or on the command, is a flag
        assert_eq!(warned("kim", KIM), None);
        assert_eq!(
            warned("kim --dangerously-skip-permissions", "kim is an alias for claude"),
            None
        );
        assert_eq!(warned(STOCK, "claude is /usr/local/bin/claude"), None);
        // a script could carry one inside: unknown is not a warning
        assert_eq!(warned("agent", "agent is /usr/local/bin/agent"), None);
        assert_eq!(warned("kim", "kim is a shell function from /home/u/.zshrc"), None);
    }

    #[test]
    fn the_aliases_that_start_claude_are_offered_by_name() {
        let zsh = "ll='ls -l'\nkim='ANTHROPIC_BASE_URL=https://kimi claude --yolo'\ncc=claude\n\
                   weird line\n=nothing\n";
        let found = candidates(zsh, CLAUDE);
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["cc", "kim"]);
        assert_eq!(found[1].expansion, "ANTHROPIC_BASE_URL=https://kimi claude --yolo");
        let bash = "alias glm=\"claude --model glm\"\nalias g='git'\n";
        assert_eq!(
            candidates(bash, CLAUDE),
            [Candidate { name: "glm".into(), expansion: "claude --model glm".into() }]
        );
        assert_eq!(candidates("", CLAUDE), []);
    }

    #[test]
    fn a_reply_is_a_pick_or_the_command_as_typed() {
        let offered = candidates("kim='claude --yolo'\ncc=claude\n", CLAUDE);
        let asked = question("https://kimi", CLAUDE, &offered);
        assert!(asked.contains("  1. cc = claude\n  2. kim = claude --yolo\n"), "{asked}");
        assert!(asked.contains("https://kimi"), "{asked}");
        assert_eq!(replied("2\n", &offered), Some("kim".to_string()));
        assert_eq!(replied(" 1 \r\n", &offered), Some("cc".to_string()));
        assert_eq!(replied("glm --yolo  \n", &offered), Some("glm --yolo  ".to_string()));
        assert_eq!(replied("7\n", &offered), Some("7".to_string()), "no such pick: a command");
        assert_eq!(replied("0\n", &offered), Some("0".to_string()));
        assert_eq!(replied("\n", &offered), None);
        assert_eq!(replied("", &offered), None);
        assert!(question("u", CLAUDE, &[]).contains("no alias that starts Claude Code"));
    }
}
