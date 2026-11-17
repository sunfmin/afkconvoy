//! The engine accounts this host's workers start on (GLOSSARY: **Account**;
//! docs/adr/0086): read from mytokens, their one home; what
//! this host last heard of each — its usage windows and a limit it met —
//! kept in its account book; and the one a worker starts on, chosen by the
//! core (`afkconvoy_core::engine_account::choose`).
//!
//! A credential's value is never written here: a worker's line fetches its
//! account's token from mytokens itself, as it starts (`Chosen::line`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use afkconvoy_core::engine_account::{self, Here, Standing, Window};
use afkconvoy_core::harness::Harness;
use afkconvoy_core::harness::account::{self, Account, Credential, Session};
use afkconvoy_core::line::Line;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::orca::Worktree;

/// The note beside a worktree's git directory that names the account its
/// worker started on; none: it started on its harness's own login.
pub(crate) const NOTE: &str = "convoy-account";

/// How long what mytokens listed answers again, in one process.
const LISTED_FOR: Duration = Duration::from_secs(60);

/// How long ago a worker's record of its account may have been written and
/// still be read, in seconds.
const RECORDS_FRESH: u64 = 3600;

/// What mytokens holds of the accounts, and the mytokens that said it.
pub(crate) struct Listed {
    pub accounts: Vec<Account>,
    pub refused: Vec<(String, String)>,
    program: PathBuf,
}

type Read = Arc<Result<Listed, String>>;

/// What mytokens holds of the accounts, read at most once in `LISTED_FOR`
/// by this process. An error: mytokens is not installed, or could not list
/// its secrets; no worker then starts on an account.
pub(crate) fn listed() -> Read {
    static KEPT: Mutex<Option<(Instant, Read)>> = Mutex::new(None);
    let mut kept = KEPT.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some((at, read)) = kept.as_ref()
        && at.elapsed() < LISTED_FOR
    {
        return read.clone();
    }
    let read: Read = Arc::new(read_listed());
    *kept = Some((Instant::now(), read.clone()));
    read
}

fn read_listed() -> Result<Listed, String> {
    let program = mytokens().ok_or("mytokens is not installed: no worker starts on an account")?;
    let source = account::Mytokens { program: program.clone() };
    let read = account::read(&source)?;
    Ok(Listed { accounts: read.accounts, refused: read.refused, program })
}

/// mytokens, where it is on PATH. A unit test reads none: the host's
/// keychain is not its.
fn mytokens() -> Option<PathBuf> {
    if cfg!(test) {
        return None;
    }
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path).map(|dir| dir.join("mytokens")).find(|file| file.is_file())
}

/// What this host last heard of one account.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Kept {
    /// When a worker on it last recorded its windows, and what they said.
    windows_at: Option<u64>,
    windows: Vec<Window>,
    /// The limit it met last, where one did.
    limit: Option<Limit>,
}

/// A limit an account met: when, until when it rests, and what said it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct Limit {
    at: u64,
    until: u64,
    said: String,
}

/// The host's account book: what it last heard of each account, by name.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
struct Book {
    accounts: BTreeMap<String, Kept>,
}

/// The account book in the host's database: a row each of `account_book`,
/// by the account's name. It was `accounts/book.json` in the cache
/// directory, with `accounts/book.lock` beside it.
pub(crate) static STORE: crate::host_db::Store = crate::host_db::Store {
    file: "accounts/book.json",
    old: crate::host_db::Old::Json,
    layout: crate::host_db::Layout::Tables { export: export_book, import: import_book },
};

fn export_book(db: &rusqlite::Connection) -> Result<Option<Value>> {
    let rows = crate::host_db::rows(db, "account_book")?;
    Ok((!rows.is_empty())
        .then(|| json!({"accounts": rows.into_iter().collect::<serde_json::Map<_, _>>()})))
}

fn import_book(db: &rusqlite::Connection, book: &Value) -> Result<()> {
    let accounts = book["accounts"].as_object().cloned().unwrap_or_default();
    crate::host_db::put_rows(
        db,
        "account_book",
        accounts.iter().map(|(name, kept)| (name.clone(), kept)),
    )
}

fn read_book() -> Book {
    crate::host_db::read(&STORE).unwrap_or_default()
}

/// Changes the book by `change`, in one transaction of the host's
/// database: two processes of the host never lose each other's change.
fn change_book(change: impl FnOnce(&mut Book)) -> Result<()> {
    crate::host_db::change(&STORE, change).context("cannot change the account book")
}

/// Keeps what a worker on `name` recorded of its windows at `at`, unless the
/// book holds a newer record.
fn observe(book: &mut Book, name: &str, at: u64, windows: Vec<Window>) {
    let kept = book.accounts.entry(name.to_string()).or_default();
    if kept.windows_at.is_none_or(|kept| at > kept) {
        kept.windows_at = Some(at);
        kept.windows = windows;
    }
}

/// Keeps that `name` met a limit at `at`, resetting at `resets_at` where it
/// says, as `said` says it: it rests until then (`engine_account::rest_until`).
fn met(book: &mut Book, name: &str, at: u64, resets_at: Option<u64>, said: String) {
    let kept = book.accounts.entry(name.to_string()).or_default();
    if kept.limit.as_ref().is_none_or(|limit| at > limit.at) {
        let until = engine_account::rest_until(at, resets_at);
        kept.limit = Some(Limit { at, until, said });
    }
}

/// Keeps that the worker in `worktree` met its account's limit `now`, as
/// worker health read it off its screen or its error, `retry_after` seconds
/// from now where it says: its account rests. `false`: its worker started
/// on no account, and its harness's own login met it.
pub(crate) fn rest(
    worktree: &Worktree,
    now: u64,
    retry_after: Option<u64>,
    said: &str,
) -> Result<bool> {
    let Some(name) = account_of(worktree) else { return Ok(false) };
    let resets_at = retry_after.map(|after| now.saturating_add(after));
    change_book(|book| met(book, &name, now, resets_at, said.to_string()))?;
    Ok(true)
}

/// The account the worker in `worktree` started on, as its note says.
pub(crate) fn account_of(worktree: &Worktree) -> Option<String> {
    let kept = std::fs::read_to_string(worktree.note(NOTE).ok()?).ok()?;
    let name = kept.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// The refreshing login the worker in `worktree` starts on, where its note
/// names an account whose login is a file (Codex, Grok, Kimi Code): its name
/// and its home, `~/.agent-accounts/<name>` under `user_home`. Its worker's
/// kept home links that home's login in place of the host user's
/// (`kept::for_worker`, docs/adr/0086). `None`: it started on no account,
/// or on a token, which no file holds.
pub(crate) fn login_of(worktree: &Worktree, user_home: &Path) -> Option<Login> {
    let name = account_of(worktree)?;
    let listed = listed();
    let account = listed.as_ref().as_ref().ok()?.accounts.iter().find(|a| a.name == name)?;
    matches!(account.credential, Credential::Login { .. })
        .then(|| Login { home: account.home(user_home), name })
}

/// An account's refreshing login, as a worker's kept home links it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Login {
    pub name: String,
    /// Its home, `~/.agent-accounts/<name>`: the harness's user
    /// configuration, where its login file is.
    pub home: PathBuf,
}

/// Keeps in the book what the workers in `worktrees`, each on its harness,
/// last recorded of their accounts by `now`: their windows, and a limit of
/// their account's use they met.
pub(crate) fn sync(worktrees: &[(Worktree, Harness)], now: u64) {
    let since = now.saturating_sub(RECORDS_FRESH);
    let mut heard = vec![];
    for (worktree, harness) in worktrees {
        let Some(name) = account_of(worktree) else { continue };
        if let Ok(Some((at, said))) = crate::session::windows(&worktree.path, *harness, since) {
            heard.push((name.clone(), Heard::Windows(at, windows_of(&said))));
        }
        if let Ok(Some((at, limit))) = crate::session::limit(&worktree.path, *harness, since) {
            let said = format!(
                "its session recorded a usage limit{}",
                limit.window.map(|window| format!(" of `{window}`")).unwrap_or_default()
            );
            heard.push((name, Heard::Limit(at, limit.resets_at, said)));
        }
    }
    if heard.is_empty() {
        return;
    }
    let _ = change_book(|book| {
        for (name, heard) in heard {
            match heard {
                Heard::Windows(at, windows) => observe(book, &name, at, windows),
                Heard::Limit(at, resets_at, said) => met(book, &name, at, resets_at, said),
            }
        }
    });
}

enum Heard {
    Windows(u64, Vec<Window>),
    Limit(u64, Option<u64>, String),
}

/// The windows its engine said, as the book keeps them: one that names no
/// reset says nothing of when it stops holding, and is left out.
fn windows_of(said: &[afkconvoy_core::harness::window::Window]) -> Vec<Window> {
    said.iter()
        .filter_map(|window| {
            Some(Window {
                name: window.name.clone(),
                used: window.used_percent.clamp(0.0, 100.0).round() as u64,
                resets_at: window.resets_at?,
            })
        })
        .collect()
}

/// Each account of `harness` in `listed`, as admission judges it on the
/// machine `here`: what the book last heard of it, and how many of the
/// host's workers run on it, `running`.
fn standings_of(
    listed: &Listed,
    book: &Book,
    harness: Harness,
    here: &str,
    user_home: &Path,
    running: &BTreeMap<String, usize>,
) -> Vec<Standing> {
    listed
        .accounts
        .iter()
        .filter(|account| account.harness == harness)
        .map(|account| {
            let kept = book.accounts.get(&account.name).cloned().unwrap_or_default();
            Standing {
                name: account.name.clone(),
                harness: harness.key().to_string(),
                windows: kept.windows,
                rests_until: kept.limit.map(|limit| limit.until),
                running: running.get(&account.name).copied().unwrap_or(0),
                sessions: account.sessions.map(|sessions| sessions.get()),
                here: match account.session(here, user_home) {
                    Session::Ready(_) => Here::Ready,
                    Session::Elsewhere { machine } => Here::Elsewhere { machine },
                    Session::LoggedOut { line, .. } => Here::LoggedOut { line },
                },
            }
        })
        .collect()
}

/// How many workers of this host run on each account: its worktrees a
/// worker holds whose note names one.
fn running() -> BTreeMap<String, usize> {
    let mut running = BTreeMap::new();
    for on in Worktree::on_host().unwrap_or_default() {
        if !on.held || on.fleets {
            continue;
        }
        let worktree = Worktree::at(on.path, String::new());
        if let Some(name) = account_of(&worktree) {
            *running.entry(name).or_insert(0) += 1;
        }
    }
    running
}

/// The accounts of one harness as admission reads them now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accounts {
    pub harness: Harness,
    pub standings: Vec<Standing>,
    /// The share of a window at or past which an account takes no new
    /// worker (`--account-limit`).
    pub limit: u64,
    pub now: u64,
}

impl Accounts {
    /// The one a new worker starts on now; `None`: none takes one.
    pub fn chosen(&self) -> Option<&Standing> {
        engine_account::choose(&self.standings, self.limit, self.now)
    }

    /// Each account and why it takes no new worker, where it takes none.
    pub fn refused(&self) -> Vec<(String, engine_account::Refused)> {
        self.standings
            .iter()
            .filter_map(|standing| {
                Some((standing.name.clone(), standing.refused(self.limit, self.now)?))
            })
            .collect()
    }
}

/// The accounts of `harness` as admission reads them `now`; `None`: a
/// worker of it starts on no account — it holds none in mytokens, mytokens
/// cannot be read.
pub(crate) fn of(harness: Harness, now: u64) -> Option<Accounts> {
    let listed = listed();
    let listed = listed.as_ref().as_ref().ok()?;
    if !listed.accounts.iter().any(|account| account.harness == harness) {
        return None;
    }
    let here = account::this_machine().unwrap_or_default();
    let home = user_home()?;
    let standings = standings_of(listed, &read_book(), harness, &here, &home, &running());
    Some(Accounts { harness, standings, limit: limit(), now })
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// The share of a window, in percent, at or past which an account takes no
/// new worker, as the host says it (`--account-limit`).
pub(crate) fn limit() -> u64 {
    crate::host_settings::of_host()
        .ok()
        .and_then(|settings| settings.account_limit)
        .unwrap_or(engine_account::LIMIT)
}

/// The account a new worker of `harness` starts on.
pub(crate) struct Chosen {
    pub name: String,
    service: String,
    credential: Credential,
    program: PathBuf,
}

impl Chosen {
    /// `command`, started on this account: its token fetched from mytokens
    /// by the line itself, as it starts, into the session's environment
    /// alone. The line holds where the token is, never the token; where
    /// mytokens gives none, nothing starts.
    pub fn line(&self, command: &str) -> String {
        match &self.credential {
            Credential::Token(_) => Line::new()
                .fixed(account::CLAUDE_TOKEN)
                .fixed("=\"$(")
                .word(&self.program.to_string_lossy())
                .fixed(" get ")
                .word(&self.service)
                .fixed(" --field ")
                .fixed(account::CLAUDE_TOKEN)
                .fixed(")\" && export ")
                .fixed(account::CLAUDE_TOKEN)
                .fixed(" && ")
                .launch(command)
                .done(),
            // Its worker's kept home links its account's login, not the
            // host user's (`login_of`): the command is started as it is.
            Credential::Login { .. } => command.to_string(),
        }
    }
}

/// The account a new worker of `harness` starts on `now`. `Ok(None)`: it
/// starts on its harness's own login, holding no account. An error: it
/// holds accounts and none takes a worker now, each said with why.
pub(crate) fn choose(harness: Harness, now: u64) -> Result<Option<Chosen>> {
    let Some(accounts) = of(harness, now) else { return Ok(None) };
    let Some(chosen) = accounts.chosen() else {
        anyhow::bail!("{}", none_said(&accounts));
    };
    let listed = listed();
    let listed = listed.as_ref().as_ref().map_err(|why| anyhow::anyhow!("{why}"))?;
    let account = listed
        .accounts
        .iter()
        .find(|account| account.name == chosen.name)
        .context("the chosen account is no longer listed")?;
    Ok(Some(Chosen {
        name: account.name.clone(),
        service: format!("{}{}", account::PREFIX, account.name),
        credential: account.credential.clone(),
        program: listed.program.clone(),
    }))
}

/// That no account of `accounts` takes a worker, and why each takes none.
pub(crate) fn none_said(accounts: &Accounts) -> String {
    let each: Vec<String> = accounts
        .refused()
        .iter()
        .map(|(name, why)| format!("`{name}`: {}", why.said(accounts.now)))
        .collect();
    format!(
        "no {} account takes a new worker: {}; its workers wait",
        accounts.harness.key(),
        each.join("; ")
    )
}

/// Keeps on `worktree` the account its worker started on, `None` for its
/// harness's own login: a note a later worker there left is not its.
pub(crate) fn keep(worktree: &Worktree, chosen: Option<&Chosen>) -> Result<()> {
    let file = worktree.note(NOTE)?;
    match chosen {
        Some(chosen) => std::fs::write(&file, &chosen.name)
            .with_context(|| format!("cannot write {}", file.display())),
        None => match std::fs::remove_file(&file) {
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                Err(error).with_context(|| format!("cannot remove {}", file.display()))
            }
            _ => Ok(()),
        },
    }
}

/// One finding for each harness whose accounts none takes a new worker
/// `now`: its workers wait until a window resets or a rest ends, and the
/// watch is not called, since nothing on the host gives an account's use
/// back before then.
pub(crate) fn findings(now: u64) -> Vec<crate::doctor::Finding> {
    use crate::doctor::{Kind, finding};
    Harness::ALL
        .into_iter()
        .filter_map(|harness| of(harness, now))
        .filter(|accounts| accounts.chosen().is_none())
        .map(|accounts| {
            let harness = accounts.harness.key();
            finding(
                Kind::AccountsSpent,
                json!(["accounts-spent", harness]),
                format!("no {harness} account takes a new worker: its workers wait"),
                none_said(&accounts),
                match accounts.harness {
                    Harness::ClaudeCode => format!(
                        "mytokens put {}<name> --set \"{}={harness}\" --fields \"{}\"",
                        account::PREFIX,
                        account::HARNESS,
                        account::CLAUDE_TOKEN
                    ),
                    _ => format!(
                        "mytokens put {}<name> --set \"{}={harness}\" --set \"{}=$(hostname -s)\"",
                        account::PREFIX,
                        account::HARNESS,
                        account::MACHINE
                    ),
                },
            )
        })
        .collect()
}

/// Every account mytokens holds, as `plan` and the panel show it `now`:
/// {name, harness, windows, rests_until, running, sessions, takes_work,
/// why}, and each secret of the prefix that is no account, with why.
pub(crate) fn json(now: u64) -> Value {
    let listed = listed();
    let listed = match listed.as_ref() {
        Ok(listed) => listed,
        Err(why) => return json!({"error": why, "accounts": [], "refused": []}),
    };
    let book = read_book();
    let here = account::this_machine().unwrap_or_default();
    let home = user_home().unwrap_or_default();
    // With no account there is nothing to count workers on: orca is not
    // asked, so a `plan` refused for a missing tool asks orca nothing more.
    let running = match listed.accounts.is_empty() {
        true => BTreeMap::new(),
        false => running(),
    };
    let limit = limit();
    let mut shown = vec![];
    let mut harnesses: Vec<Harness> = listed.accounts.iter().map(|a| a.harness).collect();
    harnesses.dedup();
    for harness in Harness::ALL.into_iter().filter(|h| harnesses.contains(h)) {
        for standing in standings_of(listed, &book, harness, &here, &home, &running) {
            let why = standing.refused(limit, now).map(|why| why.said(now));
            let windows: Vec<Value> = standing
                .current(now)
                .into_iter()
                .map(|window| {
                    json!({"window": window.name, "used": window.used,
                        "resets_at": window.resets_at,
                        "resets_in": window.resets_at.saturating_sub(now)})
                })
                .collect();
            shown.push(json!({
                "name": standing.name,
                "harness": standing.harness,
                "windows": windows,
                "rests_until": standing.rests_until.filter(|until| *until > now),
                "running": standing.running,
                "sessions": standing.sessions,
                "takes_work": why.is_none(),
                "why": why,
            }));
        }
    }
    let refused: Vec<Value> = listed
        .refused
        .iter()
        .map(|(service, why)| json!({"secret": service, "why": why}))
        .collect();
    json!({"limit_percent": limit, "accounts": shown, "refused": refused})
}

#[cfg(test)]
mod tests {
    use super::*;
    use afkconvoy_core::harness::account::{Source, read};

    const NOW: u64 = 1_800_000_000;

    struct Held(Vec<(&'static str, Value)>);

    impl Source for Held {
        fn list(&self) -> Result<String, String> {
            Ok(self.0.iter().map(|(service, _)| format!("{service}/default\tprofile\n")).collect())
        }
        fn get(&self, service: &str) -> Result<String, String> {
            let found = self.0.iter().find(|(s, _)| *s == service).ok_or("no secret")?;
            Ok(json!({"fields": found.1}).to_string())
        }
    }

    fn two_claude_accounts() -> Listed {
        let held = Held(vec![
            (
                "agent-account-busy",
                json!({"HARNESS": "claude-code", "CLAUDE_CODE_OAUTH_TOKEN": "sk-busy"}),
            ),
            (
                "agent-account-idle",
                json!({"HARNESS": "claude-code", "CLAUDE_CODE_OAUTH_TOKEN": "sk-idle"}),
            ),
        ]);
        let read = read(&held).unwrap();
        Listed {
            accounts: read.accounts,
            refused: read.refused,
            program: PathBuf::from("/bin/mytokens"),
        }
    }

    fn five_hour(used: u64) -> Vec<Window> {
        vec![Window { name: "five_hour".into(), used, resets_at: NOW + 3600 }]
    }

    #[test]
    fn the_next_worker_starts_on_the_account_not_at_ninety_percent_with_its_token() {
        let listed = two_claude_accounts();
        let mut book = Book::default();
        observe(&mut book, "busy", NOW - 60, five_hour(92));
        observe(&mut book, "idle", NOW - 60, five_hour(30));
        let standings = standings_of(
            &listed,
            &book,
            Harness::ClaudeCode,
            "studio",
            Path::new("/u"),
            &BTreeMap::new(),
        );
        let accounts = Accounts { harness: Harness::ClaudeCode, standings, limit: 90, now: NOW };
        let chosen = accounts.chosen().unwrap();
        assert_eq!(chosen.name, "idle");
        let account = listed.accounts.iter().find(|a| a.name == "idle").unwrap();
        let chosen = Chosen {
            name: account.name.clone(),
            service: format!("{}{}", account::PREFIX, account.name),
            credential: account.credential.clone(),
            program: listed.program.clone(),
        };
        let line = chosen.line("claude");
        assert_eq!(
            line,
            "CLAUDE_CODE_OAUTH_TOKEN=\"$('/bin/mytokens' get 'agent-account-idle' --field \
             CLAUDE_CODE_OAUTH_TOKEN)\" && export CLAUDE_CODE_OAUTH_TOKEN && claude"
        );
        assert!(!line.contains("sk-idle"), "the line holds where the token is, never it");
    }

    /// A Codex account whose `primary` or `secondary` window is at 90% takes
    /// no new worker: the next starts on its other account, whose login is
    /// on this machine (sunfmin/control#1231).
    #[test]
    fn a_codex_account_at_ninety_percent_of_either_window_sends_the_next_to_the_other() {
        let held = Held(vec![
            ("agent-account-first", json!({"HARNESS": "codex", "MACHINE": "studio"})),
            ("agent-account-second", json!({"HARNESS": "codex", "MACHINE": "studio"})),
        ]);
        let read = read(&held).unwrap();
        let listed = Listed {
            accounts: read.accounts,
            refused: read.refused,
            program: PathBuf::from("/bin/mytokens"),
        };
        let home = tempfile::tempdir().unwrap();
        for name in ["first", "second"] {
            let login = home.path().join(account::HOMES).join(name).join("auth.json");
            std::fs::create_dir_all(login.parent().unwrap()).unwrap();
            std::fs::write(login, "{}").unwrap();
        }
        let window = |name: &str, used| Window { name: name.into(), used, resets_at: NOW + 3600 };
        for (busy, other) in [("first", "second"), ("second", "first")] {
            for full in ["primary", "secondary"] {
                let mut book = Book::default();
                let low = if full == "primary" { "secondary" } else { "primary" };
                observe(&mut book, busy, NOW - 60, vec![window(full, 90), window(low, 5)]);
                observe(&mut book, other, NOW - 60, vec![window("primary", 60)]);
                let standings = standings_of(
                    &listed,
                    &book,
                    Harness::Codex,
                    "studio",
                    home.path(),
                    &BTreeMap::new(),
                );
                let accounts = Accounts { harness: Harness::Codex, standings, limit: 90, now: NOW };
                assert_eq!(accounts.chosen().map(|c| c.name.as_str()), Some(other), "{full}");
            }
        }
    }

    #[test]
    fn with_every_account_at_its_limit_none_starts_and_each_says_why() {
        let listed = two_claude_accounts();
        let mut book = Book::default();
        observe(&mut book, "busy", NOW - 60, five_hour(92));
        observe(&mut book, "idle", NOW - 60, five_hour(95));
        let standings = standings_of(
            &listed,
            &book,
            Harness::ClaudeCode,
            "studio",
            Path::new("/u"),
            &BTreeMap::new(),
        );
        let accounts = Accounts { harness: Harness::ClaudeCode, standings, limit: 90, now: NOW };
        assert!(accounts.chosen().is_none());
        let said = none_said(&accounts);
        assert!(said.contains("`busy`: its `five_hour` window is 92% used"), "{said}");
        assert!(said.contains("its workers wait"), "{said}");
        let later = Accounts { now: NOW + 3600, ..accounts };
        assert!(later.chosen().is_some(), "a window that reset takes work again");
    }

    #[test]
    fn a_limit_met_rests_its_account_and_a_newer_record_wins() {
        let mut book = Book::default();
        met(&mut book, "grok", NOW, Some(NOW + 900), "429".into());
        met(&mut book, "grok", NOW - 10, None, "older".into());
        assert_eq!(book.accounts["grok"].limit.as_ref().map(|l| l.until), Some(NOW + 900));
        met(&mut book, "grok", NOW + 5, None, "again".into());
        assert_eq!(
            book.accounts["grok"].limit.as_ref().map(|l| l.until),
            Some(NOW + 5 + engine_account::REST)
        );
        observe(&mut book, "a", NOW, five_hour(10));
        observe(&mut book, "a", NOW - 1, five_hour(99));
        assert_eq!(book.accounts["a"].windows, five_hour(10), "an older record is not kept");
    }

    #[test]
    fn a_window_said_without_its_reset_is_left_out() {
        use afkconvoy_core::harness::window::Window as Said;
        let said = [
            Said {
                name: "five_hour".into(),
                used_percent: 91.6,
                minutes: None,
                resets_at: Some(9),
            },
            Said { name: "seven_day".into(), used_percent: 40.0, minutes: None, resets_at: None },
        ];
        assert_eq!(
            windows_of(&said),
            [Window { name: "five_hour".into(), used: 92, resets_at: 9 }]
        );
    }
}
