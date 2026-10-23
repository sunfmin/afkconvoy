//! The host's worker launch commands. This file belongs to the host user,
//! outside every checkout; only an explicit terminal confirmation writes it.
use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::PathBuf;

use afkconvoy_core::harness::Harness;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Kit {
    pub commands: BTreeMap<String, String>,
}

pub fn path() -> Result<PathBuf> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config")))
        .context(
            "cannot find the host kit: set HOME or XDG_CONFIG_HOME, then run `afkconvoy kit`",
        )?;
    Ok(root.join("afkconvoy/kit.json"))
}

pub fn load() -> Result<Kit> {
    let file = path()?;
    let text = std::fs::read_to_string(&file).with_context(|| {
        format!("cannot read the host kit at {}: run `afkconvoy kit`", file.display())
    })?;
    let kit: Kit = serde_json::from_str(&text).with_context(|| {
        format!("cannot read the host kit at {}: run `afkconvoy kit`", file.display())
    })?;
    for name in kit.commands.keys() {
        if Harness::read(name).is_none() {
            bail!("invalid host kit entry {name:?}: run `afkconvoy kit`");
        }
    }
    Ok(kit)
}

pub fn show() -> Value {
    let file = path().map(|path| path.display().to_string()).ok();
    match load() {
        Ok(kit) => json!({"path": file, "commands": kit.commands}),
        Err(error) => json!({"path": file, "error": format!("{error:#}")}),
    }
}

/// Ask for the kit only for `run`, and only when none has been written.
pub fn ensure() -> Result<()> {
    if !path()?.exists() {
        configure()?;
    }
    load().map(|_| ())
}

pub fn configure() -> Result<Value> {
    if !std::io::stdin().is_terminal() || !std::io::stderr().is_terminal() {
        bail!(
            "the host kit needs a terminal: run `afkconvoy kit` at a terminal before starting the fleet"
        );
    }
    let file = path()?;
    let current = if file.exists() { load()? } else { Kit::default() };
    let mut proposed = current.commands;
    // Discover only on the first run. An omitted harness in an existing kit
    // was deliberately dropped, and must not silently be added again.
    if !file.exists() {
        for harness in Harness::ALL {
            if crate::launch::on_path(harness.program()) {
                proposed.insert(harness.key().to_string(), harness.stock().to_string());
            }
        }
    }
    let mut input = std::io::stdin().lock();
    let mut out = std::io::stderr();
    let Some(kit) = edit(proposed, &mut input, &mut out)? else {
        return Ok(json!({"path": file, "written": false}));
    };
    let parent = file.parent().context("the host kit needs a parent directory")?;
    std::fs::create_dir_all(parent)?;
    let temporary = file.with_extension(format!("{}.tmp", std::process::id()));
    std::fs::write(&temporary, serde_json::to_vec_pretty(&kit)?)?;
    std::fs::rename(&temporary, &file)?;
    Ok(json!({"path": file, "written": true, "commands": kit.commands}))
}

fn reply(input: &mut impl BufRead, out: &mut impl Write, prompt: &str) -> Result<Option<String>> {
    write!(out, "{prompt}")?;
    out.flush()?;
    let mut line = String::new();
    if input.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_string()))
}

/// Show every known harness so a previously dropped one can be added, too.
fn edit(
    mut commands: BTreeMap<String, String>,
    input: &mut impl BufRead,
    out: &mut impl Write,
) -> Result<Option<Kit>> {
    writeln!(out, "Host kit: keep with Enter, type a command to edit/add, or - to drop.")?;
    for harness in Harness::ALL {
        let command = commands.get(harness.key()).map(String::as_str).unwrap_or("absent");
        let prompt = format!("{}: {command}\n> ", harness.key());
        let Some(answer) = reply(input, out, &prompt)? else { return Ok(None) };
        if answer == "-" {
            commands.remove(harness.key());
        } else if !answer.trim().is_empty() {
            commands.insert(harness.key().to_string(), answer);
        }
    }
    writeln!(out, "{}", serde_json::to_string_pretty(&commands)?)?;
    let confirmed = reply(input, out, "Write this host kit? [y/N] ")?;
    Ok(confirmed
        .filter(|answer| answer.eq_ignore_ascii_case("y") || answer.eq_ignore_ascii_case("yes"))
        .map(|_| Kit { commands }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing_keeps_edits_drops_and_confirms_the_whole_kit() {
        let commands = [("claude-code".into(), "old".into()), ("pi".into(), "pi".into())].into();
        let mut shown = vec![];
        let kit =
            edit(commands, &mut &b"\n-\nmycodex --yolo\n\ny\n"[..], &mut shown).unwrap().unwrap();
        assert_eq!(
            kit.commands,
            [("claude-code".into(), "old".into()), ("codex".into(), "mycodex --yolo".into())]
                .into()
        );
        assert!(String::from_utf8(shown).unwrap().contains("pi: pi"));
    }

    #[test]
    fn eof_or_declining_writes_nothing() {
        for replies in ["", "\n\n\n\n", "\n\n\n\nn\n"] {
            assert!(edit(BTreeMap::new(), &mut replies.as_bytes(), &mut vec![]).unwrap().is_none());
        }
    }
}
