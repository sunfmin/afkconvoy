//! The tmux of a Guild machine (sunfmin/control#753): Guild runs each of its
//! instances in a tmux server of its own, and `guildd install` records where
//! tmux is, so every Guild machine needs one. Guild keeps no second copy of
//! how tmux is installed: the host kit installs it (docs/adr/0062). It is all
//! of the crate Guild uses (sunfmin/control#780).
//!
//! A Guild machine is a host whose user has Guild's home, `~/.guild`. On one
//! with no tmux, giving the machine its setup installs it where a standard
//! account can: `pixi global install tmux` (conda-forge, into `~/.pixi/bin`),
//! or `brew install tmux` where Homebrew is at a prefix it has bottles for. A
//! Homebrew in `~/homebrew` has none for tmux's ncurses and openssl and would
//! build them from source, so it is not used. Where neither is there, the
//! host kit installs pixi itself first (sunfmin/control#816): the release of
//! the version pinned here (`PIXI_VERSION`), checked against the `.sha256`
//! published next to it, unpacked into `~/.pixi/bin`; no shell profile is
//! touched. A tmux already there is left alone.

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
/// The version of pixi the host kit installs where an account has none.
pub const PIXI_VERSION: &str = "0.84.1";
/// Where pixi's releases are: `<releases>/v<version>/pixi-<target>.tar.gz`,
/// with its checksum at that name and `.sha256`.
const PIXI_RELEASES: &str = "https://github.com/prefix-dev/pixi/releases/download";
/// Where a download of pixi is kept until it is checked, under the home.
const PIXI_DOWNLOAD: &str = ".pixi/download";

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
    /// It was not: `error` says why.
    Failed { error: String },
}

/// Installs tmux on the host whose user's home is `home` where it is a Guild
/// machine with none → what was done; `None` where nothing was to be done —
/// not a Guild machine, or its tmux already there and left alone. Where no
/// line there can install it, pixi is installed first, from `PIXI_RELEASES`.
pub fn give(shell: &dyn Shell, home: &Path) -> Option<Tmux> {
    give_from(shell, home, PIXI_RELEASES)
}

/// `give`, with pixi's releases at `releases` (`PIXI_RELEASES`'s layout).
pub fn give_from(shell: &dyn Shell, home: &Path, releases: &str) -> Option<Tmux> {
    if !is_guild_machine(home) || found(shell, home).is_some() {
        return None;
    }
    let line = match installer(shell, home) {
        Ok(line) => line,
        Err(why) => match install_pixi(shell, home, releases) {
            Ok(_) => match installer(shell, home) {
                Ok(line) => line,
                Err(why) => return Some(Tmux::Failed { error: why }),
            },
            Err(error) => {
                let error = format!("{why}; and pixi could not be installed: {error:#}");
                return Some(Tmux::Failed { error });
            }
        },
    };
    let installed = ask(shell, &line, INSTALL_TIMEOUT).and_then(|_| {
        found(shell, home).with_context(|| format!("`{line}` ended and no tmux is found after it"))
    });
    Some(match installed {
        Ok(tmux) => Tmux::Installed { installed: tmux, by: line },
        Err(error) => Tmux::Failed { error: format!("{error:#}") },
    })
}

/// The release target of pixi for the machine this runs on.
fn pixi_target() -> anyhow::Result<String> {
    let arch = std::env::consts::ARCH;
    match (std::env::consts::OS, arch) {
        ("macos", "aarch64" | "x86_64") => Ok(format!("{arch}-apple-darwin")),
        ("linux", "aarch64" | "x86_64") => Ok(format!("{arch}-unknown-linux-musl")),
        (os, arch) => anyhow::bail!("pixi has no release for {arch} {os}"),
    }
}

/// Installs pixi `PIXI_VERSION` from `releases` into `~/.pixi/bin` of the
/// home `home` → where it is. The download is checked against the checksum
/// published next to it first: where they differ, nothing is installed.
pub fn install_pixi(shell: &dyn Shell, home: &Path, releases: &str) -> anyhow::Result<PathBuf> {
    let name = format!("pixi-{}.tar.gz", pixi_target()?);
    let url = format!("{}/v{PIXI_VERSION}/{name}", releases.trim_end_matches('/'));
    let staging = home.join(PIXI_DOWNLOAD);
    let _ = std::fs::remove_dir_all(&staging);
    let unpacked = staging.join("unpacked");
    std::fs::create_dir_all(&unpacked)
        .with_context(|| format!("{} cannot be made", unpacked.display()))?;
    let installed = (|| {
        let archive = staging.join(&name);
        let sum = staging.join(format!("{name}.sha256"));
        for (to, from) in [(&archive, url.clone()), (&sum, format!("{url}.sha256"))] {
            let to = word(&to.display().to_string());
            ask(shell, &format!("curl -fsSL -o {to} {}", word(&from)), INSTALL_TIMEOUT)?;
        }
        let published = std::fs::read_to_string(&sum)
            .with_context(|| format!("{} cannot be read", sum.display()))?;
        let published = checksum(&published)
            .with_context(|| format!("{url}.sha256 holds no SHA-256 checksum"))?;
        let archive_word = word(&archive.display().to_string());
        let computed = ask(shell, &format!("shasum -a 256 {archive_word}"), LOOK_TIMEOUT)?;
        let computed = checksum(&computed)
            .with_context(|| format!("`shasum` printed no checksum of {}", archive.display()))?;
        anyhow::ensure!(
            computed == published,
            "the download {url} has checksum {computed}, not the {published} published next \
             to it; nothing was installed"
        );
        let into = word(&unpacked.display().to_string());
        ask(shell, &format!("tar -xzf {archive_word} -C {into}"), INSTALL_TIMEOUT)?;
        let pixi = unpacked.join("pixi");
        anyhow::ensure!(runnable(&pixi), "{url} holds no program `pixi`");
        let bin = home.join(PIXI_BIN);
        std::fs::create_dir_all(&bin)
            .with_context(|| format!("{} cannot be made", bin.display()))?;
        let to = bin.join("pixi");
        std::fs::rename(&pixi, &to)
            .with_context(|| format!("{} cannot be written", to.display()))?;
        Ok(to)
    })();
    let _ = std::fs::remove_dir_all(&staging);
    // `~/.pixi` itself where nothing else is in it: a failed install leaves nothing.
    let _ = staging.parent().map(std::fs::remove_dir);
    installed
}

/// The SHA-256 checksum a line of `sha256sum` or `shasum` opens with, in
/// lowercase.
fn checksum(text: &str) -> Option<String> {
    let first = text.split_whitespace().next()?.to_ascii_lowercase();
    (first.len() == 64 && first.bytes().all(|b| b.is_ascii_hexdigit())).then_some(first)
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
    [PIXI_BIN, "homebrew/bin", ".local/bin"]
        .iter()
        .map(|dir| home.join(dir).join(program))
        .find(|file| runnable(file))
}

/// Whether `file` is a file that may be run.
fn runnable(file: &Path) -> bool {
    std::fs::metadata(file)
        .is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_checksum_is_the_first_word_of_a_line_and_hex_alone() {
        let line = format!("{}  pixi-aarch64-apple-darwin.tar.gz\n", "3B".repeat(32));
        assert_eq!(checksum(&line), Some("3b".repeat(32)));
        assert_eq!(checksum("not a checksum"), None);
        assert_eq!(checksum(""), None);
        assert_eq!(checksum(&"g".repeat(64)), None);
    }
}
