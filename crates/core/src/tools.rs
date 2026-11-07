//! The commands a fleet cannot work without, and what a human runs when one
//! is missing, logged out, not running or does not know this checkout. A start that lacks any refuses, naming every
//! fault with the line that fixes it; the tool itself never installs anything
//! and logs nobody in.

use crate::harness::Harness;
use crate::line::quoted;

/// A command the fleet, or a worker it starts, runs. What a harness's own
/// program is — its name, how it installs, how it is asked about its login
/// — is that harness's (`Harness`); the rest are no harness's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Git,
    Gh,
    Orca,
    /// DeepSeek Harness's own program, which dsh-tui runs (`Harness::needs`).
    Dsh,
    /// A harness: needed only when it is what starts a worker.
    Harness(Harness),
}

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
            Tool::Dsh => "dsh",
            Tool::Harness(harness) => harness.program(),
        }
    }

    /// Whether it is what a worker launch command starts: a harness, or
    /// what one needs (`Harness::needs`).
    pub fn starts_workers(self) -> bool {
        matches!(self, Tool::Harness(_) | Tool::Dsh)
    }

    /// The line that installs it. Homebrew's own `orca` cask is another
    /// program, so orca's names its tap.
    pub fn install(self) -> &'static str {
        match self {
            Tool::Git => "brew install git",
            Tool::Gh => "brew install gh",
            Tool::Orca => "brew install --cask stablyai/orca/orca",
            Tool::Dsh => "install DeepSeek Harness (dsh) and add it to PATH",
            Tool::Harness(harness) => harness.install(),
        }
    }

    /// The shell line whose answer says whether it is logged in (`logged_in`
    /// reads it), `gh` for the `host` the repo's remote is on. `None`: it
    /// logs in to nothing.
    pub fn login_question(self, host: Option<&str>) -> Option<String> {
        match self {
            Tool::Git | Tool::Orca | Tool::Dsh => None,
            Tool::Gh => Some(format!("gh auth status{}", hostname(host))),
            Tool::Harness(harness) => harness.login().map(|login| login.question.to_string()),
        }
    }

    /// Whether the answer to `login_question` says it is logged in: `gh` by
    /// its exit status, a harness as it says (`Harness::logged_in`).
    pub fn logged_in(self, succeeded: bool, printed: &str) -> bool {
        match self {
            Tool::Harness(harness) => harness.logged_in(succeeded, printed),
            Tool::Git | Tool::Gh | Tool::Orca | Tool::Dsh => succeeded,
        }
    }

    /// What it is said to be when it is not logged in.
    pub fn logged_out(self) -> &'static str {
        match self {
            Tool::Harness(harness) => harness.login().map_or(LOGGED_OUT, |login| login.logged_out),
            Tool::Git | Tool::Gh | Tool::Orca | Tool::Dsh => LOGGED_OUT,
        }
    }

    /// The line that logs it in.
    pub fn login(self, host: Option<&str>) -> String {
        match self {
            Tool::Harness(harness) => {
                harness.login().map(|login| login.line.to_string()).unwrap_or_default()
            }
            // github.com is where `gh auth login` logs in unasked.
            _ => format!("gh auth login{}", hostname(host.filter(|host| *host != "github.com"))),
        }
    }
}

/// What a tool is said to be when it is not logged in, unless its own words
/// say more.
const LOGGED_OUT: &str = "is not logged in";

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
        (Tool::Harness(Harness::ClaudeCode), Found::Ready),
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
    fn a_harness_is_named_in_its_own_words_with_its_own_lines() {
        let codex = Tool::Harness(Harness::Codex);
        assert_eq!(codex.name(), "codex");
        assert!(codex.starts_workers());
        assert!(!codex.logged_in(false, ""));
        assert_eq!(
            fault(codex, &Found::LoggedOut(LoginAnswer::default()), None, &[]).unwrap(),
            "  codex is not logged in where a worker reads its login, `~/.codex/auth.json` (a \
             login kept in a keyring is in no file): codex login"
        );
        assert_eq!(
            fault(codex, &Found::Missing, None, &[]).unwrap(),
            "  codex is not installed: brew install --cask codex"
        );
        assert!(Tool::Gh.logged_in(true, ""));
        assert!(!Tool::Gh.logged_in(false, "Logged in to github.com"));
    }

    #[test]
    fn a_harness_a_stage_calls_for_is_named_with_the_stage_and_the_line_that_fixes_it() {
        let callers = ["the stage `build` of the workflow `feature`".to_string()];
        assert_eq!(
            fault(Tool::Harness(Harness::Pi), &Found::Missing, None, &callers).unwrap(),
            "  pi is not installed, called for by the stage `build` of the workflow `feature`: \
             curl -fsSL https://pi.dev/install.sh | sh"
        );
        assert_eq!(
            fault(
                Tool::Harness(Harness::Pi),
                &Found::LoggedOut(LoginAnswer::default()),
                Some("github.com"),
                &[]
            )
            .unwrap(),
            "  pi holds credentials for no provider (`/login` at its prompt logs it in): pi"
        );
        assert_eq!(fault(Tool::Harness(Harness::Pi), &Found::Ready, None, &callers), None);
        // …under the words every refusal starts with, and beside a command's own fault
        let lines = [fault(
            Tool::Harness(Harness::Pi),
            &Found::LoggedOut(LoginAnswer::default()),
            None,
            &callers,
        )
        .unwrap()];
        let said = refused(&lines, Some("the command of another harness takes no flag")).unwrap();
        assert!(said.contains("nothing was started"), "{said}");
        assert!(
            said.contains(" of the workflow `feature`: pi\nand the command of another"),
            "{said}"
        );
        assert_eq!(refused(&[], None), None);
        // a missing pi is not said a second time as a command that resolves to nothing
        let said = refusal(
            &[(Tool::Harness(Harness::Pi), Found::Missing)],
            None,
            Some("pi resolves to nothing"),
        );
        assert!(!said.unwrap().contains("resolves to nothing"));
    }

    #[test]
    fn a_host_with_everything_is_not_refused() {
        assert_eq!(refusal(&ALL_READY, None, None), None);
        assert_eq!(refusal(&[], Some("github.com"), None), None);
    }

    #[test]
    fn every_fault_is_named_with_the_line_that_fixes_it() {
        let found = [
            (Tool::Git, Found::Missing),
            (Tool::Gh, Found::LoggedOut(LoginAnswer::default())),
            (Tool::Orca, Found::Missing),
            (Tool::Harness(Harness::ClaudeCode), Found::LoggedOut(LoginAnswer::default())),
        ];
        let said = refusal(&found, Some("github.com"), None).unwrap();
        let lines: Vec<&str> = said.lines().skip(1).collect();
        assert_eq!(
            lines,
            [
                "  git is not installed: brew install git",
                "  gh is not logged in: gh auth login",
                "  orca is not installed: brew install --cask stablyai/orca/orca",
                "  claude is not logged in: claude auth login",
            ]
        );
        assert!(said.contains("nothing was started"), "{said}");

        let missing =
            [(Tool::Gh, Found::Missing), (Tool::Harness(Harness::ClaudeCode), Found::Missing)];
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
    fn a_launch_command_that_cannot_start_a_worker_is_refused_in_its_own_words() {
        let fault = "the worker launch command \"ckim\" cannot start a worker";
        assert_eq!(refusal(&ALL_READY[..3], None, Some(fault)).as_deref(), Some(fault));
        // …beside the other faults, in the one run
        let said = refusal(&[(Tool::Orca, Found::Missing)], None, Some(fault)).unwrap();
        assert!(said.contains("orca is not installed") && said.ends_with(fault), "{said}");
        assert!(!said.contains("ckim is not installed"), "no install line is invented: {said}");
        // …but not to say a second time that claude is missing
        let said = refusal(
            &[(Tool::Harness(Harness::ClaudeCode), Found::Missing)],
            None,
            Some("claude resolves to nothing"),
        );
        assert!(!said.unwrap().contains("resolves to nothing"));
    }
}
