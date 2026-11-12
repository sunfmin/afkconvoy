//! The operator's skill list, installed through `npx skills` for each
//! harness's agent (`Harness::skills_agent`) into the shared
//! `~/.agents/skills`, with every installed skill of that agent the list does
//! not name removed. The installed copies are read-only between syncs; a
//! sync opens them for its own writes and closes them again.

use std::collections::BTreeSet;
use std::path::Path;
use std::time::Duration;

use afkconvoy_harness::Harness;
use anyhow::{Context, Result};
use serde::Deserialize;

use crate::shell::{Shell, ask, word};

/// How long one `npx skills add` or `remove` may take: it fetches a source.
const SYNC_TIMEOUT: Duration = Duration::from_secs(600);
/// How long one `npx skills ls` may take: it reads the host alone.
const LIST_TIMEOUT: Duration = Duration::from_secs(60);
/// Where the shared skill copies are, under the home.
pub const COPIES: &str = ".agents/skills";

/// One line of a skill list: a source and the skills it gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub source: String,
    pub skills: Which,
}

/// The skills one source gives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Which {
    /// `*`: every skill the source ships.
    Every,
    Named(Vec<String>),
}

/// Reads a skill list: one line per source, `<source> <skill>...` or
/// `<source> *`; blank lines and `#` lines say nothing.
pub fn parse(text: &str) -> Result<Vec<Entry>, String> {
    let mut entries = vec![];
    for (at, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let source = words.next().unwrap_or_default().to_string();
        let skills: Vec<String> = words.map(str::to_string).collect();
        let skills = match skills.as_slice() {
            [] => return Err(format!("line {}: `{source}` names no skill, nor `*`", at + 1)),
            [every] if every == "*" => Which::Every,
            named if named.iter().any(|skill| skill == "*") => {
                return Err(format!("line {}: `*` stands alone after its source", at + 1));
            }
            named => Which::Named(named.to_vec()),
        };
        entries.push(Entry { source, skills });
    }
    Ok(entries)
}

/// One skill installed for an agent, as `npx skills ls --json` says it.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Installed {
    pub name: String,
    /// Where it came from; `None` for one `npx skills` does not track.
    #[serde(default)]
    pub source: Option<String>,
    /// The agents it is installed for: empty where it is a shared copy the
    /// agent asked about was not given.
    #[serde(default)]
    pub agents: Vec<String>,
}

/// What an agent lacks of `entries`, and holds beyond them, of what is
/// `installed` for it: the names of each. A skill `npx skills` does not
/// track is no one's to remove, and a `*` source lacks something only where
/// none of its skills is installed.
pub fn drift(entries: &[Entry], installed: &[Installed]) -> (Vec<String>, Vec<String>) {
    let given: Vec<&Installed> =
        installed.iter().filter(|skill| !skill.agents.is_empty()).collect();
    let has = |source: &str, name: Option<&str>| {
        given.iter().any(|skill| {
            skill.source.as_deref() == Some(source) && name.is_none_or(|name| skill.name == name)
        })
    };
    let mut missing = vec![];
    for entry in entries {
        match &entry.skills {
            Which::Every if !has(&entry.source, None) => {
                missing.push(format!("every skill of {}", entry.source));
            }
            Which::Every => {}
            Which::Named(names) => missing.extend(
                names.iter().filter(|name| !has(&entry.source, Some(name.as_str()))).cloned(),
            ),
        }
    }
    let declares = |skill: &Installed| {
        entries.iter().any(|entry| {
            skill.source.as_deref() == Some(entry.source.as_str())
                && match &entry.skills {
                    Which::Every => true,
                    Which::Named(names) => names.contains(&skill.name),
                }
        })
    };
    let extra = given
        .iter()
        .filter(|skill| skill.source.is_some() && !declares(skill))
        .map(|skill| skill.name.clone())
        .collect();
    (missing, extra)
}

/// Makes everything under the copy at `path` writable by its owner (`open`),
/// or by no one (`!open`). A link is left as it is.
fn set_mode(path: &Path, open: bool) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let meta = std::fs::symlink_metadata(path)?;
    if meta.file_type().is_symlink() {
        return Ok(());
    }
    let mode = meta.permissions().mode();
    let mode = if open { mode | 0o200 } else { mode & !0o222 };
    // A directory is opened before what is in it is read, and closed after.
    if open || !meta.is_dir() {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    if meta.is_dir() {
        for entry in std::fs::read_dir(path)? {
            set_mode(&entry?.path(), open)?;
        }
        if !open {
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
        }
    }
    Ok(())
}

/// `set_mode` of each copy in `copies`, leaving `copies` itself writable,
/// where a sync adds a new copy.
pub fn set_open(copies: &Path, open: bool) -> Result<()> {
    let Ok(entries) = std::fs::read_dir(copies) else { return Ok(()) };
    for entry in entries {
        let path = entry?.path();
        set_mode(&path, open)
            .with_context(|| format!("cannot change the mode of {}", path.display()))?;
    }
    Ok(())
}

/// `set_open` of the shared copies under `home`, and `set_mode` of each skill
/// `npx skills` tracks for one of `harnesses` that is a copy of its own in a
/// directory where that harness reads its user's skills (Claude Code's
/// `~/.claude/skills`): an agent installed for alone may be given a copy
/// there, where several are given links to the shared one. A skill there
/// that `npx skills` does not track is the user's own, and left as it is.
pub fn set_open_all(
    shell: &dyn Shell,
    home: &Path,
    harnesses: &[(Harness, &str)],
    open: bool,
) -> Result<()> {
    set_open(&home.join(COPIES), open)?;
    for (harness, agent) in harnesses {
        // An agent whose list cannot be read has no copy known to close.
        let names: Vec<String> = listed(shell, agent)
            .unwrap_or_default()
            .into_iter()
            .filter(|skill| skill.source.is_some())
            .map(|skill| skill.name)
            .collect();
        let dirs = harness.kept().host_user.iter().filter(|link| link.home.ends_with("skills"));
        for dir in dirs.filter(|link| link.home != COPIES) {
            for name in &names {
                let path = home.join(dir.home).join(name);
                if std::fs::symlink_metadata(&path).is_ok() {
                    set_mode(&path, open)
                        .with_context(|| format!("cannot change the mode of {}", path.display()))?;
                }
            }
        }
    }
    Ok(())
}

/// The `npx skills` line that installs `entry` for `agents`.
pub fn add_line(entry: &Entry, agents: &[&str]) -> String {
    let skills = match &entry.skills {
        Which::Every => vec![word("*")],
        Which::Named(names) => names.iter().map(|name| word(name)).collect(),
    };
    let agents: Vec<String> = agents.iter().map(|agent| word(agent)).collect();
    format!(
        "npx -y skills add {} -g -y -a {} -s {}",
        word(&entry.source),
        agents.join(" "),
        skills.join(" ")
    )
}

/// The `npx skills` line that removes `skills` from `agent`.
pub fn remove_line(agent: &str, skills: &[String]) -> String {
    let skills: Vec<String> = skills.iter().map(|skill| word(skill)).collect();
    format!("npx -y skills remove -g -y -a {} -s {}", word(agent), skills.join(" "))
}

/// The `npx skills` line that lists what is installed for `agent`.
pub fn list_line(agent: &str) -> String {
    format!("npx -y skills ls -g -a {} --json", word(agent))
}

/// What is installed for `agent`, as `npx skills ls` says it in `shell`.
pub fn listed(shell: &dyn Shell, agent: &str) -> Result<Vec<Installed>> {
    let line = list_line(agent);
    let printed = ask(shell, &line, LIST_TIMEOUT)?;
    let json = printed.find('[').map_or("", |at| &printed[at..]);
    serde_json::from_str(json.trim_end()).with_context(|| format!("`{line}` printed no list"))
}

/// Installs `entries` for `agents` in `shell`, and removes from each what
/// they do not name, with the copies under `home` open only while it does;
/// each line run is added to `synced`.
pub fn sync(
    shell: &dyn Shell,
    home: &Path,
    entries: &[Entry],
    agents: &[(Harness, &str)],
    synced: &mut Vec<String>,
) -> Result<()> {
    if agents.is_empty() {
        return Ok(());
    }
    set_open_all(shell, home, agents, true)?;
    let names: Vec<&str> = agents.iter().map(|(_, agent)| *agent).collect();
    let mut run = || -> Result<()> {
        for entry in entries {
            let line = add_line(entry, &names);
            ask(shell, &line, SYNC_TIMEOUT)?;
            synced.push(line);
        }
        for agent in names.iter().copied().collect::<BTreeSet<_>>() {
            let (_, extra) = drift(entries, &listed(shell, agent)?);
            if !extra.is_empty() {
                let line = remove_line(agent, &extra);
                ask(shell, &line, SYNC_TIMEOUT)?;
                synced.push(line);
            }
        }
        Ok(())
    };
    let ran = run();
    let closed = set_open_all(shell, home, agents, false);
    ran.and(closed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn installed(name: &str, source: Option<&str>, given: bool) -> Installed {
        Installed {
            name: name.into(),
            source: source.map(str::to_string),
            agents: if given { vec!["Codex".into()] } else { vec![] },
        }
    }

    #[test]
    fn a_skill_list_names_sources_and_their_skills_or_every_one() {
        let text = "# the list\n\nmattpocock/skills *\nsunfmin/product-team complexity-review  single-source-of-truth\n";
        assert_eq!(
            parse(text).unwrap(),
            vec![
                Entry { source: "mattpocock/skills".into(), skills: Which::Every },
                Entry {
                    source: "sunfmin/product-team".into(),
                    skills: Which::Named(vec![
                        "complexity-review".into(),
                        "single-source-of-truth".into()
                    ]),
                },
            ]
        );
        assert!(parse("sunfmin/mypython\n").unwrap_err().contains("line 1"));
        assert!(parse("a/b x *\n").unwrap_err().contains("stands alone"));
    }

    #[test]
    fn drift_is_a_named_skill_not_given_and_a_tracked_skill_the_list_does_not_name() {
        let entries = parse("a/every *\nb/some one two\nc/none *\n").unwrap();
        let have = [
            installed("x", Some("a/every"), true),
            installed("one", Some("b/some"), true),
            installed("two", Some("b/some"), false),
            installed("three", Some("b/some"), true),
            installed("stray", Some("d/other"), true),
            installed("handmade", None, true),
            installed("not-for-it", Some("d/other"), false),
        ];
        let (missing, extra) = drift(&entries, &have);
        assert_eq!(missing, vec!["two".to_string(), "every skill of c/none".to_string()]);
        assert_eq!(extra, vec!["three".to_string(), "stray".to_string()]);
    }

    #[test]
    fn copies_are_closed_to_every_write_and_opened_for_a_sync() {
        use std::os::unix::fs::PermissionsExt;
        let home = tempfile::tempdir().unwrap();
        let copies = home.path().join(COPIES);
        let skill = copies.join("mypython");
        std::fs::create_dir_all(skill.join("references")).unwrap();
        std::fs::write(skill.join("SKILL.md"), "a skill\n").unwrap();
        std::fs::write(skill.join("references/more.md"), "more\n").unwrap();
        set_open(&copies, false).unwrap();
        let mode = |path: &Path| std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        for path in [&skill, &skill.join("SKILL.md"), &skill.join("references/more.md")] {
            assert_eq!(mode(path) & 0o222, 0, "{} is writable", path.display());
        }
        assert!(std::fs::write(skill.join("SKILL.md"), "edited\n").is_err());
        assert!(std::fs::write(skill.join("new.md"), "new\n").is_err());
        assert_ne!(mode(&copies) & 0o200, 0, "a sync still adds a copy");
        set_open(&copies, true).unwrap();
        std::fs::write(skill.join("SKILL.md"), "updated\n").unwrap();
        std::fs::remove_dir_all(&skill).unwrap();
    }

    #[test]
    fn each_line_quotes_what_it_names() {
        let entry = parse("mattpocock/skills *\n").unwrap().remove(0);
        assert_eq!(
            add_line(&entry, &["claude-code", "codex"]),
            "npx -y skills add 'mattpocock/skills' -g -y -a 'claude-code' 'codex' -s '*'"
        );
        assert_eq!(
            remove_line("codex", &["stray".into()]),
            "npx -y skills remove -g -y -a 'codex' -s 'stray'"
        );
    }
}
