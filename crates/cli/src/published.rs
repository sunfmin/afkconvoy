//! How a host runner learns of a release of afkconvoy it did not cut
//! (sunfmin/control#315). Only the host that cuts afkconvoy installs the
//! release it publishes; every other host's Homebrew learns of it only at
//! a `brew update`, which nothing ran. So the host runner started from a
//! Homebrew install looks at its tap on GitHub every `LOOKED_EVERY`, and
//! where the tap publishes a newer release than its own, upgrades it at the
//! next look: the file it was started from changes, and it moves onto it
//! between ticks as after any install (`host_runner::Watched`).
//!
//! The upgrade waits one look so that it never runs beside the cut's own:
//! the host that cuts installs what it published as its last step, minutes
//! after its tap has it.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use afkconvoy_core::host_runner::{Found, found};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::VERSION;
use crate::gh::GitHub;

/// How often the host runner looks at its tap for a newer release.
pub(crate) const LOOKED_EVERY: Duration = Duration::from_secs(30 * 60);

/// How long `brew update` and `brew upgrade` are given, each.
const BREW_WITHIN: Duration = Duration::from_secs(10 * 60);

/// What `brew` said as it upgraded, in the cache directory.
const BREW_LOG: &str = "brew-upgrade.log";

/// A release of afkconvoy newer than the host runner's, published to the tap
/// it was installed from, as the host runner's looks found it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Published {
    /// The formula, with its tap: `sunfmin/tap/afkconvoy`.
    pub formula: String,
    /// The newest release the tap publishes.
    pub version: String,
    /// The release Homebrew has installed where the host runner was started
    /// from, as the last look found it.
    pub installed: String,
    /// The host runner's own.
    pub from: String,
    /// When a look first found it, in epoch seconds.
    pub since: u64,
    /// Why the last upgrade to it failed, as `brew` said it; tried again at
    /// the next look.
    pub failed: Option<String>,
}

/// The release a Homebrew install of afkconvoy is of, and where its tap is.
#[derive(Debug, PartialEq, Eq)]
struct Keg {
    /// `<prefix>/bin/brew`.
    brew: PathBuf,
    /// The formula, with its tap: `sunfmin/tap/afkconvoy`.
    formula: String,
    /// The release installed.
    version: String,
    /// The tap's repo on GitHub: `sunfmin/homebrew-tap`.
    repo: String,
    /// The formula's file in it: `Formula/afkconvoy.rb`.
    file: String,
}

/// Looks at the tap of the afkconvoy at `path` now and every `LOOKED_EVERY`
/// after, on a thread of its own, keeping in `published` what it found and
/// saying in `say` what it did. Where `path` is no Homebrew install from a
/// tap, it looks no more: nothing published reaches that afkconvoy.
pub(crate) fn learn(
    path: Option<PathBuf>,
    published: Arc<Mutex<Option<Published>>>,
    say: impl Fn(&str) + Send + 'static,
) {
    let Some(path) = path else { return };
    std::thread::spawn(move || {
        loop {
            let Some(keg) = keg_at(&path) else { return };
            let kept = published.lock().unwrap_or_else(|e| e.into_inner()).clone();
            let found = looked(&keg, kept.as_ref(), crate::clock(), &say);
            *published.lock().unwrap_or_else(|e| e.into_inner()) = found;
            std::thread::sleep(LOOKED_EVERY);
        }
    });
}

/// One look at `keg`'s tap, `kept` being what the look before found → what
/// this one found, the upgrade made where it was due.
fn looked(keg: &Keg, kept: Option<&Published>, now: u64, say: &impl Fn(&str)) -> Option<Published> {
    let newest = match newest(keg) {
        Ok(newest) => newest,
        Err(error) => {
            say(&format!(
                "the newest afkconvoy {} publishes could not be read: {error:#} — the host \
                 runner looks again in {}s",
                keg.formula,
                LOOKED_EVERY.as_secs()
            ));
            return kept.cloned();
        }
    };
    let (mut found, upgrade) = next(keg, kept, &newest, now)?;
    let formula = &keg.formula;
    if kept.is_none_or(|kept| kept.version != newest) {
        say(&format!(
            "afkconvoy {newest} is published to {formula}, newer than this host runner's, \
             {VERSION}: it is installed at the host runner's next look, within {}s, unless \
             installed before then",
            LOOKED_EVERY.as_secs()
        ));
    }
    if upgrade {
        found.failed = upgraded(keg).err().map(|error| format!("{error:#}"));
        match &found.failed {
            None => say(&format!(
                "afkconvoy {newest} installed by `brew upgrade {formula}`: the host runner moves \
                 onto it between ticks"
            )),
            Some(why) => say(&format!(
                "afkconvoy {newest} could not be installed by `brew upgrade {formula}`: {why} — \
                 the host runner tries again in {}s",
                LOOKED_EVERY.as_secs()
            )),
        }
    }
    Some(found)
}

/// What a look that found `newest` published keeps, and whether it upgrades:
/// nothing where the host runner's release is as new; otherwise the upgrade,
/// where the look before found it published too and Homebrew has not
/// installed it since.
fn next(keg: &Keg, kept: Option<&Published>, newest: &str, now: u64) -> Option<(Published, bool)> {
    if found(VERSION, newest) != Found::Newer {
        return None;
    }
    let before = kept.filter(|kept| kept.version == newest);
    let found = Published {
        formula: keg.formula.clone(),
        version: newest.to_string(),
        installed: keg.version.clone(),
        from: VERSION.to_string(),
        since: before.map_or(now, |before| before.since),
        failed: before.and_then(|before| before.failed.clone()),
    };
    let upgrade = before.is_some() && found_installed(&keg.version, newest);
    Some((found, upgrade))
}

/// Whether `newest` is newer than the release Homebrew has `installed`.
fn found_installed(installed: &str, newest: &str) -> bool {
    found(installed, newest) == Found::Newer
}

/// The Homebrew install `path` is, a link into a keg as Homebrew keeps it
/// (`<prefix>/Cellar/<name>/<version>/bin/afkconvoy`), with the tap its
/// receipt names; `None` where it is not one, or of no tap of a GitHub repo.
fn keg_at(path: &Path) -> Option<Keg> {
    let real = std::fs::canonicalize(path).ok()?;
    let parts: Vec<_> = real.components().collect();
    let cellar = parts.iter().rposition(|part| part.as_os_str() == "Cellar")?;
    let (name, version) = (parts.get(cellar + 1)?, parts.get(cellar + 2)?);
    let prefix: PathBuf = parts[..cellar].iter().collect();
    let keg = prefix.join("Cellar").join(name).join(version);
    let receipt = std::fs::read_to_string(keg.join("INSTALL_RECEIPT.json")).ok()?;
    let receipt: Value = serde_json::from_str(&receipt).ok()?;
    let name = name.as_os_str().to_str()?;
    let version = version.as_os_str().to_str()?;
    of_receipt(&receipt, &prefix, name, version)
}

/// The keg of the formula `name` at `version` under `prefix`, its receipt
/// saying the tap and the formula's file in the tap's clone.
fn of_receipt(receipt: &Value, prefix: &Path, name: &str, version: &str) -> Option<Keg> {
    let tap = receipt["source"]["tap"].as_str()?;
    let (owner, tap_name) = tap.split_once('/')?;
    let clone = format!("/Taps/{owner}/homebrew-{tap_name}/");
    let at = receipt["source"]["path"].as_str()?;
    let file = &at[at.find(&clone)? + clone.len()..];
    Some(Keg {
        brew: prefix.join("bin/brew"),
        formula: format!("{tap}/{name}"),
        version: version.to_string(),
        repo: format!("{owner}/homebrew-{tap_name}"),
        file: file.to_string(),
    })
}

/// The release the formula on `keg`'s tap on GitHub says it installs.
fn newest(keg: &Keg) -> Result<String> {
    let at = format!("repos/{}/contents/{}", keg.repo, keg.file);
    let text =
        GitHub::in_no_checkout().read(&["api", &at, "-H", "Accept: application/vnd.github.raw"])?;
    version_in(&text).with_context(|| format!("{} on {} says no `version`", keg.file, keg.repo))
}

/// The release a formula's `version "0.5.49"` line says.
fn version_in(formula: &str) -> Option<String> {
    formula.lines().find_map(|line| {
        let quoted = line.trim().strip_prefix("version \"")?;
        Some(quoted.split('"').next()?.to_string())
    })
}

/// `brew update`, then `brew upgrade` of `keg`'s formula, each given
/// `BREW_WITHIN`, what they say kept in `BREW_LOG`.
fn upgraded(keg: &Keg) -> Result<()> {
    let log = crate::copies::cache()?.join(BREW_LOG);
    let file =
        std::fs::File::create(&log).with_context(|| format!("cannot write {}", log.display()))?;
    for args in [&["update", "--quiet"][..], &["upgrade", &keg.formula]] {
        let said = format!("`brew {}`", args.join(" "));
        let mut command = Command::new(&keg.brew);
        command
            .args(args)
            .env("HOMEBREW_NO_AUTO_UPDATE", "1")
            .stdin(Stdio::null())
            .stdout(file.try_clone()?)
            .stderr(file.try_clone()?);
        let mut child = crate::process::spawn(&mut command)
            .with_context(|| format!("{said} could not be started"))?;
        let until = Instant::now() + BREW_WITHIN;
        let status = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= until {
                let _ = child.kill();
                let _ = child.wait();
                bail!(
                    "{said} ran past {}s and was ended: {}",
                    BREW_WITHIN.as_secs(),
                    log.display()
                );
            }
            std::thread::sleep(Duration::from_millis(200));
        };
        if !status.success() {
            bail!("{said} ended {status}: {}", log.display());
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn keg(version: &str) -> Keg {
        Keg {
            brew: "/opt/homebrew/bin/brew".into(),
            formula: "sunfmin/tap/afkconvoy".into(),
            version: version.into(),
            repo: "sunfmin/homebrew-tap".into(),
            file: "Formula/afkconvoy.rb".into(),
        }
    }

    /// A release past this build's.
    fn newer() -> String {
        let mut parts: Vec<u64> =
            VERSION.split('+').next().unwrap().split('.').map(|n| n.parse().unwrap()).collect();
        parts[1] += 1;
        parts.iter().map(u64::to_string).collect::<Vec<_>>().join(".")
    }

    #[test]
    fn a_receipt_names_the_tap_and_the_formula_s_file() {
        let receipt = json!({"source": {
            "tap": "sunfmin/tap",
            "path": "/opt/homebrew/Library/Taps/sunfmin/homebrew-tap/Formula/afkconvoy.rb",
        }});
        let found = of_receipt(&receipt, Path::new("/opt/homebrew"), "afkconvoy", "0.5.43");
        assert_eq!(found, Some(keg("0.5.43")));
        let untapped = json!({"source": {"tap": null, "path": "/tmp/afkconvoy.rb"}});
        assert_eq!(of_receipt(&untapped, Path::new("/opt/homebrew"), "afkconvoy", "1.0.0"), None);
    }

    #[test]
    fn a_build_that_is_no_keg_learns_nothing() {
        assert_eq!(keg_at(&std::env::current_exe().unwrap()), None);
    }

    #[test]
    fn a_formula_says_its_version() {
        let formula = "class Afkconvoy < Formula\n  desc \"x\"\n  version \"0.5.49\"\n";
        assert_eq!(version_in(formula).as_deref(), Some("0.5.49"));
        assert_eq!(version_in("class X < Formula\nend\n"), None);
    }

    #[test]
    fn the_upgrade_waits_one_look_and_none_runs_for_what_is_not_newer() {
        let newer = newer();
        let (first, upgrade) = next(&keg(VERSION), None, &newer, 100).unwrap();
        assert!(!upgrade, "the first look only notes it: a cut may be installing it");
        assert_eq!((first.since, first.installed.as_str()), (100, VERSION));
        let (second, upgrade) = next(&keg(VERSION), Some(&first), &newer, 1900).unwrap();
        assert!(upgrade, "the next look installs it");
        assert_eq!(second.since, 100, "since stays the first look's");
        let (_, upgrade) = next(&keg(&newer), Some(&first), &newer, 1900).unwrap();
        assert!(!upgrade, "installed already: the host runner moves onto it");
        assert_eq!(next(&keg(VERSION), Some(&first), VERSION, 1900), None);
    }
}
