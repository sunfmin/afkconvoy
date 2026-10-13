//! The commands a fleet cannot work without, and what a human runs when one
//! is missing, logged out, not running or does not know this checkout. A start that lacks any refuses, naming every
//! fault with the line that fixes it; the tool itself never installs anything
//! and logs nobody in.

use crate::launch::{PI, first_word};
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
}

/// The coding agents among the tools: each needed only as the first word of
/// the worker launch command.
const AGENTS: [Tool; 2] = [Tool::Claude, Tool::Pi];

/// What pi says in place of its models when it has credentials for none.
const PI_NO_MODELS: &str = "No models available";

/// What a start found of one tool: the first thing in its way. A missing one
/// is never also logged out, and one not running is asked nothing more.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Found {
    Ready,
    Missing,
    LoggedOut,
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
            Tool::Claude => "claude",
            Tool::Pi => PI,
        }
    }

    /// The line that installs it. Homebrew's own `orca` cask is another
    /// program, so orca's names its tap; Claude Code's is its native
    /// installer, which keeps itself up to date; pi's is its npm package.
    pub fn install(self) -> &'static str {
        match self {
            Tool::Git => "brew install git",
            Tool::Gh => "brew install gh",
            Tool::Orca => "brew install --cask stablyai/orca/orca",
            Tool::Claude => "curl -fsSL https://claude.ai/install.sh | bash",
            Tool::Pi => "npm install -g @earendil-works/pi-coding-agent",
        }
    }

    /// The shell line whose answer says whether it is logged in (`logged_in`
    /// reads it), `gh` for the `host` the repo's remote is on. `None`: it
    /// logs in to nothing.
    pub fn login_question(self, host: Option<&str>) -> Option<String> {
        match self {
            Tool::Git | Tool::Orca => None,
            Tool::Gh => Some(format!("gh auth status{}", hostname(host))),
            Tool::Claude => Some("claude auth status".to_string()),
            // pi logs in to providers, and lists the models of those it has
            // credentials for: which one a worker uses is the command's to say.
            Tool::Pi => Some("pi --list-models".to_string()),
        }
    }

    /// Whether the answer to `login_question` says it is logged in: `gh` by
    /// its exit status, Claude Code by the `loggedIn` field of the JSON it
    /// prints — whatever else the shell printed around it — and pi by listing
    /// a model at all.
    pub fn logged_in(self, succeeded: bool, printed: &str) -> bool {
        match self {
            Tool::Claude => {
                let dense: String = printed.split_whitespace().collect();
                dense.contains("\"loggedIn\":true")
            }
            Tool::Pi => succeeded && !printed.trim().is_empty() && !printed.contains(PI_NO_MODELS),
            Tool::Git | Tool::Gh | Tool::Orca => succeeded,
        }
    }

    /// The line that logs it in. pi's starts it: `/login` is typed at its
    /// prompt.
    pub fn login(self, host: Option<&str>) -> String {
        match self {
            Tool::Claude => "claude auth login".to_string(),
            Tool::Pi => "pi  (then /login at its prompt)".to_string(),
            // github.com is where `gh auth login` logs in unasked.
            _ => format!("gh auth login{}", hostname(host.filter(|host| *host != "github.com"))),
        }
    }
}

fn hostname(host: Option<&str>) -> String {
    host.map(|host| format!(" --hostname {host}")).unwrap_or_default()
}

/// The tools a start needs. `launch` is the worker launch command, when the
/// start knows it: a coding agent is needed only as its first word — what a
/// wrapper or another agent runs and logs in to is its own business.
pub fn needed(launch: Option<&str>) -> Vec<Tool> {
    let mut tools = vec![Tool::Git, Tool::Gh, Tool::Orca];
    let word = launch.and_then(first_word);
    tools.extend(AGENTS.into_iter().filter(|agent| word == Some(agent.name())));
    tools
}

/// The host a remote's URL names, for `gh`: of `https://host/o/r`,
/// `ssh://git@host:22/o/r` and `git@host:o/r`. `None` for a path, and for
/// anything that is not plainly a host name — it goes into a shell line.
pub fn remote_host(url: &str) -> Option<&str> {
    let url = url.trim();
    let authority = match url.split_once("://") {
        Some((_, rest)) => rest.split('/').next()?,
        None => url.split_once(':')?.0,
    };
    let host = authority.rsplit('@').next()?;
    let host = host.split(':').next()?;
    let plain = host.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-');
    (plain && host.contains('.') && !host.starts_with('-')).then_some(host)
}

/// Why a start refuses, or `None` when it has all it needs: every fault in
/// `found`, each with the line that fixes it, and `launch_fault` — why the
/// worker launch command cannot start a worker — unless that only says again
/// that the coding agent is missing. `host` is the one `gh` was asked about.
pub fn refusal(
    found: &[(Tool, Found)],
    host: Option<&str>,
    launch_fault: Option<&str>,
) -> Option<String> {
    let mut faults: Vec<String> = vec![];
    let mut agent_missing = false;
    for (tool, found) in found {
        let name = tool.name();
        match found {
            Found::Ready => {}
            Found::Missing => {
                agent_missing = agent_missing || AGENTS.contains(tool);
                faults.push(format!("  {name} is not installed: {}", tool.install()));
            }
            Found::LoggedOut => {
                faults.push(format!("  {name} is not logged in: {}", tool.login(host)));
            }
            Found::NotRunning => faults.push(format!("  {name} is not running: {START_ORCA}")),
            Found::KnowsNoCheckout(path) => {
                faults
                    .push(format!("  {name} does not know this checkout: {}", add_checkout(path)));
            }
        }
    }
    let launch_fault = launch_fault.filter(|_| !agent_missing);
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
    fn claude_is_needed_only_as_the_first_word_of_the_launch_command() {
        let always = [Tool::Git, Tool::Gh, Tool::Orca];
        assert_eq!(
            needed(Some("claude --dangerously-skip-permissions")),
            [Tool::Git, Tool::Gh, Tool::Orca, Tool::Claude]
        );
        assert_eq!(needed(Some("  claude")).len(), 4);
        // a wrapper, an alias that ends in claude, another agent, a path
        for other in ["kim --yolo", "wrap claude", "codex", "/usr/local/bin/claude", "claudex", ""]
        {
            assert_eq!(needed(Some(other)), always, "{other:?}");
        }
        assert_eq!(needed(None), always);
    }

    #[test]
    fn pi_is_needed_only_as_the_first_word_of_the_launch_command() {
        assert_eq!(needed(Some("pi --model opus")), [Tool::Git, Tool::Gh, Tool::Orca, Tool::Pi]);
        for other in ["wrap pi", "pip install", "/opt/homebrew/bin/pi", "claude"] {
            assert!(!needed(Some(other)).contains(&Tool::Pi), "{other:?}");
        }
    }

    #[test]
    fn pi_is_logged_in_when_it_lists_a_model() {
        let pi = Tool::Pi;
        assert_eq!(pi.login_question(Some("github.com")).unwrap(), "pi --list-models");
        let listed = "provider      model    context\nopenai-codex  gpt-5.5  272K\n";
        assert!(pi.logged_in(true, listed));
        assert!(!pi.logged_in(false, listed));
        let none = "No models available. Use /login to log into a provider via OAuth or API key.";
        assert!(!pi.logged_in(true, none));
        assert!(!pi.logged_in(true, " \n"));
        let said = refusal(&[(pi, Found::LoggedOut), (pi, Found::Missing)], None, Some("fault"));
        let said = said.unwrap();
        assert!(
            said.contains("  pi is not logged in: pi  (then /login at its prompt)\n"),
            "{said}"
        );
        assert!(
            said.ends_with("  pi is not installed: npm install -g @earendil-works/pi-coding-agent"),
            "no second word that the agent is missing: {said}"
        );
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
            (Tool::Gh, Found::LoggedOut),
            (Tool::Orca, Found::Missing),
            (Tool::Claude, Found::LoggedOut),
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
        let said = refusal(&[(Tool::Gh, Found::LoggedOut), (Tool::Orca, unknown)], None, None);
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
        let out = [(Tool::Gh, Found::LoggedOut)];
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
