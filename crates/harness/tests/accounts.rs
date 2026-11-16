//! Engine accounts as another program reads them (docs/adr/0086): from a
//! program standing for mytokens, through the crate's public API alone.
//! A credential's value reaches the environment of the session started with
//! it, and nothing else: no file, no line printed.

use std::path::{Path, PathBuf};
use std::process::Command;

use afkconvoy_harness::Harness;
use afkconvoy_harness::account::{self, Credential, Mytokens, Session};

const TOKEN_A: &str = "sk-ant-oat01-first-account-value";
const TOKEN_B: &str = "sk-ant-oat01-second-account-value";

/// A fresh directory of this test's own.
fn scratch(name: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("afkconvoy-accounts-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Every file under `dir`, with what it holds.
fn files(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else { return found };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files(&path));
        } else {
            found.push((path.clone(), std::fs::read(&path).unwrap_or_default()));
        }
    }
    found.sort();
    found
}

/// A program answering as mytokens does, holding two Claude Code accounts and
/// a Codex account bound to the machine `mini`.
fn mytokens(dir: &Path) -> PathBuf {
    let program = dir.join("mytokens");
    let script = format!(
        r#"#!/bin/sh
case "$1 $2" in
"list ") printf 'bark/default\tstatic\tserver  — push\nagent-account-max-a/default\tprofile\tserver  — fields: HARNESS, CLAUDE_CODE_OAUTH_TOKEN\nagent-account-max-b/default\tprofile\tserver\nagent-account-pro/default\tprofile\tserver\n' ;;
"get agent-account-max-a") printf '{{"fields":{{"HARNESS":"claude-code","SESSIONS":"2","CLAUDE_CODE_OAUTH_TOKEN":"{TOKEN_A}"}},"level":"server"}}' ;;
"get agent-account-max-b") printf '{{"fields":{{"HARNESS":"claude-code","CLAUDE_CODE_OAUTH_TOKEN":"{TOKEN_B}"}},"level":"server"}}' ;;
"get agent-account-pro") printf '{{"fields":{{"HARNESS":"codex","MACHINE":"mini"}},"level":"server"}}' ;;
*) exit 1 ;;
esac
"#
    );
    std::fs::write(&program, script).unwrap();
    Command::new("chmod").arg("+x").arg(&program).status().unwrap();
    program
}

/// What the test below does, run in a process of its own so that what it
/// printed can be read: it reads the accounts, prints what each is as a
/// caller would log it, and starts a session's process on each token.
#[test]
fn reading_child() {
    let Ok(dir) = std::env::var("ACCOUNTS_TEST_DIR") else { return };
    let dir = PathBuf::from(dir);
    let read = account::read(&Mytokens { program: dir.join("mytokens") }).unwrap();
    println!("{read:?}");
    let home = dir.join("home");
    for found in &read.accounts {
        let session = found.session("mini", &home);
        println!("{found:?} {session:?} {:?}", found.login(&home));
        if let Session::Ready(env) = session {
            // The session: a process given the environment, saying whether
            // it holds a token without printing it.
            let mut command = Command::new("/bin/sh");
            command.arg("-c").arg(r#"test -n "$CLAUDE_CODE_OAUTH_TOKEN" && echo token-given"#);
            env.apply(&mut command);
            let ran = command.output().unwrap();
            println!("{}", String::from_utf8_lossy(&ran.stdout).trim());
        }
    }
}

#[test]
fn two_claude_code_accounts_give_two_environments_and_no_value_is_written_or_printed() {
    let dir = scratch("claude");
    let program = mytokens(&dir);
    let home = dir.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let before = files(&dir);

    let read = account::read(&Mytokens { program }).unwrap();
    assert!(read.refused.is_empty(), "{:?}", read.refused);
    let claude: Vec<_> =
        read.accounts.iter().filter(|a| a.harness == Harness::ClaudeCode).collect();
    assert_eq!(claude.len(), 2);
    let mut seen = Vec::new();
    for (found, token) in claude.iter().zip([TOKEN_A, TOKEN_B]) {
        assert!(matches!(found.credential, Credential::Token(_)));
        assert_eq!(found.machine(), None);
        // A token is used on any machine, beside a configuration of its own.
        let Session::Ready(env) = found.session("anywhere", &home) else {
            panic!("{found:?} is not ready")
        };
        let given = env.get("CLAUDE_CODE_OAUTH_TOKEN").unwrap();
        assert!(given.is_secret());
        assert_eq!(given.expose(), token);
        let config = env.get("CLAUDE_CONFIG_DIR").unwrap().expose().to_string();
        assert_eq!(config, home.join(".agent-accounts").join(&found.name).to_string_lossy());
        seen.push(config);
        assert_eq!(found.login(&home), None);
    }
    assert_ne!(seen[0], seen[1]);
    assert_eq!(claude[0].sessions.map(|n| n.get()), Some(2));
    assert_eq!(claude[1].sessions, None);

    // Nothing was written: no file came or changed under the test's
    // directory, the user's home among it.
    assert_eq!(files(&dir), before);

    // What a caller prints of them, and a session started on each, shows no
    // value; the session was given its token.
    let ran = Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "reading_child", "--nocapture", "--test-threads=1"])
        .env("ACCOUNTS_TEST_DIR", &dir)
        .output()
        .unwrap();
    let out =
        String::from_utf8_lossy(&ran.stdout).into_owned() + &String::from_utf8_lossy(&ran.stderr);
    assert!(ran.status.success(), "{out}");
    assert_eq!(out.matches("token-given").count(), 2, "{out}");
    assert!(out.contains("<secret>"), "{out}");
    for token in [TOKEN_A, TOKEN_B] {
        assert!(!out.contains(token), "{out}");
        for (path, held) in files(&dir) {
            assert!(!String::from_utf8_lossy(&held).contains(token) || path.ends_with("mytokens"));
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_codex_account_is_its_own_home_on_its_machine_only() {
    let dir = scratch("codex");
    let program = mytokens(&dir);
    let home = dir.join("home");
    let read = account::read(&Mytokens { program }).unwrap();
    let pro = read.accounts.iter().find(|a| a.name == "pro").unwrap();
    assert_eq!((pro.harness, pro.machine()), (Harness::Codex, Some("mini")));
    let own = home.join(".agent-accounts").join("pro");

    // Elsewhere, it says the machine it is bound to.
    assert_eq!(pro.session("macstudio", &home), Session::Elsewhere { machine: "mini".to_string() });

    // On it, with no login yet: the device-code login in its home.
    let line = format!(
        "mkdir -p '{0}' && export CODEX_HOME='{0}' && codex login --device-auth",
        own.display()
    );
    assert_eq!(pro.session("mini", &home), Session::LoggedOut { home: own.clone(), line });

    // Logged in there: its own CODEX_HOME, and no token.
    std::fs::create_dir_all(&own).unwrap();
    std::fs::write(own.join("auth.json"), "{}").unwrap();
    let Session::Ready(env) = pro.session("MINI", &home) else { panic!("not ready") };
    assert_eq!(env.vars.len(), 1);
    assert_eq!(env.get("CODEX_HOME").unwrap().expose(), own.to_string_lossy());
    let _ = std::fs::remove_dir_all(&dir);
}
