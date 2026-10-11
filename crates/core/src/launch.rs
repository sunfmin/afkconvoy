//! The worker launch command: the one opaque shell string every worker is
//! started with. A worker's shell inherits nothing of the environment the
//! fleet was started in, so a fleet started through a provider wrapper must be
//! told the command that wraps its workers the same way — or it would start
//! them on stock Claude, silently.
//!
//! Nothing here reads the command beyond its first word, and nothing builds
//! one from parts.

use flux_rs::attrs::*;

/// What a worker is started with when the fleet is on no custom provider.
pub const STOCK: &str = "claude --dangerously-skip-permissions";

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

/// Where a start gets its command: `provider` is the `ANTHROPIC_BASE_URL` the
/// fleet itself runs under, `supplied` what `--worker-command` was given, and
/// `terminal` whether there is a human to ask.
pub fn source(provider: Option<&str>, supplied: Option<&str>, terminal: bool) -> Source {
    if let Some(supplied) = supplied {
        return Source::Given(supplied.to_string());
    }
    let Some(provider) = provider.filter(|url| !url.trim().is_empty()) else {
        return Source::Given(STOCK.to_string());
    };
    if terminal {
        Source::Ask
    } else {
        Source::Refuse(format!(
            "this fleet runs on a custom provider ({provider}) and a worker's shell inherits \
             none of it: say what starts a worker with --worker-command — there is no \
             terminal to ask on"
        ))
    }
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
        || word == "claude";
    if visible {
        Some(true)
    } else if whole {
        Some(false)
    } else {
        None
    }
}

/// A login shell's alias that starts Claude: what a human is offered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub name: String,
    /// What the alias stands for. Shown, never run.
    pub expansion: String,
}

/// The aliases a shell's `alias` printed that start Claude, by name. Reads
/// zsh's `name='value'` and bash's `alias name='value'`.
pub fn candidates(aliases: &str) -> Vec<Candidate> {
    let mut found: Vec<Candidate> = vec![];
    for line in aliases.lines() {
        let line = line.trim();
        let line = line.strip_prefix("alias ").unwrap_or(line);
        let Some((name, value)) = line.split_once('=') else { continue };
        if name.is_empty() || name.contains(char::is_whitespace) {
            continue;
        }
        let expansion = unquoted(value.trim());
        if expansion.contains("claude") {
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

/// What the human is asked, with what they can pick from.
pub fn question(provider: &str, candidates: &[Candidate]) -> String {
    let mut text = format!(
        "This fleet runs on a custom provider ({provider}); a worker's shell inherits none of \
         it.\n"
    );
    if candidates.is_empty() {
        text.push_str("The login shell has no alias that starts Claude.\n");
    } else {
        text.push_str("The login shell's aliases that start Claude:\n");
        for (index, candidate) in candidates.iter().enumerate() {
            text.push_str(&format!(
                "  {}. {} = {}\n",
                index.saturating_add(1),
                candidate.name,
                candidate.expansion
            ));
        }
    }
    text.push_str("Start every worker with (a number above, or a command): ");
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
}

#[cfg(test)]
mod tests {
    use super::*;

    const KIM: &str = "kim is an alias for ANTHROPIC_BASE_URL=https://kimi claude --yolo";

    #[test]
    fn with_no_custom_provider_the_stock_command_is_used_and_nobody_is_asked() {
        let stock = Source::Given(STOCK.to_string());
        assert_eq!(source(None, None, true), stock);
        assert_eq!(source(None, None, false), stock);
        assert_eq!(source(Some("  "), None, true), stock, "a blank url is no provider");
    }

    #[test]
    fn what_was_passed_is_the_answer_as_it_was_passed() {
        let passed = "  kim  --model 'big one' ; true ";
        for provider in [None, Some("https://kimi")] {
            for terminal in [true, false] {
                assert_eq!(
                    source(provider, Some(passed), terminal),
                    Source::Given(passed.to_string())
                );
            }
        }
    }

    #[test]
    fn a_custom_provider_and_no_answer_is_asked_about_or_is_an_error() {
        assert_eq!(source(Some("https://kimi"), None, true), Source::Ask);
        match source(Some("https://kimi"), None, false) {
            Source::Refuse(why) => {
                assert!(why.contains("--worker-command") && why.contains("https://kimi"), "{why}");
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
        let found = candidates(zsh);
        let names: Vec<&str> = found.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["cc", "kim"]);
        assert_eq!(found[1].expansion, "ANTHROPIC_BASE_URL=https://kimi claude --yolo");
        let bash = "alias glm=\"claude --model glm\"\nalias g='git'\n";
        assert_eq!(
            candidates(bash),
            [Candidate { name: "glm".into(), expansion: "claude --model glm".into() }]
        );
        assert_eq!(candidates(""), []);
    }

    #[test]
    fn a_reply_is_a_pick_or_the_command_as_typed() {
        let offered = candidates("kim='claude --yolo'\ncc=claude\n");
        let asked = question("https://kimi", &offered);
        assert!(asked.contains("  1. cc = claude\n  2. kim = claude --yolo\n"), "{asked}");
        assert!(asked.contains("https://kimi"), "{asked}");
        assert_eq!(replied("2\n", &offered), Some("kim".to_string()));
        assert_eq!(replied(" 1 \r\n", &offered), Some("cc".to_string()));
        assert_eq!(replied("glm --yolo  \n", &offered), Some("glm --yolo  ".to_string()));
        assert_eq!(replied("7\n", &offered), Some("7".to_string()), "no such pick: a command");
        assert_eq!(replied("0\n", &offered), Some("0".to_string()));
        assert_eq!(replied("\n", &offered), None);
        assert_eq!(replied("", &offered), None);
        assert!(question("u", &[]).contains("no alias that starts Claude"));
    }
}
