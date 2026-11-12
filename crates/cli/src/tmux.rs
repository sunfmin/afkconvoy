//! The tmux of a Guild machine (sunfmin/control#753): Guild runs each of its
//! instances in a tmux server of its own, and `guildd install` records where
//! tmux is, so every Guild machine needs one. Guild keeps no second copy of
//! what a machine needs: the host kit installs it (ADR-0066).
//!
//! A Guild machine is a host whose user has Guild's home, `~/.guild`. On one
//! with no tmux, writing the host kit installs it where a standard account
//! can: `pixi global install tmux` (conda-forge, into `~/.pixi/bin`), or
//! `brew install tmux` where Homebrew is at a prefix it has bottles for. A
//! Homebrew in `~/homebrew` has none for tmux's ncurses and openssl and would
//! build them from source, so it is not used. A tmux already there is left
//! alone. `doctor` names a Guild machine that has none, with the line that
//! installs it.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

/// Guild's home under the home of the host's user: a host that has it is a
/// Guild machine.
pub const GUILD_HOME: &str = ".guild";
/// How long an install of tmux may take: a download, nothing is built.
const INSTALL_TIMEOUT: Duration = Duration::from_secs(900);
/// How long a login shell is given to say where a command is.
const LOOK_TIMEOUT: Duration = Duration::from_secs(20);
/// The prefixes Homebrew has bottles for, so its tmux is poured, not built.
const BOTTLED: [&str; 2] = ["/opt/homebrew", "/usr/local"];
/// Where pixi puts the programs it installs globally, under the home.
const PIXI_BIN: &str = ".pixi/bin";
/// The line that installs pixi itself, for an account that has none.
const PIXI_INSTALL: &str = "curl -fsSL https://pixi.sh/install.sh | sh";
/// The line that installs tmux again: writing the host kit.
pub const FIX: &str = "afkconvoy kit";

/// Whether this host is a Guild machine: its user has Guild's home.
pub fn is_guild_machine(home: &Path) -> bool {
    home.join(GUILD_HOME).is_dir()
}

/// The tmux of this host: the one a login shell of its user finds, else
/// one in a place an install of it puts it under the home.
pub fn found(home: &Path) -> Option<PathBuf> {
    on_login_path("tmux").or_else(|| in_home(home, "tmux"))
}

/// How tmux is installed on this host: the line that does it, or why no
/// line here can, and the line that makes one that can.
fn installer(home: &Path) -> Result<String, String> {
    if let Some(pixi) = in_home(home, "pixi").or_else(|| on_login_path("pixi")) {
        return Ok(format!("{} global install tmux", word(&pixi.display().to_string())));
    }
    if let Some(brew) = on_login_path("brew") {
        let line = format!("{} --prefix", word(&brew.display().to_string()));
        if let Ok(prefix) = ask(&line, LOOK_TIMEOUT)
            && BOTTLED.contains(&prefix.trim())
        {
            return Ok(format!("{} install tmux", word(&brew.display().to_string())));
        }
    }
    Err(format!(
        "neither pixi nor a Homebrew with bottles (at {}) is on this host, and a Homebrew in \
         ~/homebrew would build tmux's ncurses and openssl from source",
        BOTTLED.join(" or ")
    ))
}

/// Installs tmux on this host where it is a Guild machine with none → what
/// was done: null where nothing was to be done — not a Guild machine, or its
/// tmux already there and left alone — else `{installed, by}` or
/// `{error, fix}`. What failed fails no write of the host kit: `doctor`
/// names the Guild machine still without tmux.
pub fn give() -> Value {
    let Ok(home) = home() else { return Value::Null };
    if !is_guild_machine(&home) || found(&home).is_some() {
        return Value::Null;
    }
    let line = match installer(&home) {
        Ok(line) => line,
        Err(why) => {
            eprintln!("tmux was not installed on this Guild machine: {why}");
            return json!({"error": why, "fix": fix(&home)});
        }
    };
    eprintln!("installing tmux on this Guild machine: {line}");
    let installed = ask(&line, INSTALL_TIMEOUT).and_then(|_| {
        found(&home).with_context(|| format!("`{line}` ended and no tmux is found after it"))
    });
    match installed {
        Ok(tmux) => json!({"installed": tmux, "by": line}),
        Err(error) => {
            eprintln!("tmux was not installed on this Guild machine: {error:#}");
            json!({"error": format!("{error:#}"), "fix": fix(&home)})
        }
    }
}

/// The line that gives this host its tmux: writing the host kit where it
/// can install one, else installing pixi first.
fn fix(home: &Path) -> String {
    match installer(home) {
        Ok(_) => FIX.to_string(),
        Err(_) => format!("{PIXI_INSTALL} && {FIX}"),
    }
}

/// A Guild machine with no tmux, as a finding.
pub fn findings() -> Vec<crate::doctor::Finding> {
    let Ok(home) = home() else { return vec![] };
    if !is_guild_machine(&home) || found(&home).is_some() {
        return vec![];
    }
    let evidence = format!(
        "{} is there, and neither a login shell's PATH nor {} holds a tmux; Guild runs each \
         instance in a tmux server of its own, and `guildd install` fails without one",
        home.join(GUILD_HOME).display(),
        home.join(PIXI_BIN).display()
    );
    vec![crate::doctor::finding(
        crate::doctor::Kind::TmuxMissing,
        json!(["tmux-missing"]),
        "this Guild machine has no tmux".to_string(),
        evidence,
        fix(&home),
    )]
}

/// Where a login shell of the host's user finds `program`, as the PATH it
/// gives a command typed there.
fn on_login_path(program: &str) -> Option<PathBuf> {
    let printed = ask(&format!("command -v {program}"), LOOK_TIMEOUT).ok()?;
    let path = PathBuf::from(printed.lines().last()?.trim());
    path.is_absolute().then_some(path)
}

/// `program` where an install puts it under the home, where it may be run.
fn in_home(home: &Path, program: &str) -> Option<PathBuf> {
    [PIXI_BIN, "homebrew/bin", ".local/bin"].iter().map(|dir| home.join(dir).join(program)).find(
        |file| {
            std::fs::metadata(file)
                .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        },
    )
}

/// Runs `line` in the login shell of the host's user → what it printed,
/// where it exited zero.
fn ask(line: &str, timeout: Duration) -> Result<String> {
    match crate::launch::ask_host(line, timeout)? {
        Some((true, printed)) => Ok(printed),
        Some((false, printed)) => {
            let tail: Vec<&str> = printed.lines().rev().take(3).collect();
            let tail: Vec<&str> = tail.into_iter().rev().collect();
            bail!("`{line}` failed: {}", tail.join(" / "))
        }
        None => bail!("`{line}` did not end within {}s", timeout.as_secs()),
    }
}

fn word(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from).context("HOME is not set")
}
