//! The host kit crate as another program uses it (docs/adr/0062): through its
//! public API alone, with no afkconvoy beside it. A program gives a machine
//! the declared instructions, skills and guards for every harness it finds,
//! installs tmux on a Guild machine, and reads back where the machine differs
//! from the declaration.

use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use afkconvoy_host_kit::{Declared, Harness, Shell, Tmux};

/// A login shell that finds Claude Code and Codex, keeps what `npx skills`
/// installs for each agent, and installs tmux with pixi under the home.
#[derive(Default)]
struct Machine {
    asked: Mutex<Vec<String>>,
    /// Each agent and the skills installed for it, from `source`.
    skills: Mutex<Vec<(String, String, String)>>,
}

impl Machine {
    fn asked(&self) -> Vec<String> {
        self.asked.lock().unwrap().clone()
    }
}

/// The words of a line as a POSIX shell reads them, quotes taken off.
fn words(line: &str) -> Vec<String> {
    line.split_whitespace().map(|word| word.trim_matches('\'').to_string()).collect()
}

impl Shell for Machine {
    fn run(&self, line: &str, _timeout: Duration) -> anyhow::Result<Option<(bool, String)>> {
        self.asked.lock().unwrap().push(line.to_string());
        let words = words(line);
        let words: Vec<&str> = words.iter().map(String::as_str).collect();
        let answer = match words.as_slice() {
            ["type", "--", "claude" | "codex"] => (true, String::new()),
            ["type", ..] | ["command", "-v", _] => (false, String::new()),
            ["npx", "-y", "skills", "add", source, "-g", "-y", "-a", rest @ ..] => {
                let at = rest.iter().position(|word| *word == "-s").unwrap();
                let mut skills = self.skills.lock().unwrap();
                for agent in &rest[..at] {
                    for skill in &rest[at + 1..] {
                        skills.push((agent.to_string(), source.to_string(), skill.to_string()));
                    }
                }
                (true, String::new())
            }
            ["npx", "-y", "skills", "ls", "-g", "-a", agent, "--json"] => {
                let skills = self.skills.lock().unwrap();
                let listed: Vec<_> = skills
                    .iter()
                    .filter(|(of, _, _)| of == agent)
                    .map(|(_, source, name)| {
                        serde_json::json!({"name": name, "source": source, "agents": [agent]})
                    })
                    .collect();
                (true, format!("npm notice\n{}", serde_json::Value::from(listed)))
            }
            [pixi, "global", "install", "tmux"] => {
                let tmux = Path::new(pixi).with_file_name("tmux");
                executable(&tmux);
                (true, String::new())
            }
            _ => (false, format!("{line}: not known on this machine")),
        };
        Ok(Some(answer))
    }
}

fn executable(file: &Path) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, "#!/bin/sh\n").unwrap();
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A home with the operator's declaration in `dotfiles/`.
fn declared(home: &Path) -> Declared {
    let dotfiles = home.join("dotfiles");
    std::fs::create_dir_all(&dotfiles).unwrap();
    std::fs::write(dotfiles.join("CLAUDE.md"), "principles\n").unwrap();
    std::fs::write(dotfiles.join("skills.txt"), "sunfmin/skills tdd grilling\n").unwrap();
    std::fs::write(dotfiles.join("guards.txt"), "shell guard-branch-delete.sh\n").unwrap();
    Declared {
        instructions: Some(dotfiles.join("CLAUDE.md")),
        skills: Some(dotfiles.join("skills.txt")),
        guards: Some(dotfiles.join("guards.txt")),
    }
}

#[test]
fn a_machine_is_given_the_declared_setup_for_every_harness_it_has_and_differs_in_nothing_after() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    let declared = declared(home);
    let machine = Machine::default();

    let (before, tmux) = afkconvoy_host_kit::report(&machine, home, &declared);
    assert_eq!(tmux, None, "no Guild machine");
    let differ: Vec<Harness> = before.iter().map(|difference| difference.harness).collect();
    assert_eq!(differ, [Harness::ClaudeCode, Harness::Codex]);
    let said = before[1].said.join("; ");
    assert!(said.contains("~/.codex/AGENTS.md is missing"), "{said}");
    assert!(said.contains("is missing guards: guard-branch-delete.sh"), "{said}");
    assert!(said.contains("missing skills: tdd, grilling"), "{said}");

    let given = afkconvoy_host_kit::machine(&machine, home, &declared, 7);
    assert_eq!(given.harnesses, [Harness::ClaudeCode, Harness::Codex]);
    let done = given.done.unwrap();
    assert!(done.outcomes.iter().all(|(_, failed)| failed.is_none()), "{done:?}");
    assert_eq!(done.linked, [Harness::ClaudeCode, Harness::Codex]);
    assert_eq!(done.wired, [Harness::ClaudeCode, Harness::Codex]);
    assert_eq!(
        done.synced,
        ["npx -y skills add 'sunfmin/skills' -g -y -a 'claude-code' 'codex' -s 'tdd' 'grilling'"]
    );
    let linked = std::fs::read_link(home.join(".claude/CLAUDE.md")).unwrap();
    assert_eq!(linked, home.join("dotfiles/CLAUDE.md"));
    let hooks = std::fs::read_to_string(home.join(".codex/hooks.json")).unwrap();
    assert!(hooks.contains("guard-branch-delete.sh"), "{hooks}");

    let (after, tmux) = afkconvoy_host_kit::report(&machine, home, &declared);
    assert_eq!(after, [], "the machine is as declared");
    assert_eq!(tmux, None);
}

#[test]
fn a_guild_machine_with_no_tmux_is_given_one_and_said_to_lack_it_until_then() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::create_dir_all(home.join(".guild")).unwrap();
    executable(&home.join(".pixi/bin/pixi"));
    let machine = Machine::default();
    let nothing = Declared::default();

    let (differ, tmux) = afkconvoy_host_kit::report(&machine, home, &nothing);
    assert_eq!(differ, [], "nothing declared, nothing differs");
    assert!(tmux.unwrap().contains("holds a tmux"));

    let given = afkconvoy_host_kit::machine(&machine, home, &nothing, 7);
    assert_eq!(given.done, None, "nothing declared, nothing given");
    let pixi = home.join(".pixi/bin/pixi");
    let by = format!("'{}' global install tmux", pixi.display());
    assert_eq!(given.tmux, Some(Tmux::Installed { installed: home.join(".pixi/bin/tmux"), by }));
    let installs = machine.asked().into_iter().filter(|line| line.ends_with("install tmux"));
    assert_eq!(installs.count(), 1);

    assert_eq!(afkconvoy_host_kit::report(&machine, home, &nothing).1, None);
    assert_eq!(afkconvoy_host_kit::machine(&machine, home, &nothing, 8).tmux, None, "left alone");
}

/// What the crate depends on: the harness crate, anyhow and serde — never
/// afkconvoy-core, whose fleet concepts its API is to stay free of.
#[test]
fn the_crate_depends_on_nothing_of_the_fleet() {
    let manifest = include_str!("../Cargo.toml");
    let section = manifest.split("[dependencies]").nth(1).unwrap().split("\n[").next().unwrap();
    let mut names: Vec<&str> = section
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .filter_map(|line| line.split_once(['=', '.']).map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty())
        .collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names, ["afkconvoy-harness", "anyhow", "serde", "serde_json"]);
}

/// The crate's public API names no fleet concept: none of its public items
/// and fields is named for a team, role, stage, issue, claim, patrol,
/// intake, tracker or GitHub.
#[test]
fn its_public_api_names_no_fleet_concept() {
    const FLEET: [&str; 9] =
        ["team", "role", "stage", "issue", "claim", "patrol", "intake", "tracker", "github"];
    let sources = [
        include_str!("../src/lib.rs"),
        include_str!("../src/found.rs"),
        include_str!("../src/guards.rs"),
        include_str!("../src/instructions.rs"),
        include_str!("../src/shell.rs"),
        include_str!("../src/skills.rs"),
        include_str!("../src/tmux.rs"),
    ];
    for source in sources {
        let public = source.split("#[cfg(test)]").next().unwrap();
        for line in public.lines().map(str::trim).filter(|line| line.starts_with("pub ")) {
            let lower = line.to_lowercase();
            let words: Vec<&str> = lower
                .split(|c: char| !c.is_ascii_alphanumeric())
                .filter(|w| !w.is_empty())
                .collect();
            for concept in FLEET {
                assert!(
                    !words.iter().any(|word| word.starts_with(concept)),
                    "`{line}` names a fleet concept: {concept}"
                );
            }
        }
    }
}
