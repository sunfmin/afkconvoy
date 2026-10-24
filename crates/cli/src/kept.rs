//! What this host keeps for a harness before it starts a worker of it
//! (`Harness::kept`): a directory of the fleet's own, beside its copies,
//! holding the files the tool carries for that harness and links to the few
//! files of the host user's the harness is to have — and, in a directory kept
//! apart for a worker given them, to the host user's skills and rules. Which
//! files is the harness's to say; here they are only put there.

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use afkconvoy_core::harness::Harness;
use afkconvoy_core::team::{SKILL_FILE, SKILLS};
use anyhow::{Context, Result};

use crate::copies::cache;

/// Puts what this host keeps for `harness` in its directory as it stands
/// now → the directory. Every start does: a file the tool carries is as this
/// build has it, and a link follows what the host user has today — made when
/// the file is there, taken away when it no longer is. A harness nothing is
/// kept for gets no directory; any other has one, whatever it holds. With
/// `host_user` it is the directory that links to the host user's skills and
/// rules too (`Kept::host_user`); without, the one that holds none of them.
pub fn keep(harness: Harness, host_user: bool) -> Result<PathBuf> {
    let dir = keep_under(&cache()?, home().as_deref(), harness, host_user)?;
    if harness == Harness::Dsh {
        // Prefer the installed program's package to an editable profile copy,
        // as the shell a worker gets finds it.
        let asked = crate::orca::ask("command -v dsh-tui", crate::launch::SHELL_TIMEOUT);
        let (_, path) = asked.ok().flatten().unwrap_or_default();
        if let Ok(program) = std::fs::canonicalize(path.trim())
            && let Some(package) = program.parent().and_then(Path::parent)
            && package.join("package.json").is_file()
        {
            point(
                &dir.join("profiles/dsh-tui/node_modules/@deepseek-harness-tui/dsh-tui"),
                Some(package),
            )?;
        }
    }
    Ok(dir)
}

/// The home of the host's user, where it has one.
fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// `keep`, on a host whose fleets keep what they keep under `cache` and whose
/// user's home is `home`.
pub fn keep_under(
    cache: &Path,
    home: Option<&Path>,
    harness: Harness,
    host_user: bool,
) -> Result<PathBuf> {
    let kept = harness.kept();
    let (name, theirs) = match host_user {
        true => (format!("{}-host-user", harness.key()), kept.host_user),
        false => (harness.key().to_string(), &[][..]),
    };
    let dir = cache.join("harness").join(name);
    let put =
        |what: &str| format!("cannot keep {what} for {} in {}", harness.name(), dir.display());
    // The directory is there for a harness started under it, whatever of the
    // host user's is missing today.
    if !(kept.files.is_empty() && kept.links.is_empty() && theirs.is_empty()) {
        std::fs::create_dir_all(&dir).with_context(|| put("anything"))?;
    }
    // An overlay names the plugin it loads by its whole path, as YAML quotes it.
    let plugin = |file: &str| dir.join(file).display().to_string().replace('\'', "''");
    for (name, text) in kept.files {
        let text = text
            .replace("@AFKCONVOY_DSH_PLUGIN@", &plugin("afkconvoy.mjs"))
            .replace("@AFKCONVOY_DSH_PROBE@", &plugin("probe.mjs"))
            .replace("@AFKCONVOY_DSH_MODELS@", &plugin("models.mjs"));
        carry(&dir.join(name), &text).with_context(|| put(name))?;
    }
    for link in kept.links.iter().chain(theirs) {
        let theirs = if harness == Harness::Dsh {
            let custom =
                link.home.strip_prefix(".dsh/").map(|path| ("DSH_HOME", path)).or_else(|| {
                    link.home.strip_prefix(".agents/").map(|path| ("DSH_AGENTS_HOME", path))
                });
            custom
                .and_then(|(variable, path)| {
                    std::env::var_os(variable)
                        .filter(|dir| !dir.is_empty())
                        .map(|dir| PathBuf::from(dir).join(path))
                })
                .or_else(|| home.map(|home| home.join(link.home)))
        } else {
            home.map(|home| home.join(link.home))
        }
        .filter(|file| file.exists());
        point(&dir.join(link.kept), theirs.as_deref()).with_context(|| put(link.kept))?;
    }
    Ok(dir)
}

/// What a worker of `harness` in the role whose copy is `role_dir` is
/// started with of what this host keeps: the directory, put in place
/// (`keep`), and the skills of the host user's its role's stand for
/// (`shadowed`).
pub fn for_worker(
    harness: Harness,
    host_user: bool,
    role_dir: &Path,
) -> Result<(String, Vec<String>)> {
    let dir = keep(harness, host_user)?;
    let shadowed = shadowed(harness, role_dir, &dir);
    Ok((dir.display().to_string(), shadowed))
}

/// The files of the host user's skills that a skill of the role in
/// `role_dir` stands for, where a worker of `harness` would be shown both
/// (`Harness::shadowed_in`): each as the user's home has it, as it is with
/// every link followed, and as the directory `kept` links to it — whichever
/// the harness knows it by.
pub fn shadowed(harness: Harness, role_dir: &Path, kept: &Path) -> Vec<String> {
    match home() {
        Some(home) => shadowed_under(&home, harness, role_dir, kept),
        None => vec![],
    }
}

/// `shadowed`, of the host user whose home is `home`.
pub fn shadowed_under(home: &Path, harness: Harness, role_dir: &Path, kept: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(role_dir.join(SKILLS)) else { return vec![] };
    let mut names: Vec<String> =
        entries.flatten().map(|entry| entry.file_name().to_string_lossy().into_owned()).collect();
    names.sort();
    let mut files: Vec<String> = vec![];
    for dir in harness.shadowed_in() {
        let linked = harness.kept().host_user.iter().find(|link| link.home == *dir);
        for name in &names {
            let theirs = home.join(dir).join(name).join(SKILL_FILE);
            if !theirs.is_file() {
                continue;
            }
            let followed = theirs.canonicalize().ok();
            let through = linked.map(|link| kept.join(link.kept).join(name).join(SKILL_FILE));
            for file in [Some(theirs), followed, through].into_iter().flatten() {
                let file = file.display().to_string();
                if !files.contains(&file) {
                    files.push(file);
                }
            }
        }
    }
    files
}

/// Has `file` hold `text`, replaced in one step: a worker starting beside
/// this one never loads half of it.
fn carry(file: &Path, text: &str) -> std::io::Result<()> {
    if std::fs::read_to_string(file).is_ok_and(|held| held == text) {
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let draft = file.with_extension(format!("{}.draft", crate::unique()));
    std::fs::write(&draft, text)?;
    std::fs::rename(&draft, file)
}

/// Has `link` point at `theirs`, or be gone when there is none.
fn point(link: &Path, theirs: Option<&Path>) -> std::io::Result<()> {
    if std::fs::read_link(link).ok().as_deref() == theirs && link.is_symlink() == theirs.is_some() {
        return Ok(());
    }
    match std::fs::remove_file(link) {
        Err(error) if error.kind() != ErrorKind::NotFound => return Err(error),
        _ => {}
    }
    let Some(theirs) = theirs else { return Ok(()) };
    if let Some(parent) = link.parent() {
        std::fs::create_dir_all(parent)?;
    }
    match std::os::unix::fs::symlink(theirs, link) {
        // Another start of this host put it there first: the same link.
        Err(error) if error.kind() == ErrorKind::AlreadyExists => Ok(()),
        made => made,
    }
}
