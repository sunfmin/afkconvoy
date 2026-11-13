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
            // pixi's download, its checksum and its unpacking, run as they are.
            ["curl" | "shasum" | "tar", ..] => {
                let out = std::process::Command::new("/bin/sh").args(["-c", line]).output()?;
                let mut printed = String::from_utf8_lossy(&out.stdout).into_owned();
                printed.push_str(&String::from_utf8_lossy(&out.stderr));
                (out.status.success(), printed)
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

/// Serves the files of `dir` over HTTP on this host, as GitHub serves a
/// release's → its address. A file not there is a 404.
fn serve(dir: &Path) -> String {
    use std::io::{BufRead, BufReader, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let dir = dir.to_path_buf();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request = String::new();
            let _ = reader.read_line(&mut request);
            loop {
                let mut header = String::new();
                if reader.read_line(&mut header).unwrap_or(0) <= 2 {
                    break;
                }
            }
            let path = request.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/');
            let reply = match std::fs::read(dir.join(path)) {
                Ok(body) => [
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).into(),
                    body,
                ]
                .concat(),
                Err(_) => b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n".to_vec(),
            };
            let _ = stream.write_all(&reply);
        }
    });
    address
}

/// A release of pixi as GitHub publishes one, under `releases/`: the archive
/// of a program `pixi` and, next to it, its checksum — or, where `wrong`, a
/// checksum of something else.
fn release(releases: &Path, wrong: bool) {
    let target = match std::env::consts::OS {
        "macos" => format!("{}-apple-darwin", std::env::consts::ARCH),
        _ => format!("{}-unknown-linux-musl", std::env::consts::ARCH),
    };
    let version = releases.join(format!("v{}", afkconvoy_host_kit::tmux::PIXI_VERSION));
    let program = releases.join("program");
    executable(&program.join("pixi"));
    std::fs::create_dir_all(&version).unwrap();
    let name = format!("pixi-{target}.tar.gz");
    let archive = version.join(&name);
    let packed = std::process::Command::new("tar")
        .arg("-czf")
        .arg(&archive)
        .arg("-C")
        .arg(&program)
        .arg("pixi")
        .status()
        .unwrap();
    assert!(packed.success());
    let summed = std::process::Command::new("shasum").args(["-a", "256"]).arg(&archive).output();
    let summed = String::from_utf8(summed.unwrap().stdout).unwrap();
    let sum = summed.split_whitespace().next().unwrap();
    let sum = if wrong { "0".repeat(64) } else { sum.to_string() };
    std::fs::write(version.join(format!("{name}.sha256")), format!("{sum}  {name}\n")).unwrap();
}

#[test]
fn a_guild_machine_with_neither_pixi_nor_a_bottled_homebrew_is_given_the_pinned_pixi_then_tmux() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::create_dir_all(home.join(".guild")).unwrap();
    let releases = tempfile::tempdir().unwrap();
    release(releases.path(), false);
    let machine = Machine::default();

    let given = afkconvoy_host_kit::tmux::give_from(&machine, home, &serve(releases.path()));
    let pixi = home.join(".pixi/bin/pixi");
    let by = format!("'{}' global install tmux", pixi.display());
    assert_eq!(given, Some(Tmux::Installed { installed: home.join(".pixi/bin/tmux"), by }));
    assert!(!home.join(".pixi/download").exists(), "the download is not kept");
    let pinned = format!("/v{}/pixi-", afkconvoy_host_kit::tmux::PIXI_VERSION);
    let downloads: Vec<String> =
        machine.asked().into_iter().filter(|line| line.starts_with("curl ")).collect();
    assert_eq!(downloads.len(), 2, "the archive and its checksum: {downloads:?}");
    assert!(downloads.iter().all(|line| line.contains(&pinned)), "{downloads:?}");
}

#[test]
fn a_pixi_download_whose_checksum_differs_installs_nothing() {
    let home = tempfile::tempdir().unwrap();
    let home = home.path();
    std::fs::create_dir_all(home.join(".guild")).unwrap();
    let releases = tempfile::tempdir().unwrap();
    release(releases.path(), true);
    let machine = Machine::default();

    let given = afkconvoy_host_kit::tmux::give_from(&machine, home, &serve(releases.path()));
    let Some(Tmux::Failed { error }) = given else { panic!("{given:?}") };
    assert!(error.contains("checksum"), "{error}");
    assert!(error.contains("nothing was installed"), "{error}");
    assert!(!home.join(".pixi").exists(), "nothing of pixi is left");
    assert!(machine.asked().iter().all(|line| !line.ends_with("install tmux")));
    assert!(machine.asked().iter().all(|line| !line.starts_with("tar ")), "nothing unpacked");
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
