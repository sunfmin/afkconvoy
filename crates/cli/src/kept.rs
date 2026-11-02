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
        // as the login shell a worker gets finds it. No terminal is opened.
        let asked = crate::launch::ask_host("command -v dsh-tui", crate::launch::SHELL_TIMEOUT);
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

/// A home for an interactive Agent outside a role. Generated hook files live
/// in a session's own home: a probe or asking launch must never overwrite a
/// worker's or watch's guard in a shared home.
pub(crate) fn outside_role(
    harness: Harness,
    worktree: &Path,
    guard: Option<&str>,
) -> Result<String> {
    let shared = keep(harness, true)?;
    if has_own_home(harness) {
        let dir = worktree
            .join(crate::worker_tmp::TMP_DIR)
            .join(format!("{OWN_HOME}{}", crate::unique()));
        keep_at(&dir, home().as_deref(), harness, true)?;
        configure(&dir, home().as_deref(), harness, true, None, None, guard)?;
        return Ok(dir.display().to_string());
    }
    Ok(shared.display().to_string())
}

/// The name every home `outside_role` makes starts with.
const OWN_HOME: &str = "guard-";

/// Whether a session of `harness` outside a role gets a home of its own.
fn has_own_home(harness: Harness) -> bool {
    matches!(harness, Harness::Kimi | Harness::Grok)
}

/// Every home `outside_role` made for a session of `harness` in `worktree`:
/// the harness keeps that session's record there, so whoever reads the
/// session reads these too. A worktree with none has none.
pub(crate) fn outside_role_homes(harness: Harness, worktree: &Path) -> Result<Vec<PathBuf>> {
    if !has_own_home(harness) {
        return Ok(Vec::new());
    }
    let dir = worktree.join(crate::worker_tmp::TMP_DIR);
    let entries = match std::fs::read_dir(&dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", dir.display())),
    };
    let mut homes = Vec::new();
    for entry in entries {
        let path = entry.with_context(|| format!("cannot read {}", dir.display()))?.path();
        let own = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(OWN_HOME));
        if own && path.is_dir() {
            homes.push(path);
        }
    }
    Ok(homes)
}

/// The host's rule files which native project discovery can find even when
/// the harness's user settings are disabled.
pub fn host_rules(harness: Harness) -> Vec<String> {
    home().map(|home| harness.host_rule_excludes(&home.display().to_string())).unwrap_or_default()
}

/// `keep`, on a host whose fleets keep what they keep under `cache` and whose
/// user's home is `home`.
pub fn keep_under(
    cache: &Path,
    home: Option<&Path>,
    harness: Harness,
    host_user: bool,
) -> Result<PathBuf> {
    keep_under_with_dsh_homes(
        cache,
        home,
        harness,
        host_user,
        std::env::var_os("DSH_HOME").filter(|dir| !dir.is_empty()).map(PathBuf::from),
        std::env::var_os("DSH_AGENTS_HOME").filter(|dir| !dir.is_empty()).map(PathBuf::from),
    )
}

/// `keep_under` with the DSH homes supplied by its caller rather than the
/// process environment. This keeps callers that construct an isolated world
/// from accidentally reading the invoking session's DSH profile.
pub fn keep_under_with_dsh_homes(
    cache: &Path,
    home: Option<&Path>,
    harness: Harness,
    host_user: bool,
    dsh_home: Option<PathBuf>,
    dsh_agents_home: Option<PathBuf>,
) -> Result<PathBuf> {
    let name = match host_user {
        true => format!("{}-host-user", harness.key()),
        false => harness.key().to_string(),
    };
    let dir = cache.join("harness").join(name);
    keep_at_with_dsh_homes(&dir, home, harness, host_user, dsh_home, dsh_agents_home)?;
    configure(&dir, home, harness, host_user, None, None, None)?;
    Ok(dir)
}

/// Materializes one kept directory and its current links.
fn keep_at(dir: &Path, home: Option<&Path>, harness: Harness, host_user: bool) -> Result<()> {
    keep_at_with_dsh_homes(
        dir,
        home,
        harness,
        host_user,
        std::env::var_os("DSH_HOME").filter(|dir| !dir.is_empty()).map(PathBuf::from),
        std::env::var_os("DSH_AGENTS_HOME").filter(|dir| !dir.is_empty()).map(PathBuf::from),
    )
}

fn keep_at_with_dsh_homes(
    dir: &Path,
    home: Option<&Path>,
    harness: Harness,
    host_user: bool,
    dsh_home: Option<PathBuf>,
    dsh_agents_home: Option<PathBuf>,
) -> Result<()> {
    let kept = harness.kept();
    let theirs = if host_user { kept.host_user } else { &[][..] };
    let put =
        |what: &str| format!("cannot keep {what} for {} in {}", harness.name(), dir.display());
    if !(kept.files.is_empty() && kept.links.is_empty() && theirs.is_empty()) {
        std::fs::create_dir_all(dir).with_context(|| put("anything"))?;
    }
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
                    let dir = match variable {
                        "DSH_HOME" => dsh_home.as_ref(),
                        "DSH_AGENTS_HOME" => dsh_agents_home.as_ref(),
                        _ => None,
                    }?;
                    Some(dir.join(path))
                })
                .or_else(|| home.map(|home| home.join(link.home)))
        } else {
            home.map(|home| home.join(link.home))
        }
        .filter(|file| file.exists());
        point(&dir.join(link.kept), theirs.as_deref()).with_context(|| put(link.kept))?;
    }
    Ok(())
}

/// The directories under a role's `.claude/skills`. Grok ignores a host skill
/// of each of these names so the role's copy is the one that loads (ADR-0043).
fn role_skill_names(role_dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(role_dir.join(SKILLS)) else { return vec![] };
    let mut names: Vec<String> = entries
        .flatten()
        .filter_map(|entry| {
            if !entry.file_type().ok()?.is_dir() {
                return None;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.is_empty() || name.contains('/') || name.contains('\\') {
                return None;
            }
            Some(name)
        })
        .collect();
    names.sort();
    names
}

/// Writes the harness's generated files from the host's current model routing
/// and, for a worker, its role's limits and the fleet's guard.
fn configure(
    dir: &Path,
    home: Option<&Path>,
    harness: Harness,
    host_user: bool,
    role_dir: Option<&Path>,
    told: Option<&afkconvoy_core::role::Told>,
    guard: Option<&str>,
) -> Result<()> {
    if harness
        .kept_configuration(None, None, None, host_user, None, &[])
        .map_err(anyhow::Error::msg)?
        .is_none()
    {
        return Ok(());
    }
    let user = harness.user_config().and_then(|path| {
        home.map(|home| home.join(path)).filter(|file| file.is_file()).map(std::fs::read_to_string)
    });
    let user = user.transpose()?;
    let role = role_dir
        .map(|role| role.join(harness.limits()))
        .filter(|file| file.is_file())
        .map(std::fs::read_to_string)
        .transpose()?;
    let names = role_dir.map(role_skill_names).unwrap_or_default();
    let skills = role_dir.and_then(|role| {
        let dir = role.join(SKILLS);
        dir.is_dir().then(|| dir.display().to_string())
    });
    let Some(config) = harness
        .kept_configuration(
            user.as_deref(),
            role.as_deref(),
            guard,
            host_user,
            skills.as_deref(),
            &names,
        )
        .map_err(anyhow::Error::msg)?
    else {
        return Ok(());
    };
    std::fs::create_dir_all(dir)?;
    let file = dir.join("config.toml");
    carry(&file, &config)?;
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o600))?;
    let system_file = dir.join(harness.kept_system_file());
    if let Some(system) = told.and_then(|told| harness.kept_system(told)) {
        carry(&system_file, &system)?;
    } else {
        match std::fs::remove_file(&system_file) {
            Err(error) if error.kind() != ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    Ok(())
}

/// What a worker of `harness` in the role whose copy is `role_dir` is
/// started with of what this host keeps: the directory, put in place, and the
/// skills of the host user's its role's stand for. A harness with generated
/// role configuration gets a role-specific home so concurrent roles share none.
pub fn for_worker(
    harness: Harness,
    host_user: bool,
    role_dir: &Path,
    worktree: &Path,
    told: &afkconvoy_core::role::Told,
    guard: &str,
) -> Result<(String, Vec<String>)> {
    let shared = keep(harness, host_user)?;
    // A harness that keeps a session apart gets a fresh directory, so a
    // choice it saves belongs to this session and to no other worker.
    let base = match crate::codex_launch::fresh_root(harness, worktree) {
        Some(root) => keep_under(&root, home().as_deref(), harness, host_user)?,
        None => shared,
    };
    let (dir, shadowed) = worker_at(base, home(), harness, host_user, role_dir, told, guard)?;
    if let Some(session) = harness.session() {
        let worker = crate::orca::Worktree::at(worktree.to_path_buf(), String::new());
        std::fs::write(worker.note(session.note)?, &dir)?;
    }
    Ok((dir, shadowed))
}

/// `for_worker` on a host whose cache and home are named explicitly.
pub fn for_worker_under(
    cache: &Path,
    home: Option<&Path>,
    harness: Harness,
    host_user: bool,
    role_dir: &Path,
    told: &afkconvoy_core::role::Told,
    guard: &str,
) -> Result<(String, Vec<String>)> {
    let base = keep_under(cache, home, harness, host_user)?;
    worker_at(base, home.map(Path::to_path_buf), harness, host_user, role_dir, told, guard)
}

fn worker_at(
    base: PathBuf,
    home: Option<PathBuf>,
    harness: Harness,
    host_user: bool,
    role_dir: &Path,
    told: &afkconvoy_core::role::Told,
    guard: &str,
) -> Result<(String, Vec<String>)> {
    let dir = if harness
        .kept_configuration(None, None, None, host_user, None, &[])
        .map_err(anyhow::Error::msg)?
        .is_some()
    {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        role_dir.hash(&mut hasher);
        let dir = base.join(format!("role-{:016x}", hasher.finish()));
        keep_at(&dir, home.as_deref(), harness, host_user)?;
        configure(
            &dir,
            home.as_deref(),
            harness,
            host_user,
            Some(role_dir),
            Some(told),
            Some(guard),
        )?;
        dir
    } else {
        base
    };
    let shadowed = match home {
        Some(home) => shadowed_under(&home, harness, role_dir, &dir),
        None => vec![],
    };
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
