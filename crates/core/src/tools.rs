//! The commands a fleet cannot work without, and what a human runs when one
//! is missing, logged out, not running or does not know this checkout. A start that lacks any refuses, naming every
//! fault with the line that fixes it; the tool itself never installs anything
//! and logs nobody in.

use crate::harness::{Harness, grok_not_authenticated};
use crate::launch::first_word;
use crate::line::quoted;

/// A command the fleet, or a worker it starts, runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Git,
    Gh,
    Orca,
    /// Claude Code: needed only when it is what starts a worker.
    Claude,
    /// pi: needed only when it is what starts a worker.
    Pi,
    /// Codex: needed only when it is what starts a worker.
    Codex,
    DshTui,
    Dsh,
    /// Kimi Code: needed only when it is what starts a worker.
    Kimi,
    /// Grok: needed only when it is what starts a worker.
    Grok,
}

/// What pi lists in place of its models when it holds credentials for no
/// provider.
const PI_NO_MODELS: &str = "No models available";

/// What asks whether Codex holds a login a worker can use: the file a worker
/// is given a link to, and that Codex takes what it holds for a login. A
/// login Codex keeps in a keyring is in no file, and is no worker's.
const CODEX_LOGIN: &str = "test -f \"$HOME/.codex/auth.json\" && codex login status";

/// The last answer to a login question, kept for a finding without asking again.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LoginAnswer {
    /// Whether the shell command exited zero; `None` on an old probe note.
    succeeded: Option<bool>,
    /// Its first 500 characters, with credential-shaped text redacted.
    printed: String,
}

impl LoginAnswer {
    pub fn new(succeeded: bool, printed: &str) -> Self {
        let printed = redact_credentials(printed).chars().take(500).collect();
        Self { succeeded: Some(succeeded), printed }
    }

    /// The exit status and safe excerpt of what the harness printed.
    pub fn evidence(&self) -> String {
        let status = match self.succeeded {
            Some(true) => "0",
            Some(false) => "non-zero",
            None => "not recorded",
        };
        let printed = if self.printed.is_empty() { "(nothing printed)" } else { &self.printed };
        format!("exit status: {status}\nprinted:\n{printed}")
    }
}

fn redact_credentials(printed: &str) -> String {
    let mut clean = printed.to_string();
    for label in [
        "token",
        "secret",
        "password",
        "passwd",
        "api_key",
        "api-key",
        "apikey",
        "authorization",
        "credential",
        "private_key",
        "private-key",
        "access_key",
        "access-key",
        "session_key",
        "session-key",
        "key",
    ] {
        clean = redact_label(&clean, label);
    }
    for prefix in
        ["bearer ", "github_pat_", "ghp_", "gho_", "ghu_", "ghs_", "ghr_", "sk-", "xai-", "eyJ"]
    {
        clean = redact_prefix(&clean, prefix);
    }
    clean
}

fn redact_label(text: &str, label: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let mut ranges = vec![];
    for (start, _) in lower.match_indices(label) {
        let boundary =
            lower[..start].chars().next_back().is_none_or(|c| !c.is_ascii_alphanumeric());
        if !boundary {
            continue;
        }
        let after = &lower[start.saturating_add(label.len())..];
        let separator = start
            .saturating_add(label.len())
            .saturating_add(after.len())
            .saturating_sub(after.trim_start().len());
        if !matches!(lower[separator..].chars().next(), Some('=' | ':')) {
            continue;
        }
        let after = &lower[separator.saturating_add(1)..];
        let value = separator
            .saturating_add(1)
            .saturating_add(after.len())
            .saturating_sub(after.trim_start().len());
        let Some(first) = lower[value..].chars().next() else { continue };
        let (content, end) = if matches!(first, '\'' | '"') {
            let content = value.saturating_add(first.len_utf8());
            let end = lower[content..]
                .find(first)
                .map(|at| content.saturating_add(at))
                .unwrap_or(lower.len());
            (content, end)
        } else {
            let end = lower[value..]
                .char_indices()
                .find(|(_, c)| c.is_whitespace() || matches!(c, ',' | ';' | '}' | ']'))
                .map(|(at, _)| value.saturating_add(at))
                .unwrap_or(lower.len());
            (value, end)
        };
        if content < end {
            ranges.push((content, end));
        }
    }
    replace_ranges(text, ranges)
}

fn redact_prefix(text: &str, prefix: &str) -> String {
    let lower = text.to_ascii_lowercase();
    let prefix_lower = prefix.to_ascii_lowercase();
    let mut ranges = vec![];
    for (start, _) in lower.match_indices(&prefix_lower) {
        if lower[..start].chars().next_back().is_some_and(|c| c.is_ascii_alphanumeric()) {
            continue;
        }
        let content = start.saturating_add(prefix.len());
        let end = lower[content..]
            .char_indices()
            .find(|(_, c)| !(c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
            .map(|(at, _)| content.saturating_add(at))
            .unwrap_or(lower.len());
        if content < end {
            ranges.push((content, end));
        }
    }
    replace_ranges(text, ranges)
}

fn replace_ranges(text: &str, mut ranges: Vec<(usize, usize)>) -> String {
    let mut clean = text.to_string();
    ranges.sort_unstable_by_key(|(start, _)| std::cmp::Reverse(*start));
    for (start, end) in ranges {
        clean.replace_range(start..end, "[redacted]");
    }
    clean
}

/// What a start found of one tool: the first thing in its way. A missing one
/// is never also logged out, and one not running is asked nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Ready,
    Missing,
    LoggedOut(LoginAnswer),
    /// Installed, and not running: orca, which every call goes to as an app.
    NotRunning,
    /// Running, and it has no repo at this path, the checkout's main worktree:
    /// orca, which cuts a worktree only from a repo it was given.
    KnowsNoCheckout(String),
}

/// Where Orca's SSH remote runtime puts its CLI, under the home directory:
/// the desktop Orca on another machine installs it here, on the PATH only of
/// the terminals Orca opens itself. An `orca` on the PATH wins over it.
pub const ORCA_REMOTE_CLI: &str = ".orca/cli/bin";

/// The line that starts orca.
pub const START_ORCA: &str = "orca open";

/// The line that gives orca the checkout at `path`, once.
pub fn add_checkout(path: &str) -> String {
    format!("orca repo add --path {}", quoted(path))
}

impl Tool {
    /// The word it is run by.
    pub fn name(self) -> &'static str {
        match self {
            Tool::Git => "git",
            Tool::Gh => "gh",
            Tool::Orca => "orca",
            Tool::Claude => Harness::ClaudeCode.program(),
            Tool::Pi => Harness::Pi.program(),
            Tool::Codex => Harness::Codex.program(),
            Tool::DshTui => Harness::Dsh.program(),
            Tool::Dsh => "dsh",
            Tool::Kimi => Harness::Kimi.program(),
            Tool::Grok => Harness::Grok.program(),
        }
    }

    /// The tool the workers of `harness` are sessions of, when `launch`,
    /// their worker launch command, runs it by its own name: a harness is
    /// looked for only as that command's first word — what a wrapper runs
    /// and logs in to is its own business.
    pub fn of(harness: Harness, launch: &str) -> Option<Tool> {
        let tool = match harness {
            Harness::ClaudeCode => Tool::Claude,
            Harness::Pi => Tool::Pi,
            Harness::Codex => Tool::Codex,
            Harness::Dsh => Tool::DshTui,
            Harness::Kimi => Tool::Kimi,
            Harness::Grok => Tool::Grok,
        };
        (first_word(launch) == Some(tool.name())).then_some(tool)
    }

    /// Whether it is a harness: what a worker launch command starts.
    pub fn starts_workers(self) -> bool {
        matches!(
            self,
            Tool::Claude
                | Tool::Pi
                | Tool::Codex
                | Tool::DshTui
                | Tool::Dsh
                | Tool::Kimi
                | Tool::Grok
        )
    }

    /// The line that installs it. Homebrew's own `orca` cask is another
    /// program, so orca's names its tap, and Orca's SSH remote runtime, the
    /// other way to have orca, is named with where its CLI is looked for
    /// (`ORCA_REMOTE_CLI`); Claude Code's and pi's are their
    /// native installers, which keep them up to date.
    pub fn install(self) -> &'static str {
        match self {
            Tool::Git => "brew install git",
            Tool::Gh => "brew install gh",
            Tool::Orca => {
                "brew install --cask stablyai/orca/orca, or connect to this host from Orca over \
                 SSH, whose remote runtime puts its CLI at ~/.orca/cli/bin/orca"
            }
            Tool::Claude => "curl -fsSL https://claude.ai/install.sh | bash",
            Tool::Pi => "curl -fsSL https://pi.dev/install.sh | sh",
            Tool::Codex => "brew install --cask codex",
            Tool::DshTui => "npm install -g @deepseek-harness-tui/dsh-tui",
            Tool::Dsh => "install DeepSeek Harness (dsh) and add it to PATH",
            Tool::Kimi => "curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash",
            Tool::Grok => "curl -fsSL https://x.ai/cli/install.sh | bash",
        }
    }

    /// The shell line whose answer says whether it is logged in (`logged_in`
    /// reads it), `gh` for the `host` the repo's remote is on. `None`: it
    /// logs in to nothing.
    pub fn login_question(self, host: Option<&str>) -> Option<String> {
        match self {
            Tool::Git | Tool::Orca | Tool::DshTui | Tool::Dsh => None,
            Tool::Gh => Some(format!("gh auth status{}", hostname(host))),
            Tool::Claude => Some("claude auth status".to_string()),
            // pi logs in to providers, and lists the models of those it
            // holds credentials for: which one a worker uses is its stage's
            // to say.
            Tool::Pi => Some("pi --list-models".to_string()),
            Tool::Codex => Some(CODEX_LOGIN.to_string()),
            Tool::Kimi => Some("kimi provider list --json".to_string()),
            Tool::Grok => Some("grok models".to_string()),
        }
    }

    /// Whether the answer to `login_question` says it is logged in: `gh` by
    /// its exit status, Claude Code by the `loggedIn` field of the JSON it
    /// prints — whatever else the shell printed around it — pi by listing a
    /// model at all, Kimi by having a configured model, and Codex, as `gh`,
    /// by its exit status.
    pub fn logged_in(self, succeeded: bool, printed: &str) -> bool {
        match self {
            Tool::Claude => {
                let dense: String = printed.split_whitespace().collect();
                dense.contains("\"loggedIn\":true")
            }
            Tool::Pi => succeeded && !printed.trim().is_empty() && !printed.contains(PI_NO_MODELS),
            Tool::Kimi => succeeded && Harness::Kimi.models(printed).is_some(),
            Tool::Grok => {
                succeeded
                    && !grok_not_authenticated(printed)
                    && Harness::Grok.models(printed).is_some()
            }
            Tool::Git | Tool::Gh | Tool::Orca | Tool::Codex | Tool::DshTui | Tool::Dsh => succeeded,
        }
    }

    /// What it is said to be when it is not logged in. pi logs in to a
    /// provider, at its own prompt: its words say how, since no line does.
    /// Codex's say where a worker reads its login: one kept elsewhere is
    /// none to a worker.
    pub fn logged_out(self) -> &'static str {
        match self {
            Tool::Pi => "holds credentials for no provider (`/login` at its prompt logs it in)",
            Tool::Codex => {
                "is not logged in where a worker reads its login, `~/.codex/auth.json` (a login \
                 kept in a keyring is in no file)"
            }
            Tool::Kimi => "is not logged in or has no models configured",
            Tool::Grok => "is not logged in or listed no model",
            Tool::Git | Tool::Gh | Tool::Orca | Tool::Claude | Tool::DshTui | Tool::Dsh => {
                "is not logged in"
            }
        }
    }

    /// The line that logs it in — pi's starts it, and `/login` is typed at
    /// its prompt.
    pub fn login(self, host: Option<&str>) -> String {
        match self {
            Tool::Claude => "claude auth login".to_string(),
            Tool::Pi => "pi".to_string(),
            Tool::Codex => "codex login".to_string(),
            Tool::Kimi => "kimi login".to_string(),
            Tool::Grok => "grok login".to_string(),
            // github.com is where `gh auth login` logs in unasked.
            _ => format!("gh auth login{}", hostname(host.filter(|host| *host != "github.com"))),
        }
    }
}

fn hostname(host: Option<&str>) -> String {
    host.map(|host| format!(" --hostname {host}")).unwrap_or_default()
}

/// The tools every start needs, whatever starts its workers.
pub const ALWAYS: [Tool; 3] = [Tool::Git, Tool::Gh, Tool::Orca];

/// The host a remote's URL names, for `gh`: of `https://host/o/r`,
/// `ssh://git@host:22/o/r` and `git@host:o/r`. `None` for a path, and for
/// anything that is not plainly a host name — it goes into a shell line.
pub fn remote_host(url: &str) -> Option<&str> {
    remote_parts(url).map(|(host, _)| host)
}

/// The host a remote's URL names (`remote_host`), and the path after it as
/// written: `o/r` of each of the three. One reading of a remote's URL, for
/// `gh` and for which repo a checkout is of (`checkouts::repo_of`).
pub(crate) fn remote_parts(url: &str) -> Option<(&str, &str)> {
    let url = url.trim();
    let (authority, path) = match url.split_once("://") {
        Some((_, rest)) => rest.split_once('/').unwrap_or((rest, "")),
        None => url.split_once(':')?,
    };
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;
    let plain = host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    (plain && host.contains('.') && !host.starts_with('-')).then_some((host, path))
}

/// The line of a refusal that names what is in the way of `tool`, with the
/// line that fixes it; `None` when nothing is. `host` is the one `gh` was
/// asked about, and `callers` what calls for the harness the tool is, where
/// the start knows: a stage or a patrol that names it.
pub fn fault(tool: Tool, found: &Found, host: Option<&str>, callers: &[String]) -> Option<String> {
    let name = tool.name();
    let (what, line) = match found {
        Found::Ready => return None,
        Found::Missing => ("is not installed".to_string(), tool.install().to_string()),
        Found::LoggedOut(_) => (tool.logged_out().to_string(), tool.login(host)),
        Found::NotRunning => ("is not running".to_string(), START_ORCA.to_string()),
        Found::KnowsNoCheckout(path) => {
            ("does not know this checkout".to_string(), add_checkout(path))
        }
    };
    let called = match callers {
        [] => String::new(),
        callers => format!(", called for by {}", callers.join(", ")),
    };
    Some(format!("  {name} {what}{called}: {line}"))
}

/// Whether a harness in `found` is missing: that its worker launch command
/// cannot start a worker then says the same thing a second time.
pub fn harness_missing(found: &[(Tool, Found)]) -> bool {
    found.iter().any(|(tool, found)| tool.starts_workers() && *found == Found::Missing)
}

/// Why a start refuses, or `None` when it has all it needs: every line in
/// `faults` (`fault`), and `launch_fault` — why a worker launch command
/// cannot start a worker.
pub fn refused(faults: &[String], launch_fault: Option<&str>) -> Option<String> {
    if faults.is_empty() {
        return launch_fault.map(str::to_string);
    }
    let mut text = "this host cannot work a fleet yet: nothing was started. Run each line after \
                    a colon below, then start again — afkconvoy installs nothing and logs \
                    nobody in:\n"
        .to_string();
    text.push_str(&faults.join("\n"));
    if let Some(fault) = launch_fault {
        text.push_str("\nand ");
        text.push_str(fault);
    }
    Some(text)
}

/// Why a start refuses, or `None` when it has all it needs: every fault in
/// `found`, each with the line that fixes it, and `launch_fault` — why the
/// worker launch command cannot start a worker — unless that only says again
/// that its harness is missing. `host` is the one `gh` was asked about.
pub fn refusal(
    found: &[(Tool, Found)],
    host: Option<&str>,
    launch_fault: Option<&str>,
) -> Option<String> {
    let faults: Vec<String> =
        found.iter().filter_map(|(tool, found)| fault(*tool, found, host, &[])).collect();
    refused(&faults, launch_fault.filter(|_| !harness_missing(found)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const ALL_READY: [(Tool, Found); 4] = [
        (Tool::Git, Found::Ready),
        (Tool::Gh, Found::Ready),
        (Tool::Orca, Found::Ready),
        (Tool::Claude, Found::Ready),
    ];

    #[test]
    fn a_login_answer_keeps_a_bounded_excerpt_without_credentials() {
        let answer = LoginAnswer::new(
            false,
            "not authenticated\ntoken=abc123 apiKey: \"def456\" Bearer ghp_secret789\n",
        );
        let evidence = answer.evidence();
        assert!(evidence.contains("exit status: non-zero"), "{evidence}");
        assert!(evidence.contains("token=[redacted]"), "{evidence}");
        assert!(evidence.contains("apiKey: \"[redacted]\""), "{evidence}");
        assert!(evidence.contains("Bearer [redacted]"), "{evidence}");
        for secret in ["abc123", "def456", "ghp_secret789"] {
            assert!(!evidence.contains(secret), "{evidence}");
        }
        let long = LoginAnswer::new(true, &"x".repeat(600));
        assert_eq!(long.printed.chars().count(), 500);
    }

    #[test]
    fn a_harness_is_looked_for_only_as_the_first_word_of_its_launch_command() {
        let claude = |launch| Tool::of(Harness::ClaudeCode, launch);
        assert_eq!(claude("claude --dangerously-skip-permissions"), Some(Tool::Claude));
        assert_eq!(claude("  claude"), Some(Tool::Claude));
        // a wrapper, an alias that ends in claude, another agent, a path
        for other in ["kim --yolo", "wrap claude", "aider", "/usr/local/bin/claude", "claudex", ""]
        {
            assert_eq!(claude(other), None, "{other:?}");
        }
        assert_eq!(Tool::of(Harness::Pi, "pi --thinking high"), Some(Tool::Pi));
        for other in ["wrap pi", "pip install", "/opt/homebrew/bin/pi", "claude"] {
            assert_eq!(Tool::of(Harness::Pi, other), None, "{other:?}");
        }
        // …and only as its own harness's: pi starts no Claude Code worker
        assert_eq!(claude("pi"), None);
        let codex = |launch| Tool::of(Harness::Codex, launch);
        assert_eq!(codex("codex --dangerously-bypass-approvals-and-sandbox"), Some(Tool::Codex));
        for other in ["wrap codex", "codexx", "claude", "pi"] {
            assert_eq!(codex(other), None, "{other:?}");
        }
    }

    #[test]
    fn pi_holds_credentials_when_it_lists_a_model() {
        let pi = Tool::Pi;
        assert_eq!(pi.login_question(Some("github.com")).unwrap(), "pi --list-models");
        let listed = "provider      model    context\nopenai-codex  gpt-5.5  272K\n";
        assert!(pi.logged_in(true, listed));
        assert!(!pi.logged_in(false, listed));
        let none = "No models available. Use /login to log into a provider via OAuth or API key.";
        assert!(!pi.logged_in(true, none));
        assert!(!pi.logged_in(true, " \n"));
    }

    #[test]
    fn codex_is_logged_in_when_the_file_a_worker_is_linked_to_holds_a_login() {
        let codex = Tool::Codex;
        assert_eq!(
            codex.login_question(Some("github.com")).unwrap(),
            "test -f \"$HOME/.codex/auth.json\" && codex login status"
        );
        assert!(codex.logged_in(true, "Logged in using ChatGPT"));
        assert!(!codex.logged_in(false, ""));
        assert!(!codex.logged_in(false, "Not logged in"));
        assert_eq!(
            fault(codex, &Found::LoggedOut(LoginAnswer::default()), None, &[]).unwrap(),
            "  codex is not logged in where a worker reads its login, `~/.codex/auth.json` (a \
             login kept in a keyring is in no file): codex login"
        );
        assert_eq!(
            fault(codex, &Found::Missing, None, &[]).unwrap(),
            "  codex is not installed: brew install --cask codex"
        );
    }

    #[test]
    fn a_harness_a_stage_calls_for_is_named_with_the_stage_and_the_line_that_fixes_it() {
        let callers = ["the stage `build` of the workflow `feature`".to_string()];
        assert_eq!(
            fault(Tool::Pi, &Found::Missing, None, &callers).unwrap(),
            "  pi is not installed, called for by the stage `build` of the workflow `feature`: \
             curl -fsSL https://pi.dev/install.sh | sh"
        );
        assert_eq!(
            fault(Tool::Pi, &Found::LoggedOut(LoginAnswer::default()), Some("github.com"), &[])
                .unwrap(),
            "  pi holds credentials for no provider (`/login` at its prompt logs it in): pi"
        );
        assert_eq!(fault(Tool::Pi, &Found::Ready, None, &callers), None);
        // …under the words every refusal starts with, and beside a command's own fault
        let lines =
            [fault(Tool::Pi, &Found::LoggedOut(LoginAnswer::default()), None, &callers).unwrap()];
        let said = refused(&lines, Some("the command of another harness takes no flag")).unwrap();
        assert!(said.contains("nothing was started"), "{said}");
        assert!(
            said.contains(" of the workflow `feature`: pi\nand the command of another"),
            "{said}"
        );
        assert_eq!(refused(&[], None), None);
        // a missing pi is not said a second time as a command that resolves to nothing
        let said = refusal(&[(Tool::Pi, Found::Missing)], None, Some("pi resolves to nothing"));
        assert!(!said.unwrap().contains("resolves to nothing"));
    }

    #[test]
    fn a_host_with_everything_is_not_refused() {
        assert_eq!(refusal(&ALL_READY, None, None), None);
        assert_eq!(refusal(&[], Some("github.com"), None), None);
    }

    #[test]
    fn a_missing_orca_is_named_with_both_ways_to_have_it() {
        let install = Tool::Orca.install();
        assert!(install.starts_with("brew install --cask stablyai/orca/orca, "), "{install}");
        assert!(install.contains(&format!("~/{ORCA_REMOTE_CLI}/orca")), "{install}");
    }

    #[test]
    fn every_fault_is_named_with_the_line_that_fixes_it() {
        let found = [
            (Tool::Git, Found::Missing),
            (Tool::Gh, Found::LoggedOut(LoginAnswer::default())),
            (Tool::Orca, Found::Missing),
            (Tool::Claude, Found::LoggedOut(LoginAnswer::default())),
        ];
        let said = refusal(&found, Some("github.com"), None).unwrap();
        let lines: Vec<&str> = said.lines().skip(1).collect();
        assert_eq!(
            lines,
            [
                "  git is not installed: brew install git",
                "  gh is not logged in: gh auth login",
                "  orca is not installed: brew install --cask stablyai/orca/orca, or connect to \
                 this host from Orca over SSH, whose remote runtime puts its CLI at \
                 ~/.orca/cli/bin/orca",
                "  claude is not logged in: claude auth login",
            ]
        );
        assert!(said.contains("nothing was started"), "{said}");

        let missing = [(Tool::Gh, Found::Missing), (Tool::Claude, Found::Missing)];
        let said = refusal(&missing, None, None).unwrap();
        assert!(said.contains("  gh is not installed: brew install gh\n"), "{said}");
        assert!(
            said.ends_with(
                "  claude is not installed: curl -fsSL https://claude.ai/install.sh | bash"
            ),
            "{said}"
        );
        assert!(!said.contains("logged in:"), "missing is not also logged out: {said}");
    }

    #[test]
    fn an_orca_that_cannot_cut_a_worktree_here_is_named_with_the_line_to_run() {
        let said = refusal(&[(Tool::Orca, Found::NotRunning)], None, None).unwrap();
        assert!(said.ends_with("\n  orca is not running: orca open"), "{said}");
        assert!(said.contains("nothing was started"), "{said}");

        let unknown = Found::KnowsNoCheckout("/Users/kim/hospital demo".to_string());
        let said = refusal(
            &[(Tool::Gh, Found::LoggedOut(LoginAnswer::default())), (Tool::Orca, unknown)],
            None,
            None,
        );
        let said = said.unwrap();
        let lines: Vec<&str> = said.lines().skip(1).collect();
        assert_eq!(
            lines,
            [
                "  gh is not logged in: gh auth login",
                "  orca does not know this checkout: orca repo add --path '/Users/kim/hospital demo'",
            ]
        );
    }

    #[test]
    fn gh_is_asked_about_and_logged_in_to_the_host_the_remote_is_on() {
        assert_eq!(Tool::Gh.login_question(None).unwrap(), "gh auth status");
        assert_eq!(
            Tool::Gh.login_question(Some("ghe.example.com")).unwrap(),
            "gh auth status --hostname ghe.example.com"
        );
        let out = [(Tool::Gh, Found::LoggedOut(LoginAnswer::default()))];
        let said = refusal(&out, Some("ghe.example.com"), None).unwrap();
        assert!(said.ends_with(": gh auth login --hostname ghe.example.com"), "{said}");
        assert_eq!(Tool::Git.login_question(None), None);
        assert_eq!(Tool::Orca.login_question(None), None);
    }

    #[test]
    fn a_remote_url_names_its_host() {
        for (url, host) in [
            ("https://github.com/o/r.git", Some("github.com")),
            ("https://user@ghe.example.com:8443/o/r", Some("ghe.example.com")),
            ("ssh://git@github.com:22/o/r.git", Some("github.com")),
            ("git@github.com:o/r.git\n", Some("github.com")),
            ("/srv/git/remote.git", None),
            ("../remote.git", None),
            ("C:/repos/r", None),
            ("git@evil.com;rm -rf:o/r", None),
            ("https://$(id).example.com/o/r", None),
            ("https://-flag.example.com/o/r", None),
            ("", None),
        ] {
            assert_eq!(remote_host(url), host, "{url:?}");
        }
    }

    /// Any URL a remote can have, and any text in place of one: the parts of
    /// a URL and what does not belong in a host name, in any order, alone or
    /// where one of the three forms of URL has its host.
    fn remote_url() -> impl Strategy<Value = String> {
        let not_of_a_host = " \n;&|$`'\"\\_()é文．".chars().collect::<Vec<_>>();
        let part = prop_oneof![
            4 => "[a0.-]{0,6}",
            1 => prop::sample::select(vec!["://", ":", "@", "/", "https", "git", "22", "$(id)"])
                .prop_map(String::from),
            1 => prop::sample::select(not_of_a_host).prop_map(String::from),
            1 => any::<char>().prop_map(String::from),
        ];
        let parts =
            |most| prop::collection::vec(part.clone(), 0..most).prop_map(|parts| parts.concat());
        let before = prop::sample::select(vec!["https://", "https://user@", "ssh://git@", "git@"]);
        let after = prop::sample::select(vec!["/o/r", ":22/o/r", ":o/r", ""]);
        prop_oneof![
            parts(8),
            (before, parts(3), after)
                .prop_map(|(before, host, after)| [before, &host, after].concat()),
        ]
    }

    proptest! {
        #[test]
        fn the_host_read_from_any_url_is_plainly_a_host_name(url in remote_url()) {
            if let Some(host) = remote_host(&url) {
                let plain = |b: u8| b.is_ascii_alphanumeric() || b == b'.' || b == b'-';
                prop_assert!(host.bytes().all(plain), "{host:?} of {url:?}");
                prop_assert!(host.contains('.'), "{host:?} of {url:?}");
                prop_assert!(!host.starts_with('-'), "{host:?} of {url:?}");
            }
        }

        /// The repo of a URL is on the host `gh` is asked about for it, and a
        /// URL with no host is the caller's to read as a path.
        #[test]
        fn the_repo_read_from_any_url_is_on_the_host_read_from_it(url in remote_url()) {
            let repo = crate::checkouts::repo_of(&url);
            match remote_host(&url) {
                Some(host) => {
                    let host = host.to_ascii_lowercase();
                    let repo = repo.unwrap_or_default();
                    let on_it = repo == host || repo.starts_with(&format!("{host}/"));
                    prop_assert!(on_it, "{repo:?} of {url:?}");
                }
                None => prop_assert_eq!(repo, None, "{:?}", url),
            }
        }
    }

    #[test]
    fn claude_is_logged_in_when_the_json_it_prints_says_so() {
        let claude = Tool::Claude;
        let json = "{\n  \"loggedIn\": true,\n  \"authMethod\": \"claude.ai\"\n}\n";
        assert!(claude.logged_in(true, json));
        assert!(claude.logged_in(false, json), "the field is read, not the exit status");
        assert!(claude.logged_in(true, &format!("a banner from the rc file\n{json}")));
        for out in ["{\"loggedIn\": false, \"authMethod\": \"none\"}", "", "Not logged in"] {
            assert!(!claude.logged_in(true, out), "{out:?}");
        }
        assert!(Tool::Gh.logged_in(true, ""));
        assert!(!Tool::Gh.logged_in(false, "Logged in to github.com"));
    }

    #[test]
    fn grok_is_logged_in_only_when_it_says_so_and_names_a_model() {
        let grok = Tool::Grok;
        let listed = "You are logged in with grok.com.\nAvailable models:\n  * grok-4.7 (default)\n  - grok-4.5\n";
        assert!(grok.logged_in(true, listed));
        assert!(!grok.logged_in(false, listed), "a failed question is not a login");
        let out =
            "You are not authenticated.\nAvailable models:\n  * grok-4.6 (default)\n  - grok-4.5\n";
        assert!(!grok.logged_in(true, out), "a listing that names models is still logged out");
        assert!(!grok.logged_in(true, "You are logged in with grok.com.\n"));
        assert_eq!(grok.login_question(None).unwrap(), "grok models");
        assert_eq!(grok.login(None), "grok login");
        assert_eq!(grok.install(), "curl -fsSL https://x.ai/cli/install.sh | bash");
        assert_eq!(grok.logged_out(), "is not logged in or listed no model");
    }

    #[test]
    fn a_launch_command_that_cannot_start_a_worker_is_refused_in_its_own_words() {
        let fault = "the worker launch command \"ckim\" cannot start a worker";
        assert_eq!(refusal(&ALL_READY[..3], None, Some(fault)).as_deref(), Some(fault));
        // …beside the other faults, in the one run
        let said = refusal(&[(Tool::Orca, Found::Missing)], None, Some(fault)).unwrap();
        assert!(said.contains("orca is not installed") && said.ends_with(fault), "{said}");
        assert!(!said.contains("ckim is not installed"), "no install line is invented: {said}");
        // …but not to say a second time that claude is missing
        let said =
            refusal(&[(Tool::Claude, Found::Missing)], None, Some("claude resolves to nothing"));
        assert!(!said.unwrap().contains("resolves to nothing"));
    }
}
