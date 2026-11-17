//! The engine accounts a session may start on, read from mytokens, their one
//! home (docs/adr/0086): several per harness, each one mytokens secret named
//! `agent-account-<name>`, whose fields say its harness, how many sessions
//! may run on it at once, and its credential — a Claude Code token, or the
//! machine a refreshing login of Codex, Grok or Kimi Code is bound to.
//! GLOSSARY.md, **Account**, says the format.
//!
//! A credential's value goes only into the environment a session starts with
//! ([`Environment::apply`]): no type here prints it, serializes it or writes
//! it anywhere, and reading it runs mytokens with no shell and keeps nothing
//! of what that printed but what it parsed.

use std::fmt;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::Command;

use crate::Harness;
use crate::line::Line;

/// What the name of every account's mytokens secret starts with; the rest is
/// the account's name.
pub const PREFIX: &str = "agent-account-";
/// The directory under the user's home that holds each account's own home,
/// one directory per account named as the account is: the same on every
/// machine and for every program, so that a login a person made once is
/// found by all.
pub const HOMES: &str = ".agent-accounts";

/// The field that names the account's harness, by its key (`Harness::key`).
pub const HARNESS: &str = "HARNESS";
/// The field that says how many sessions may run on it at once; optional.
pub const SESSIONS: &str = "SESSIONS";
/// The field that names the machine a refreshing login is bound to.
pub const MACHINE: &str = "MACHINE";
/// The field holding a Claude Code account's token, which `claude
/// setup-token` made: also the variable a session is given it in.
pub const CLAUDE_TOKEN: &str = "CLAUDE_CODE_OAUTH_TOKEN";

/// What an account is read from: mytokens, or what stands for it in a test.
pub trait Source {
    /// What `mytokens list` printed.
    fn list(&self) -> Result<String, String>;
    /// What `mytokens get <service> --json` printed. Its error never holds
    /// what it printed.
    fn get(&self, service: &str) -> Result<String, String>;
}

/// mytokens itself, run as `program` (`mytokens`, where it is on the path).
#[derive(Debug, Clone)]
pub struct Mytokens {
    pub program: PathBuf,
}

impl Default for Mytokens {
    fn default() -> Self {
        Mytokens { program: PathBuf::from("mytokens") }
    }
}

impl Mytokens {
    fn run(&self, args: &[&str]) -> Result<String, String> {
        let program = self.program.display();
        let ran = Command::new(&self.program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("could not run {program}: {e}"))?;
        // What it printed is a secret's: neither stream goes in the error.
        if !ran.status.success() {
            return Err(format!("`{program} {}` failed ({})", args.join(" "), ran.status));
        }
        String::from_utf8(ran.stdout)
            .map_err(|_| format!("`{program} {}` printed no UTF-8", args.join(" ")))
    }
}

impl Source for Mytokens {
    fn list(&self) -> Result<String, String> {
        self.run(&["list"])
    }

    fn get(&self, service: &str) -> Result<String, String> {
        self.run(&["get", service, "--json"])
    }
}

/// One value a session's environment is given. A secret's is shown by no
/// `Debug` and by no other trait: only [`Value::expose`] gives it.
#[derive(Clone, PartialEq, Eq)]
pub struct Value {
    text: String,
    secret: bool,
}

impl Value {
    fn plain(text: String) -> Value {
        Value { text, secret: false }
    }

    fn secret(text: String) -> Value {
        Value { text, secret: true }
    }

    /// The value itself, to give a session's environment and nothing else.
    pub fn expose(&self) -> &str {
        &self.text
    }

    /// Whether it is a credential's.
    pub fn is_secret(&self) -> bool {
        self.secret
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.secret { f.write_str("<secret>") } else { fmt::Debug::fmt(&self.text, f) }
    }
}

/// The variables a session on an account starts with, in this order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub vars: Vec<(&'static str, Value)>,
}

impl Environment {
    /// Puts each variable in the environment of the process `command`
    /// starts — the one way here a credential leaves this process.
    pub fn apply(&self, command: &mut Command) {
        for (name, value) in &self.vars {
            command.env(name, value.expose());
        }
    }

    /// The value of `name`, where it is given.
    pub fn get(&self, name: &str) -> Option<&Value> {
        self.vars.iter().find(|(n, _)| *n == name).map(|(_, v)| v)
    }
}

/// How an account's sessions are logged in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Credential {
    /// A Claude Code token: a year long, refreshed never, usable on several
    /// machines at once.
    Token(Value),
    /// A refreshing login in a file of the account's home on one machine,
    /// whose refresh token no other machine may share.
    Login { machine: String },
}

/// One engine account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Account {
    /// Its name: its secret's, less [`PREFIX`].
    pub name: String,
    pub harness: Harness,
    /// How many sessions may run on it at once; `None`: it says no bound.
    pub sessions: Option<NonZeroU32>,
    pub credential: Credential,
}

/// What a session on an account can start with on one machine
/// ([`Account::session`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Session {
    /// It starts with this environment.
    Ready(Environment),
    /// The account's login is bound to another machine: it starts there.
    Elsewhere { machine: String },
    /// Its login is to be made here first, in its home, by a person running
    /// `line` ([`Account::login`]).
    LoggedOut { home: PathBuf, line: String },
}

/// The accounts mytokens holds, and each secret of the prefix that is no
/// account, with why: one bad secret keeps no other account from being read.
#[derive(Debug, Clone, Default)]
pub struct Accounts {
    pub accounts: Vec<Account>,
    pub refused: Vec<(String, String)>,
}

/// Reads every account from `source`. An error: mytokens could not list its
/// secrets.
pub fn read(source: &dyn Source) -> Result<Accounts, String> {
    let listed = source.list()?;
    let mut found = Accounts::default();
    for service in services(&listed) {
        let read = source.get(&service).and_then(|json| parse(&service, &json));
        match read {
            Ok(account) => found.accounts.push(account),
            Err(why) => found.refused.push((service, why)),
        }
    }
    Ok(found)
}

/// The account secrets `mytokens list` printed, each once, in its order: a
/// line's first column is `<service>/<account label>`.
fn services(listed: &str) -> Vec<String> {
    let mut services: Vec<String> = Vec::new();
    for line in listed.lines() {
        let first = line.split(['\t', ' ']).next().unwrap_or("");
        let service = first.rsplit_once('/').map_or(first, |(service, _)| service);
        if service.starts_with(PREFIX) && !services.iter().any(|s| s == service) {
            services.push(service.to_string());
        }
    }
    services
}

/// Whether `name` can name an account and its home: a single, plain
/// directory name.
fn plain_name(name: &str) -> bool {
    !name.is_empty()
        && !name.starts_with('.')
        && name.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// The account the secret `service` is, from what `mytokens get --json`
/// printed of it. A why never holds a field's value.
fn parse(service: &str, json: &str) -> Result<Account, String> {
    let name = service.strip_prefix(PREFIX).unwrap_or(service);
    if !plain_name(name) {
        return Err(format!("`{name}` is no account name: letters, digits, `-`, `_` and `.`"));
    }
    let read: serde_json::Value =
        serde_json::from_str(json).map_err(|_| "mytokens printed no JSON".to_string())?;
    let Some(fields) = read.get("fields").and_then(|f| f.as_object()) else {
        return Err(format!("it holds no fields: an account is a profile with `{HARNESS}`"));
    };
    let mut harness = None;
    let mut sessions = None;
    let mut machine = None;
    let mut token = None;
    for (field, value) in fields {
        let Some(text) = value.as_str() else {
            return Err(format!("its field `{field}` holds no text"));
        };
        let text = text.trim();
        match field.as_str() {
            HARNESS => harness = Some(Harness::read(text).ok_or_else(|| Harness::unknown(text))?),
            SESSIONS => {
                sessions =
                    Some(text.parse::<NonZeroU32>().map_err(|_| {
                        format!("its `{SESSIONS}` is no whole number of one or more")
                    })?)
            }
            MACHINE if !text.is_empty() => machine = Some(text.to_string()),
            CLAUDE_TOKEN if !text.is_empty() => token = Some(Value::secret(text.to_string())),
            MACHINE | CLAUDE_TOKEN => return Err(format!("its `{field}` is empty")),
            other => return Err(format!("`{other}` is no field of an account")),
        }
    }
    let Some(harness) = harness else {
        return Err(format!("it names no `{HARNESS}`"));
    };
    let credential = match (harness, token, machine) {
        (Harness::ClaudeCode, Some(token), None) => Credential::Token(token),
        (Harness::ClaudeCode, None, _) => {
            return Err(format!("a Claude Code account holds its token, `{CLAUDE_TOKEN}`"));
        }
        (Harness::ClaudeCode, Some(_), Some(_)) => {
            return Err(format!(
                "a Claude Code account's token is bound to no machine: it names no `{MACHINE}`"
            ));
        }
        (harness, None, Some(machine)) if harness.device_login().is_some() => {
            Credential::Login { machine }
        }
        (harness, _, _) if harness.device_login().is_some() => {
            return Err(format!(
                "a {} account names the one `{MACHINE}` its login is on, and holds no token",
                harness.name()
            ));
        }
        (harness, _, _) => return Err(format!("{} takes no account", harness.name())),
    };
    Ok(Account { name: name.to_string(), harness, sessions, credential })
}

impl Account {
    /// The machine its login is bound to; `None`: it is bound to none.
    pub fn machine(&self) -> Option<&str> {
        match &self.credential {
            Credential::Token(_) => None,
            Credential::Login { machine } => Some(machine),
        }
    }

    /// Its own home under the user's home `user_home`: a refreshing login's
    /// harness's user configuration, pointed at by that harness's variable
    /// (`Harness::config_dir`). A Claude Code account's session is given
    /// none (docs/adr/0086).
    pub fn home(&self, user_home: &Path) -> PathBuf {
        user_home.join(HOMES).join(&self.name)
    }

    /// Whether its login is bound to the machine `here`, or to none. A
    /// machine is named as `hostname -s` names it ([`this_machine`]), in
    /// any case.
    pub fn is_here(&self, here: &str) -> bool {
        self.machine().is_none_or(|machine| machine.eq_ignore_ascii_case(here))
    }

    /// The shell line a person runs on its machine where its login is
    /// missing: its home made, its harness pointed there and logged in with
    /// a device code. `None`: it is a token, which no line logs in.
    pub fn login(&self, user_home: &Path) -> Option<String> {
        if let Credential::Token(_) = self.credential {
            return None;
        }
        let line = self.harness.device_login()?;
        let home = self.home(user_home);
        let home = home.to_string_lossy();
        Some(
            Line::new()
                .fixed("mkdir -p ")
                .word(&home)
                .fixed(" && export ")
                .fixed(self.harness.config_dir().variable)
                .fixed("=")
                .word(&home)
                .fixed(" && ")
                .launch(line)
                .done(),
        )
    }

    /// What a session on it can start with on the machine `here`, the user's
    /// home being `user_home`: a Claude Code token goes in its environment
    /// alone, and the session keeps the configuration directory it would
    /// have — where its sessions, status line, rules and skills are, which
    /// whoever started it reads (docs/adr/0086); a login, on its machine, is
    /// its home, found logged in by its login file there.
    pub fn session(&self, here: &str, user_home: &Path) -> Session {
        let home = self.home(user_home);
        let dir = (self.harness.config_dir().variable, Value::plain(home_text(&home)));
        match &self.credential {
            Credential::Token(token) => {
                Session::Ready(Environment { vars: vec![(CLAUDE_TOKEN, token.clone())] })
            }
            Credential::Login { machine } if !self.is_here(here) => {
                Session::Elsewhere { machine: machine.clone() }
            }
            Credential::Login { .. } => {
                let logged_in = self
                    .harness
                    .kept()
                    .links
                    .first()
                    .is_some_and(|login| home.join(login.kept).exists());
                match self.login(user_home) {
                    Some(line) if !logged_in => Session::LoggedOut { home, line },
                    _ => Session::Ready(Environment { vars: vec![dir] }),
                }
            }
        }
    }
}

fn home_text(home: &Path) -> String {
    home.to_string_lossy().into_owned()
}

/// This machine's name, as `hostname -s` says it: what an account's
/// `MACHINE` names. The system's name up to its first `.`.
pub fn this_machine() -> Result<String, String> {
    let system = gethostname::gethostname();
    let system = system.to_string_lossy();
    let name = short_name(&system);
    if name.is_empty() {
        return Err(format!("the system names no machine ({system:?})"));
    }
    Ok(name.to_string())
}

/// `name` as `hostname -s` prints it: trimmed, and cut at its first `.`.
fn short_name(name: &str) -> &str {
    let name = name.trim();
    name.split_once('.').map_or(name, |(short, _)| short)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn this_machine_is_named_as_hostname_s_names_it() {
        assert_eq!(short_name("macs-Mac-Mini.local\n"), "macs-Mac-Mini");
        assert_eq!(short_name("box"), "box");
        assert_eq!(short_name(".local"), "");
        let ran = Command::new("hostname").arg("-s").output().unwrap();
        assert_eq!(this_machine().unwrap(), String::from_utf8_lossy(&ran.stdout).trim());
    }

    struct Held {
        list: &'static str,
        secrets: Vec<(&'static str, String)>,
    }

    impl Source for Held {
        fn list(&self) -> Result<String, String> {
            Ok(self.list.to_string())
        }
        fn get(&self, service: &str) -> Result<String, String> {
            self.secrets
                .iter()
                .find(|(s, _)| *s == service)
                .map(|(_, json)| json.clone())
                .ok_or_else(|| format!("{service} is no secret"))
        }
    }

    fn fields(pairs: &[(&str, &str)]) -> String {
        let map: serde_json::Map<String, serde_json::Value> =
            pairs.iter().map(|(k, v)| (k.to_string(), serde_json::json!(v))).collect();
        serde_json::json!({ "fields": map, "level": "server" }).to_string()
    }

    #[test]
    fn the_list_names_each_account_secret_once() {
        let listed = "bark/default\tstatic\tserver  — push\n\
                      agent-account-a/default\tprofile\tserver  — fields: HARNESS\n\
                      agent-account-b/default\tprofile\tpersonal\n\
                      agent-account-a/other\tprofile\tserver\n";
        assert_eq!(services(listed), ["agent-account-a", "agent-account-b"]);
    }

    #[test]
    fn a_secret_that_is_no_account_is_refused_alone_and_says_no_value() {
        let held = Held {
            list: "agent-account-ok/default\nagent-account-bad/default\nagent-account-two/default\n\
                   agent-account-gone/default\n",
            secrets: vec![
                ("agent-account-ok", fields(&[("HARNESS", "codex"), ("MACHINE", "mini")])),
                (
                    "agent-account-bad",
                    fields(&[("HARNESS", "codex"), ("CLAUDE_CODE_OAUTH_TOKEN", "sk-leak")]),
                ),
                (
                    "agent-account-two",
                    fields(&[
                        ("HARNESS", "claude-code"),
                        ("CLAUDE_CODE_OAUTH_TOKEN", "sk-leak"),
                        ("MACHINE", "mini"),
                    ]),
                ),
            ],
        };
        let read = read(&held).unwrap();
        assert_eq!(read.accounts.len(), 1);
        let refused: Vec<&str> = read.refused.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(refused, ["agent-account-bad", "agent-account-two", "agent-account-gone"]);
        assert!(!format!("{read:?}").contains("sk-leak"), "{read:?}");
    }

    #[test]
    fn each_field_is_checked() {
        let refused =
            |pairs: &[(&str, &str)]| parse("agent-account-x", &fields(pairs)).unwrap_err();
        assert!(refused(&[("MACHINE", "m")]).contains("no `HARNESS`"));
        assert!(refused(&[("HARNESS", "pi"), ("MACHINE", "m")]).contains("takes no account"));
        assert!(refused(&[("HARNESS", "codex")]).contains("`MACHINE`"));
        assert!(refused(&[("HARNESS", "claude-code")]).contains("CLAUDE_CODE_OAUTH_TOKEN"));
        assert!(
            refused(&[("HARNESS", "codex"), ("MACHINE", "m"), ("SESSIONS", "0")])
                .contains("SESSIONS")
        );
        assert!(
            refused(&[("HARNESS", "codex"), ("MACHINE", "m"), ("OTHER", "1")]).contains("`OTHER`")
        );
        assert!(refused(&[("HARNESS", "nope")]).contains("is no harness"));
        assert!(
            parse("agent-account-../x", &fields(&[("HARNESS", "codex"), ("MACHINE", "m")]))
                .unwrap_err()
                .contains("no account name")
        );
        let kimi = parse(
            "agent-account-k",
            &fields(&[("HARNESS", "kimi"), ("MACHINE", "m"), ("SESSIONS", "3")]),
        )
        .unwrap();
        assert_eq!((kimi.harness, kimi.sessions.map(NonZeroU32::get)), (Harness::Kimi, Some(3)));
    }

    #[test]
    fn a_login_line_points_the_harness_at_the_accounts_home() {
        for (harness, line) in [
            (
                "codex",
                "mkdir -p '/u/.agent-accounts/a' && export CODEX_HOME='/u/.agent-accounts/a' && codex login --device-auth",
            ),
            (
                "grok",
                "mkdir -p '/u/.agent-accounts/a' && export GROK_HOME='/u/.agent-accounts/a' && grok login --device-auth",
            ),
            (
                "kimi",
                "mkdir -p '/u/.agent-accounts/a' && export KIMI_CODE_HOME='/u/.agent-accounts/a' && kimi login",
            ),
        ] {
            let account =
                parse("agent-account-a", &fields(&[("HARNESS", harness), ("MACHINE", "m")]))
                    .unwrap();
            assert_eq!(account.login(Path::new("/u")).as_deref(), Some(line));
        }
    }
}
