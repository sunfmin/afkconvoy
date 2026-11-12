//! The tmux of a Guild machine (sunfmin/control#753): Guild runs each of its
//! instances in a tmux server of its own, and `guildd install` records where
//! tmux is, so every Guild machine needs one. Guild keeps no second copy of
//! what a machine needs: the host kit installs it (control ADR 0009, ADR
//! 0010).
//!
//! A Guild machine is a host whose user has Guild's home, `~/.guild`. On one
//! with no tmux, giving the machine its setup installs it where a standard
//! account can: `pixi global install tmux` (conda-forge, into `~/.pixi/bin`),
//! or `brew install tmux` where Homebrew is at a prefix it has bottles for. A
//! Homebrew in `~/homebrew` has none for tmux's ncurses and openssl and would
//! build them from source, so it is not used. A tmux already there is left
//! alone.

use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context;
use serde::Serialize;

use crate::shell::{Shell, ask, word};

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
pub const PIXI_BIN: &str = ".pixi/bin";
/// The line that installs pixi itself, for an account that has none.
pub const PIXI_INSTALL: &str = "curl -fsSL https://pixi.sh/install.sh | sh";

/// Whether the host whose user's home is `home` is a Guild machine.
pub fn is_guild_machine(home: &Path) -> bool {
    home.join(GUILD_HOME).is_dir()
}

/// The tmux of this host: the one a login shell of its user finds, else
/// one in a place an install of it puts it under the home.
pub fn found(shell: &dyn Shell, home: &Path) -> Option<PathBuf> {
    on_login_path(shell, "tmux").or_else(|| in_home(home, "tmux"))
}

/// How tmux is installed on this host: the line that does it, or why no
/// line here can.
pub fn installer(shell: &dyn Shell, home: &Path) -> Result<String, String> {
    if let Some(pixi) = in_home(home, "pixi").or_else(|| on_login_path(shell, "pixi")) {
        return Ok(format!("{} global install tmux", word(&pixi.display().to_string())));
    }
    if let Some(brew) = on_login_path(shell, "brew") {
        let line = format!("{} --prefix", word(&brew.display().to_string()));
        if let Ok(prefix) = ask(shell, &line, LOOK_TIMEOUT)
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

/// What giving a machine its tmux did, where there was something to do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum Tmux {
    /// tmux was installed at `installed`, by the line `by`.
    Installed { installed: PathBuf, by: String },
    /// It was not: `error` says why. `pixi_first` where no line on this host
    /// can install it, so pixi is to be installed first (`PIXI_INSTALL`).
    Failed {
        error: String,
        #[serde(skip)]
        pixi_first: bool,
    },
}

/// Installs tmux on the host whose user's home is `home` where it is a Guild
/// machine with none → what was done; `None` where nothing was to be done —
/// not a Guild machine, or its tmux already there and left alone.
pub fn give(shell: &dyn Shell, home: &Path) -> Option<Tmux> {
    if !is_guild_machine(home) || found(shell, home).is_some() {
        return None;
    }
    let line = match installer(shell, home) {
        Ok(line) => line,
        Err(why) => return Some(Tmux::Failed { error: why, pixi_first: true }),
    };
    let installed = ask(shell, &line, INSTALL_TIMEOUT).and_then(|_| {
        found(shell, home).with_context(|| format!("`{line}` ended and no tmux is found after it"))
    });
    Some(match installed {
        Ok(tmux) => Tmux::Installed { installed: tmux, by: line },
        Err(error) => Tmux::Failed {
            error: format!("{error:#}"),
            pixi_first: installer(shell, home).is_err(),
        },
    })
}

/// How the host whose user's home is `home` differs from what a Guild machine
/// needs: why it has no tmux, where it is a Guild machine with none.
pub fn missing(shell: &dyn Shell, home: &Path) -> Option<String> {
    if !is_guild_machine(home) || found(shell, home).is_some() {
        return None;
    }
    Some(format!(
        "{} is there, and neither a login shell's PATH nor {} holds a tmux; Guild runs each \
         instance in a tmux server of its own, and `guildd install` fails without one",
        home.join(GUILD_HOME).display(),
        home.join(PIXI_BIN).display()
    ))
}

/// Where a login shell of the host's user finds `program`, as the PATH it
/// gives a command typed there.
fn on_login_path(shell: &dyn Shell, program: &str) -> Option<PathBuf> {
    let printed = ask(shell, &format!("command -v {program}"), LOOK_TIMEOUT).ok()?;
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
