//! A harness's session for one worktree: the turn it has open, or the one
//! it ended. The fleet reads that record. Orca's receipt and the terminal
//! screen are not a turn.
//!
//! The harness names where the session is and what its text says
//! (docs/adr/0044). This module only opens files. A missing session is no
//! turn. A file that is there and cannot be read is an error, so a tick
//! does not treat it as idle.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use afkconvoy_core::harness::{Harness, SessionLayout};
use afkconvoy_core::turn::{Turn, prefer};
use anyhow::{Context, Result};

/// The turn `harness` has recorded for `cwd`.
///
/// `since` drops session files last written before the worker started, so a
/// killed worker's open file does not keep the next one busy. `not_before`
/// is the start proof's bound: a turn that started earlier is not this
/// send. A missing session directory is no turn. A file that is there and
/// cannot be read is an error.
pub fn read_turn(
    cwd: &Path,
    harness: Harness,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Option<Turn>> {
    let mut turns = Vec::new();
    for root in &roots(cwd, harness)? {
        if let Some(turn) = read_root(root, harness, cwd, since, not_before)? {
            turns.push(turn);
        }
    }
    Ok(prefer(&turns))
}

/// Whether `harness`'s session for `cwd` holds `mark` in a message its
/// model was given as the user's (`Harness::holds_note`). `since` drops
/// files last written before it, as for a turn.
pub fn holds_note(cwd: &Path, harness: Harness, since: u64, mark: &str) -> Result<bool> {
    notes_match(cwd, harness, since, |text| harness.holds_note(text, mark))
}

/// The timestamp of a recorded user entry, for live delivery evidence.
/// Queue records and tool output cannot supply it.
pub fn note_at(cwd: &Path, harness: Harness, since: u64, mark: &str) -> Result<Option<u64>> {
    let found = std::cell::Cell::new(None);
    notes_match(cwd, harness, since, |text| {
        let at = harness.noted_at(text, mark);
        if at.is_some() {
            found.set(at);
        }
        at.is_some()
    })?;
    Ok(found.get())
}

/// Reads the harness's message files once for a question about this worker.
/// A tree of sessions holds every worktree's, so a session in one must
/// record this worktree's turn; a project message store already has its
/// directory in its layout.
fn notes_match(
    cwd: &Path,
    harness: Harness,
    since: u64,
    matches: impl Fn(&str) -> bool,
) -> Result<bool> {
    let cwd_text = cwd_str(cwd);
    let place = harness.note_place(&cwd_text);
    let brief = brief(cwd);
    for root in &roots(cwd, harness)? {
        if !root.exists() {
            continue;
        }
        for layout in std::iter::once(&place.layout).chain(&place.also) {
            let tree = matches!(layout, SessionLayout::Tree { .. });
            let holds = |text: &str| {
                let belongs = !tree
                    || harness
                        .recorded_turn(&cwd_text, text, Some(since), brief.as_deref())
                        .is_some();
                (belongs && matches(text)).then_some(())
            };
            let held =
                read_layout(root, harness, &cwd_text, layout.clone(), Some(since), whole(holds))?;
            if !held.is_empty() {
                return Ok(true);
            }
        }
    }
    Ok(false)
}

/// A write's result link identifies a worker's own comment even when it
/// used a body file. Read commands and user messages cannot suppress one.
pub(crate) fn posted_comment(cwd: &Path, harness: Harness, since: u64, url: &str) -> Result<bool> {
    notes_match(cwd, harness, since, |text| posted_in(harness, text, url))
}

fn posted_in(harness: Harness, text: &str, url: &str) -> bool {
    harness.steps(text).iter().any(|step| writes_comment(&step.action) && links(&step.result, url))
}

/// Whether a tool call is a `gh` write of an issue comment.
fn writes_comment(action: &str) -> bool {
    action.contains("gh ")
        && (action.contains("issue comment")
            || action.contains("/comments") && (action.contains("POST") || action.contains("-f")))
}

/// Whether `result` gives `url` itself, not a longer comment id that starts with it.
fn links(result: &str, url: &str) -> bool {
    result.match_indices(url).any(|(at, _)| {
        result[at + url.len()..].chars().next().is_none_or(|next| !next.is_ascii_digit())
    })
}

/// Writes, for every harness under `home`, what it keeps of `line` typed at
/// the worker in `cwd` while it worked: given to its model when `taken`,
/// else only queued. The stand-in calls this.
pub fn record_note(home: &Path, cwd: &Path, line: &str, taken: bool, at: u64) -> Result<()> {
    let cwd = cwd_str(cwd);
    for harness in Harness::ALL {
        let root = home.join(harness.note_place(&cwd).under_home);
        for file in harness.note_files(&cwd, line, taken, at) {
            write_text(&root.join(&file.path), &file.body, at)?;
        }
    }
    Ok(())
}

/// Every directory `harness` may keep the session of `cwd` under: the
/// user's home, the directories the fleet keeps, a home of its own a session
/// outside a role was given in the worktree, and a home noted beside the
/// worktree.
fn roots(cwd: &Path, harness: Harness) -> Result<Vec<PathBuf>> {
    let place = harness.session_place(&cwd_str(cwd));
    let mut roots = Vec::new();
    if let Some(home) = user_home() {
        roots.push(home.join(place.under_home));
    }
    roots.extend(kept_roots(harness)?);
    roots.extend(crate::kept::outside_role_homes(harness, cwd)?);
    if let Some(name) = harness.noted_home()
        && let Some(home) = noted(cwd, name)
    {
        roots.push(home);
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

/// Writes one turn for every harness under `home`, closed unless `open`.
/// The stand-in calls this. The files are the ones the harness names, so a
/// test does not keep a second copy of where a session lives.
pub fn record(home: &Path, cwd: &Path, open: bool, at: u64) -> Result<()> {
    let cwd = cwd_str(cwd);
    for harness in Harness::ALL {
        write_harness(home, harness, &cwd, open, at)?;
    }
    Ok(())
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// The names of the directories the fleet keeps for `harness`: the fleet's
/// own and the host user's.
fn kept_names(harness: Harness) -> [String; 2] {
    [harness.key().to_string(), format!("{}-host-user", harness.key())]
}

/// Directories the fleet keeps for `harness`, including a role's own home.
/// Each is already the harness directory.
fn kept_roots(harness: Harness) -> Result<Vec<PathBuf>> {
    let Ok(cache) = crate::copies::cache() else {
        return Ok(Vec::new());
    };
    let base = cache.join("harness");
    let mut roots = Vec::new();
    for name in kept_names(harness) {
        let dir = base.join(name);
        if !dir.is_dir() {
            continue;
        }
        roots.push(dir.clone());
        let Some(entries) = list_dir(&dir)? else { continue };
        for entry in entries {
            let role = entry
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("role-"));
            if role && entry.is_dir() {
                roots.push(entry);
            }
        }
    }
    Ok(roots)
}

/// Whether `harness`'s session for `cwd` shows it began a turn of its goal
/// by itself (`Harness::goal_continued`), in files written since `since`.
pub fn goal_continued(cwd: &Path, harness: Harness, since: u64) -> Result<bool> {
    notes_match(cwd, harness, since, |text| harness.goal_continued(text))
}

/// The directory the launch kept for the session of the worker in `cwd`,
/// where `harness` keeps one apart.
pub fn kept_home(cwd: &Path, harness: Harness) -> Option<PathBuf> {
    noted(cwd, harness.noted_home()?)
}

/// The harness directory a launch noted beside the worktree's git directory.
fn noted(cwd: &Path, name: &str) -> Option<PathBuf> {
    let home = std::fs::read_to_string(git_dir(cwd)?.join(name)).ok()?;
    let home = home.trim();
    if home.is_empty() { None } else { Some(PathBuf::from(home)) }
}

/// The worktree's own git directory, also where its brief is written.
/// Read the pointer directly: polling for a turn must not spawn git.
fn git_dir(cwd: &Path) -> Option<PathBuf> {
    let git = std::fs::read_to_string(cwd.join(".git")).ok()?;
    let dir = git.trim().strip_prefix("gitdir:")?.trim();
    if dir.is_empty() { None } else { Some(cwd.join(dir)) }
}

/// The task brief the fleet keeps for the worker in `cwd`, as an absolute path.
fn brief(cwd: &Path) -> Option<String> {
    Some(cwd_str(&git_dir(cwd)?.join(crate::orca::BRIEF_FILE)))
}

fn read_root(
    root: &Path,
    harness: Harness,
    cwd: &Path,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Option<Turn>> {
    if !root.exists() {
        return Ok(None);
    }
    let brief = brief(cwd);
    let cwd = cwd_str(cwd);
    let layout = harness.session_place(&cwd).layout;
    let turns = read_layout(root, harness, &cwd, layout, since, |path, load| {
        let key = Asked {
            path: path.to_path_buf(),
            harness: harness.key(),
            cwd: cwd.clone(),
            not_before,
            brief: brief.clone(),
        };
        remembered(key, load, |text| {
            harness.recorded_turn(&cwd, text, not_before, brief.as_deref())
        })
    })?;
    Ok(prefer(&turns))
}

/// What one turn question asks of one session file.
#[derive(Clone, PartialEq, Eq, Hash)]
struct Asked {
    path: PathBuf,
    harness: &'static str,
    cwd: String,
    not_before: Option<u64>,
    brief: Option<String>,
}

/// A session file is known by its size and modification time: a harness
/// only appends to or rewrites it, and either changes one of them.
type Stamp = (u64, u128);

/// The answers the host runner already read, so a file unchanged since the
/// last look costs a `stat`, not a parse (sunfmin/control#938). A host
/// watch's directory holds every session of its checkout, tens of
/// megabytes read every two seconds.
static READ: std::sync::Mutex<Option<Answers>> = std::sync::Mutex::new(None);

type Answers = std::collections::HashMap<Asked, (Stamp, Option<Turn>)>;

/// More answers than this are forgotten together: each start proof asks
/// with its own bound.
const REMEMBERED: usize = 1024;

/// `turn_of` of the file `key` names, read only when its stamp changed
/// since it was last answered. The stamp is taken before the read, so a
/// write during it is read again next time.
fn remembered(
    key: Asked,
    load: &mut dyn FnMut() -> Result<Option<String>>,
    turn_of: impl FnOnce(&str) -> Option<Turn>,
) -> Result<Option<Turn>> {
    let Some(stamp) = stamp(&key.path)? else { return Ok(None) };
    let memo = |read: &Option<Answers>| -> Option<Option<Turn>> {
        let (known, turn) = read.as_ref()?.get(&key)?;
        (*known == stamp).then_some(*turn)
    };
    if let Some(turn) = memo(&READ.lock().unwrap_or_else(|e| e.into_inner())) {
        return Ok(turn);
    }
    #[cfg(test)]
    LOADS.with(|loads| loads.set(loads.get() + 1));
    let Some(text) = load()? else { return Ok(None) };
    let turn = turn_of(&text);
    let mut read = READ.lock().unwrap_or_else(|e| e.into_inner());
    let read = read.get_or_insert_with(Default::default);
    if read.len() >= REMEMBERED {
        read.clear();
    }
    read.insert(key, (stamp, turn));
    Ok(turn)
}

#[cfg(test)]
thread_local! {
    /// How many session files this thread read for a turn.
    static LOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn stamp(path: &Path) -> Result<Option<Stamp>> {
    match path.metadata() {
        Ok(meta) => {
            let at = meta
                .modified()
                .ok()
                .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
                .map_or(0, |at| at.as_nanos());
            Ok(Some((meta.len(), at)))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

/// A reader of whole texts, for a question that is not remembered.
fn whole<T>(
    mut read: impl FnMut(&str) -> Option<T>,
) -> impl FnMut(&Path, &mut dyn FnMut() -> Result<Option<String>>) -> Result<Option<T>> {
    move |_, load| Ok(load()?.and_then(|text| read(&text)))
}

/// What `read` makes of each session file `layout` names under `root`.
fn read_layout<T>(
    root: &Path,
    harness: Harness,
    cwd: &str,
    layout: SessionLayout,
    since: Option<u64>,
    read: impl FnMut(&Path, &mut dyn FnMut() -> Result<Option<String>>) -> Result<Option<T>>,
) -> Result<Vec<T>> {
    match layout {
        SessionLayout::Files { dir, extension } => {
            read_files(&root.join(dir), extension, since, read)
        }
        SessionLayout::Tree { dir, extension } => {
            read_tree(&root.join(dir), extension, since, read)
        }
        SessionLayout::Named { dir, name, compressed, .. } => {
            read_named(&root.join(dir), name, compressed, since, read)
        }
        SessionLayout::Index { file } => read_index(root, &file, harness, cwd, since, read),
    }
}

fn read_files<T>(
    dir: &Path,
    extension: &str,
    since: Option<u64>,
    mut turn_of: impl FnMut(&Path, &mut dyn FnMut() -> Result<Option<String>>) -> Result<Option<T>>,
) -> Result<Vec<T>> {
    let Some(entries) = list_dir(dir)? else { return Ok(Vec::new()) };
    let mut turns = Vec::new();
    for path in entries {
        if path.extension().and_then(|ext| ext.to_str()) != Some(extension) {
            continue;
        }
        if !fresh(&path, since)? {
            continue;
        }
        if let Some(turn) = turn_of(&path, &mut || read_text(&path))? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn read_tree<T>(
    dir: &Path,
    extension: &str,
    since: Option<u64>,
    mut turn_of: impl FnMut(&Path, &mut dyn FnMut() -> Result<Option<String>>) -> Result<Option<T>>,
) -> Result<Vec<T>> {
    let mut files = Vec::new();
    collect(dir, extension, tree_floor(since), &mut files)?;
    let mut turns = Vec::new();
    for path in files {
        if let Some(turn) = turn_of(&path, &mut || read_text(&path))? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn read_named<T>(
    dir: &Path,
    name: &str,
    compressed: bool,
    since: Option<u64>,
    mut turn_of: impl FnMut(&Path, &mut dyn FnMut() -> Result<Option<String>>) -> Result<Option<T>>,
) -> Result<Vec<T>> {
    let Some(entries) = list_dir(dir)? else { return Ok(Vec::new()) };
    let mut turns = Vec::new();
    for session in entries {
        let path = session.join(name);
        if !path.is_file() || !fresh(&path, since)? {
            continue;
        }
        let mut load = || if compressed { read_zstd(&path) } else { read_text(&path) };
        if let Some(turn) = turn_of(&path, &mut load)? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn read_index<T>(
    root: &Path,
    file: &str,
    harness: Harness,
    cwd: &str,
    since: Option<u64>,
    mut turn_of: impl FnMut(&Path, &mut dyn FnMut() -> Result<Option<String>>) -> Result<Option<T>>,
) -> Result<Vec<T>> {
    let Some(text) = read_text(&root.join(file))? else { return Ok(Vec::new()) };
    let brief = brief(Path::new(cwd));
    let mut turns = Vec::new();
    for session in harness.indexed_sessions(cwd, &text) {
        // A session filed under another directory is this worktree's only
        // when prompted with its brief. With no start given, as for a tree,
        // only the last two days of the whole index are opened.
        let (floor, brief) = if session.here {
            (since, None)
        } else {
            let Some(brief) = &brief else { continue };
            (Some(tree_floor(since)), Some(brief))
        };
        let path = PathBuf::from(&session.file);
        let path = if path.is_absolute() { path } else { root.join(path) };
        if !fresh(&path, floor)? {
            continue;
        }
        let mut load = || {
            let body = read_text(&path)?;
            Ok(body.filter(|body| brief.is_none_or(|brief| harness.names_brief(body, brief))))
        };
        if let Some(turn) = turn_of(&path, &mut load)? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn write_harness(home: &Path, harness: Harness, cwd: &str, open: bool, at: u64) -> Result<()> {
    write_session(&home.join(harness.session_place(cwd).under_home), harness, cwd, open, at)
}

/// One turn, written as `harness` writes it into its directory `root`.
fn write_session(root: &Path, harness: Harness, cwd: &str, open: bool, at: u64) -> Result<()> {
    let root = root.to_path_buf();
    let place = harness.session_place(cwd);
    let files = harness.session_files(cwd, open, at);
    for file in &files {
        if let Some(name) = &file.replaces {
            remove_named(&root, name)?;
        }
    }
    for file in &files {
        let path = root.join(&file.path);
        if file.compressed {
            write_zstd(&path, &file.body, at)?;
        } else {
            write_text(&path, &file.body, at)?;
        }
    }
    if let SessionLayout::Index { file } = place.layout {
        let path = root.join(file);
        let existing = read_text(&path)?.unwrap_or_default();
        if let Some(body) = harness.session_index(cwd, &existing) {
            write_text(&path, &body, at)?;
        }
    }
    Ok(())
}

fn cwd_str(cwd: &Path) -> String {
    cwd.to_string_lossy().into_owned()
}

/// A tree of sessions is not read whole when the caller gave no start: two
/// days covers the turn a worker is in. A given start drops a file last
/// written before it, with the same two seconds `fresh` allows.
fn tree_floor(since: Option<u64>) -> u64 {
    since.unwrap_or_else(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
            .saturating_sub(2 * 86_400)
    })
}

fn fresh(path: &Path, since: Option<u64>) -> Result<bool> {
    let Some(at) = modified(path)? else { return Ok(false) };
    let floor = match since {
        Some(since) => since.saturating_sub(2),
        None => 0,
    };
    Ok(at + 2 >= floor)
}

fn modified(path: &Path) -> Result<Option<u64>> {
    match path.metadata() {
        Ok(meta) => Ok(meta
            .modified()
            .ok()
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map(|at| at.as_secs())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn list_dir(path: &Path) -> Result<Option<Vec<PathBuf>>> {
    let read = match std::fs::read_dir(path) {
        Ok(read) => read,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", path.display()));
        }
    };
    let mut out = Vec::new();
    for entry in read {
        out.push(entry.with_context(|| format!("cannot read {}", path.display()))?.path());
    }
    Ok(Some(out))
}

fn collect(dir: &Path, extension: &str, floor: u64, out: &mut Vec<PathBuf>) -> Result<()> {
    let Some(entries) = list_dir(dir)? else { return Ok(()) };
    for entry in entries {
        if entry.is_dir() {
            collect(&entry, extension, floor, out)?;
            continue;
        }
        if entry.extension().and_then(|ext| ext.to_str()) != Some(extension) {
            continue;
        }
        if modified(&entry)?.is_some_and(|at| at + 2 >= floor) {
            out.push(entry);
        }
    }
    Ok(())
}

fn read_text(path: &Path) -> Result<Option<String>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("cannot read {}", path.display())),
    }
}

fn read_zstd(path: &Path) -> Result<Option<String>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", path.display()));
        }
    };
    let mut decoder = zstd::stream::read::Decoder::new(file)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let mut text = String::new();
    decoder.read_to_string(&mut text).with_context(|| format!("cannot read {}", path.display()))?;
    Ok(Some(text))
}

fn write_text(path: &Path, text: &str, at: u64) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
    dated(path, at)
}

fn write_zstd(path: &Path, text: &str, at: u64) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("cannot write {}", path.display()))?;
    }
    let file =
        std::fs::File::create(path).with_context(|| format!("cannot write {}", path.display()))?;
    let mut encoder = zstd::stream::write::Encoder::new(file, 0)
        .with_context(|| format!("cannot write {}", path.display()))?;
    encoder
        .write_all(text.as_bytes())
        .with_context(|| format!("cannot write {}", path.display()))?;
    encoder.finish().with_context(|| format!("cannot write {}", path.display()))?;
    dated(path, at)
}

/// A stamp ahead of the wall clock is the fleet's own clock. The start
/// proof skips a file last written before the handoff, so the file has to
/// carry that stamp as its mtime or the turn is never read.
fn dated(path: &Path, at: u64) -> Result<()> {
    let wall = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    if at <= wall {
        return Ok(());
    }
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .with_context(|| format!("cannot date {}", path.display()))?;
    file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(at))
        .with_context(|| format!("cannot date {}", path.display()))
}

fn remove_named(dir: &Path, name: &str) -> Result<()> {
    let Some(entries) = list_dir(dir)? else { return Ok(()) };
    for entry in entries {
        if entry.is_dir() {
            remove_named(&entry, name)?;
            continue;
        }
        if entry.file_name().and_then(|file| file.to_str()) == Some(name) {
            std::fs::remove_file(&entry)
                .with_context(|| format!("cannot replace {}", entry.display()))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_a_comment_write_receipt_identifies_the_workers_own_comment() {
        use serde_json::json;
        let url = "https://github.com/acme/tasks/issues/7#issuecomment-100";
        let calls = [
            (
                Harness::Codex,
                json!({"type":"response_item", "payload":{"type":"function_call", "call_id":"write", "name":"exec", "arguments":"await tools.exec_command({cmd: 'gh issue comment 7 --body-file note.md'})"}}),
                json!({"type":"response_item", "payload":{"type":"function_call_output", "call_id":"write", "output":url}}),
            ),
            (
                Harness::ClaudeCode,
                json!({"type":"assistant", "message":{"role":"assistant", "content":[{"type":"tool_use", "id":"write", "name":"Bash", "input":{"command":"gh api repos/acme/tasks/issues/7/comments -f body=done"}}]}}),
                json!({"type":"user", "message":{"role":"user", "content":[{"type":"tool_result", "tool_use_id":"write", "content":url}]}}),
            ),
            (
                Harness::Pi,
                json!({"type":"message", "message":{"role":"assistant", "content":[{"type":"toolCall", "id":"write", "name":"bash", "arguments":{"command":"gh issue comment 7 --body done"}}]}}),
                json!({"type":"message", "message":{"role":"toolResult", "toolCallId":"write", "content":[{"type":"text","text":url}]}}),
            ),
            (
                Harness::Kimi,
                json!({"type":"context.append_loop_event", "agentId":"main", "event":{"type":"tool.call", "toolCallId":"write", "name":"Bash", "args":{"command":"gh issue comment 7 --body done"}}}),
                json!({"type":"context.append_loop_event", "agentId":"main", "event":{"type":"tool.result", "toolCallId":"write", "result":{"output":url, "isError":false}}}),
            ),
        ];
        for (harness, call, result) in calls {
            assert!(super::posted_in(harness, &format!("{call}\n{result}\n"), url), "{harness:?}");
            assert!(!super::posted_in(
                harness,
                &format!("{call}\n{}\n", result.to_string().replace(url, &format!("{url}0"))),
                url
            ));
            assert!(
                !super::posted_in(harness, &result.to_string(), url),
                "a link alone proves no write"
            );
            let read = call
                .to_string()
                .replace("issue comment", "issue view")
                .replace("/comments -f", "/comments --jq");
            assert!(
                !super::posted_in(harness, &format!("{read}\n{result}\n"), url),
                "a read proves no write"
            );
        }
        let quoted = json!({"type":"user", "message":{"role":"user","content":format!("gh issue comment 7: {url}")}});
        assert!(!super::posted_in(Harness::ClaudeCode, &quoted.to_string(), url));
    }

    use super::*;

    const AT: u64 = 1_793_106_373;

    #[test]
    fn a_claude_role_session_belongs_to_the_worker_named_by_its_first_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("work");
        let gitdir = dir.path().join("git/worktrees/work");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(cwd.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        let brief = gitdir.join("convoy-worker-prompt.md");
        let prompt = afkconvoy_core::prompt::brief_pointer(&brief.display().to_string());
        let root = dir.path().join("claude");
        let role = "/cache/copies/acme/team/hash-role+claude-code-roles%2Fbuilder";
        let harness = Harness::ClaudeCode;
        let session = &harness.session_files(role, true, AT)[0];
        let file = root.join(&session.path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let body = |message: &str, open| {
            harness.session_files(role, open, AT)[0]
                .body
                .replace("\"brief\"", &serde_json::to_string(message).unwrap())
        };
        let read = |body: &str, not_before| {
            std::fs::write(&file, body).unwrap();
            read_root(&root, harness, &cwd, None, not_before).unwrap()
        };
        let started = body(&prompt, true);
        assert_eq!(read(&started, Some(AT)), Some(Turn::Open { since: AT }));
        assert_eq!(read(&started, None), Some(Turn::Open { since: AT }));
        assert_eq!(read(&started, Some(AT + 1)), None, "an old turn cannot prove this send");
        assert_eq!(read("", Some(AT)), None, "no turn was written");
        assert_eq!(read(&body("no brief", true), Some(AT)), None);
        let other = prompt.replace("/work/convoy-", "/other/convoy-");
        assert_ne!(other, prompt);
        assert_eq!(read(&body(&other, true), Some(AT)), None);
        assert_eq!(
            read(&format!("{}{started}", body(&other, true)), Some(AT)),
            None,
            "a later mention cannot take another worker's session"
        );
        assert_eq!(
            read(&body(&prompt, false), Some(AT)),
            Some(Turn::Closed { since: AT, started: Some(AT) })
        );
        assert_eq!(
            read(&started.replace("\"isSidechain\":false", "\"isSidechain\":true"), Some(AT)),
            None,
            "a child cannot prove the worker's start"
        );
    }

    #[test]
    fn a_codex_role_session_belongs_to_the_worker_named_by_its_first_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("work");
        let gitdir = dir.path().join("git/worktrees/work");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::write(cwd.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        let brief = gitdir.join("convoy-worker-prompt.md");
        let prompt = afkconvoy_core::prompt::brief_pointer(&brief.display().to_string());
        let root = dir.path().join("codex");
        let file = root.join("sessions/2026/10/27/rollout-role.jsonl");
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        let rollout = |message: &str, start: Option<u64>| {
            let mut lines = vec![
                serde_json::json!({"type": "session_meta", "payload": {
                    "cwd": "/cache/copies/acme/team/hash-role+codex-roles%2Fbuilder"}}),
                // Codex's real rollout puts tool definitions and environment
                // messages before the brief, even within its first turn.
                serde_json::json!({"type": "response_item", "payload": {
                    "type": "additional_tools", "role": "developer", "tools": []}}),
                serde_json::json!({"type": "response_item", "payload": {
                    "type": "message", "role": "user", "content": [
                        {"type": "input_text", "text": "# AGENTS.md instructions for /role"},
                        {"type": "input_text", "text": "<environment_context>"}]}}),
                serde_json::json!({"type": "response_item", "payload": {
                    "type": "message", "role": "user", "content": [
                        {"type": "input_text", "text": message}]}}),
            ];
            if let Some(at) = start {
                lines.push(serde_json::json!({"type": "event_msg", "payload": {
                    "type": "task_started", "started_at": at}}));
            }
            lines.iter().map(|line| format!("{line}\n")).collect::<String>()
        };
        let read = |body: &str, not_before| {
            std::fs::write(&file, body).unwrap();
            read_root(&root, Harness::Codex, &cwd, None, not_before).unwrap()
        };
        let body = rollout(&prompt, Some(AT));
        assert_eq!(read(&body, Some(AT)), Some(Turn::Open { since: AT }));
        assert_eq!(read(&body, None), Some(Turn::Open { since: AT }));
        assert_eq!(read(&body, Some(AT + 1)), None, "an old turn cannot prove this send");
        assert_eq!(read(&rollout(&prompt, None), Some(AT)), None, "a prompt is not a turn");
        let no_header = body.lines().skip(1).collect::<Vec<_>>().join("\n");
        assert_eq!(read(&no_header, Some(AT)), None, "a header is still required");
        let complete = serde_json::json!({"type": "event_msg", "payload": {
            "type": "task_complete", "completed_at": AT + 3}});
        assert_eq!(
            read(&format!("{body}{complete}\n"), Some(AT)),
            Some(Turn::Closed { since: AT + 3, started: Some(AT) }),
            "the same role session also records when its worker stops"
        );
        let event = serde_json::json!({"type": "event_msg", "payload": {
            "type": "user_message", "message": prompt}});
        let empty = rollout("", Some(AT));
        assert_eq!(read(&format!("{empty}{event}\n"), Some(AT)), Some(Turn::Open { since: AT }));
        for payload in [
            serde_json::json!({"type": "function_call_output", "output": prompt}),
            serde_json::json!({"type": "message", "role": "assistant", "content": [
                {"type": "output_text", "text": prompt}]}),
        ] {
            let reply = serde_json::json!({"type": "response_item", "payload": payload});
            assert_eq!(
                read(&format!("{empty}{reply}\n{event}\n"), Some(AT)),
                None,
                "neither a reply nor a later user mention can associate a session"
            );
        }
        let other = prompt.replace("/work/convoy-", "/other/convoy-");
        assert_ne!(other, prompt);
        assert_eq!(read(&rollout(&other, Some(AT)), Some(AT)), None);
        let mut wrong_worker = rollout(&other, Some(AT));
        wrong_worker.push_str(&format!(
            "{}\n",
            serde_json::json!({"type": "response_item",
            "payload": {"type": "message", "role": "user", "content": [
                {"type": "input_text", "text": prompt}]}})
        ));
        assert_eq!(
            read(&wrong_worker, Some(AT)),
            None,
            "a later mention cannot take another worker's session"
        );
    }

    /// An unbound Kimi or Grok worker, and a watch on either, is launched
    /// with a home of its own under its worktree (`kept::outside_role`), and
    /// the harness records its turn there. The start proof reads it, or a
    /// worker at work is a failed start (sunfmin/control#504).
    #[test]
    fn a_turn_in_the_home_a_session_outside_a_role_was_given_is_the_turn() {
        for harness in [Harness::Kimi, Harness::Grok] {
            let dir = tempfile::tempdir().unwrap();
            let cwd = dir.path().join("work");
            std::fs::create_dir(&cwd).unwrap();
            let home = crate::kept::outside_role(harness, &cwd, None).unwrap();
            let home = Path::new(&home);
            assert!(home.starts_with(&cwd), "{}: {}", harness.key(), home.display());
            let read = || read_turn(&cwd, harness, Some(AT - 2), Some(AT - 2)).unwrap();
            assert_eq!(read(), None, "{}: no turn recorded is no start", harness.key());

            write_session(home, harness, &cwd_str(&cwd), true, AT).unwrap();
            assert_eq!(read(), Some(Turn::Open { since: AT }), "{}", harness.key());
        }
    }

    #[test]
    fn every_harness_round_trips_an_open_and_a_closed_turn() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        record(&home, &cwd, false, AT).unwrap();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            let turn = read_root(&root, harness, &cwd, None, Some(AT)).unwrap();
            assert!(
                matches!(turn, Some(Turn::Closed { since: AT, started: Some(AT) })),
                "{} closed: {turn:?}",
                harness.key()
            );
        }
        record(&home, &cwd, true, AT).unwrap();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            let turn = read_root(&root, harness, &cwd, None, Some(AT - 2)).unwrap();
            assert!(
                matches!(turn, Some(Turn::Open { since: AT })),
                "{} open: {turn:?}",
                harness.key()
            );
            let earlier = read_root(&root, harness, &cwd, None, Some(AT + 1)).unwrap();
            assert_eq!(earlier, None, "{} must not prove a start before the bound", harness.key());
        }
    }

    #[test]
    fn a_long_worktree_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let mut cwd = dir.path().to_path_buf();
        for i in 0..40 {
            cwd.push(format!("segment-{i:02}"));
        }
        std::fs::create_dir_all(&cwd).unwrap();
        let home = dir.path().join("home");
        record(&home, &cwd, false, AT).unwrap();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            let turn = read_root(&root, harness, &cwd, None, None).unwrap();
            assert_eq!(
                turn,
                Some(Turn::Closed { since: AT, started: Some(AT) }),
                "{}",
                harness.key()
            );
        }
    }

    #[test]
    fn a_noted_home_is_the_session_for_its_worktree() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        let stored = dir.path().join("stored");
        record(&stored, &cwd, true, AT).unwrap();
        let place = Harness::Codex.session_place(&cwd_str(&cwd));
        let gitdir = dir.path().join("gitdir");
        std::fs::create_dir(&gitdir).unwrap();
        std::fs::write(cwd.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
        let note = Harness::Codex.noted_home().unwrap();
        std::fs::write(gitdir.join(note), stored.join(place.under_home).display().to_string())
            .unwrap();
        let turn = read_turn(&cwd, Harness::Codex, None, Some(AT - 2)).unwrap();
        assert_eq!(turn, Some(Turn::Open { since: AT }));
    }

    /// A Kimi worker starts in its role copy, so its index line names that
    /// directory; its wire's prompt names the brief in the worktree's git
    /// dir. Seen on 2026-10-28 (sunfmin/control#381).
    #[test]
    fn a_kimi_session_under_the_role_copy_prompted_with_this_brief_is_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("kimi-role");
        let worktree = |name: &str| {
            let cwd = dir.path().join("orca").join(name);
            let gitdir = dir.path().join("repo/.git/worktrees").join(name);
            std::fs::create_dir_all(&cwd).unwrap();
            std::fs::create_dir_all(&gitdir).unwrap();
            std::fs::write(cwd.join(".git"), format!("gitdir: {}\n", gitdir.display())).unwrap();
            (cwd, gitdir.join(crate::orca::BRIEF_FILE))
        };
        let (cwd, brief) = worktree("issue-7");
        let (other, other_brief) = worktree("issue-70");
        let role = "/cache/afkconvoy/copies/acme/team/abc-role+kimi-roles%2Fbuilder";
        let index =
            serde_json::json!({"sessionId": "s", "sessionDir": "sessions/s", "workDir": role});
        write_text(&root.join("session_index.jsonl"), &format!("{index}\n"), AT).unwrap();
        let wire = root.join("sessions/s/agents/main/wire.jsonl");
        let ms = AT * 1000;
        let prompted = |brief: &Path, started: u64| {
            let prompt = afkconvoy_core::prompt::brief_pointer(&brief.display().to_string());
            format!(
                "{}\n{}\n",
                serde_json::json!({"type": "turn.prompt", "time": started, "prompt": prompt}),
                serde_json::json!({"type": "agent.turn.started", "time": started + 130}),
            )
        };
        write_text(&wire, &prompted(&brief, ms), AT).unwrap();
        let read = |cwd: &Path, not_before| read_root(&root, Harness::Kimi, cwd, None, not_before);
        assert_eq!(read(&cwd, Some(AT - 2)).unwrap(), Some(Turn::Open { since: AT }));
        assert_eq!(read(&cwd, Some(AT + 1)).unwrap(), None, "a turn before this send");
        assert_eq!(read(&other, Some(AT - 2)).unwrap(), None, "another worktree's start");

        write_text(&wire, &prompted(&other_brief, ms), AT).unwrap();
        assert_eq!(read(&cwd, Some(AT - 2)).unwrap(), None, "another worktree's session");
        assert_eq!(read(&other, Some(AT - 2)).unwrap(), Some(Turn::Open { since: AT }));

        let unprompted = serde_json::json!({"type": "agent.turn.started", "time": ms});
        write_text(&wire, &format!("{unprompted}\n"), AT).unwrap();
        assert_eq!(read(&cwd, Some(AT - 2)).unwrap(), None, "no brief, no worktree");
    }

    #[test]
    fn the_primary_session_that_started_last_is_the_turn() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        let harness = Harness::Grok;
        let cwd_text = cwd_str(&cwd);
        let root = home.join(harness.session_place(&cwd_text).under_home);
        let killed = grok_body("2026-10-27T13:06:13Z", None, "primary");
        let finished = grok_body("2026-10-27T13:06:53Z", Some("2026-10-27T13:07:13Z"), "primary");
        grok_file(&root, &cwd, "earlier", &killed);
        grok_file(&root, &cwd, "later", &finished);
        let turn = read_root(&root, harness, &cwd, None, None).unwrap();
        assert_eq!(turn, harness.recorded_turn(&cwd_text, &finished, None, None));
        assert!(matches!(turn, Some(Turn::Closed { .. })));
        assert!(matches!(
            harness.recorded_turn(&cwd_text, &killed, None, None),
            Some(Turn::Open { .. })
        ));

        let earlier = grok_body("2026-10-27T13:06:13Z", Some("2026-10-27T13:06:33Z"), "primary");
        let later = grok_body("2026-10-27T13:06:53Z", None, "primary");
        grok_file(&root, &cwd, "earlier", &earlier);
        grok_file(&root, &cwd, "later", &later);
        let turn = read_root(&root, harness, &cwd, None, None).unwrap();
        assert_eq!(turn, harness.recorded_turn(&cwd_text, &later, None, None));
        assert!(matches!(turn, Some(Turn::Open { .. })));
        assert!(matches!(
            harness.recorded_turn(&cwd_text, &earlier, None, None),
            Some(Turn::Closed { .. })
        ));

        let child = grok_body("2026-10-27T13:08:00Z", Some("2026-10-27T13:08:10Z"), "subagent");
        grok_file(&root, &cwd, "child", &child);
        let turn = read_root(&root, harness, &cwd, None, None).unwrap();
        assert_eq!(turn, harness.recorded_turn(&cwd_text, &later, None, None));
        assert_eq!(harness.recorded_turn(&cwd_text, &child, None, None), None);

        let stale = grok_body("2026-10-27T14:00:00Z", None, "primary");
        let stale_path = grok_file(&root, &cwd, "stale", &stale);
        let file = std::fs::OpenOptions::new().write(true).open(&stale_path).unwrap();
        file.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1_000)).unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        let turn = read_root(&root, harness, &cwd, Some(now.saturating_sub(30)), None).unwrap();
        assert_eq!(
            turn,
            harness.recorded_turn(&cwd_text, &later, None, None),
            "a file last written before the handoff is not this turn"
        );
    }

    fn grok_file(root: &Path, cwd: &Path, session: &str, body: &str) -> PathBuf {
        let place = Harness::Grok.session_place(&cwd_str(cwd));
        let SessionLayout::Named { dir, name, .. } = place.layout else {
            panic!("grok keeps a named session");
        };
        let path = root.join(dir).join(session).join(name);
        write_text(&path, body, 0).unwrap();
        path
    }

    fn grok_body(start: &str, end: Option<&str>, relationship: &str) -> String {
        let mut body = format!("{}\n", grok_line(start, "turn_started", relationship));
        if end.is_none() && relationship == "primary" {
            body.push_str(&format!(
                "{}\n",
                grok_line("2026-10-27T13:06:23Z", "waiting_for_model", relationship)
            ));
        }
        if let Some(end) = end {
            body.push_str(&format!("{}\n", grok_line(end, "turn_ended", relationship)));
        }
        body
    }

    fn grok_line(at: &str, kind: &str, relationship: &str) -> String {
        format!(r#"{{"session_relationship":"{relationship}","ts":"{at}","type":"{kind}"}}"#)
    }

    #[test]
    fn a_missing_session_directory_is_no_turn() {
        let dir = tempfile::tempdir().unwrap();
        for harness in Harness::ALL {
            let turn = read_root(dir.path(), harness, Path::new("/nowhere"), None, None).unwrap();
            assert_eq!(turn, None, "{}", harness.key());
        }
    }

    #[test]
    fn a_session_written_before_the_worker_started_is_not_this_turn() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        record(&home, &cwd, true, AT).unwrap();
        let now = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            let stale = read_root(&root, harness, &cwd, Some(now + 60), Some(AT - 2)).unwrap();
            assert_eq!(stale, None, "{}", harness.key());
            let kept = read_root(&root, harness, &cwd, Some(now.saturating_sub(60)), Some(AT - 2))
                .unwrap();
            assert!(matches!(kept, Some(Turn::Open { since: AT })), "{}: {kept:?}", harness.key());
        }
    }

    #[test]
    fn another_worktree_is_not_this_turn() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("work");
        let other = dir.path().join("other");
        std::fs::create_dir(&cwd).unwrap();
        std::fs::create_dir(&other).unwrap();
        record(&home, &cwd, true, AT).unwrap();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            let turn = read_root(&root, harness, &other, None, None).unwrap();
            assert_eq!(turn, None, "{}", harness.key());
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_session_file_that_cannot_be_read_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        record(&home, &cwd, true, AT).unwrap();
        for harness in Harness::ALL {
            let root = home.join(harness.session_place(&cwd_str(&cwd)).under_home);
            deny_files(&root);
            let err = read_root(&root, harness, &cwd, None, None).unwrap_err();
            assert!(err.to_string().contains("cannot read"), "{}: {err}", harness.key());
            allow_files(&root);
        }
    }

    #[cfg(unix)]
    fn deny_files(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                deny_files(&path);
            } else {
                let mut perms = std::fs::metadata(&path).unwrap().permissions();
                perms.set_mode(0o0);
                std::fs::set_permissions(&path, perms).unwrap();
            }
        }
    }

    #[cfg(unix)]
    fn allow_files(dir: &Path) {
        use std::os::unix::fs::PermissionsExt;
        let Ok(read) = std::fs::read_dir(dir) else { return };
        for entry in read.flatten() {
            let path = entry.path();
            if path.is_dir() {
                allow_files(&path);
            } else {
                let mut perms = std::fs::metadata(&path).unwrap().permissions();
                perms.set_mode(0o644);
                std::fs::set_permissions(&path, perms).unwrap();
            }
        }
    }
}

/// Every directory `harness` may keep a worker's sessions under in `cwd`
/// (`roots`), with each earlier launch's home: a harness that keeps each
/// session apart (`Harness::session`) makes a fresh home for each launch,
/// and the note names only the latest. Earlier homes remain in the worktree
/// until it is removed.
fn worker_homes(cwd: &Path, harness: Harness) -> Result<Vec<PathBuf>> {
    let mut homes = roots(cwd, harness)?;
    if harness.session().is_some()
        && let Some(entries) = list_dir(&cwd.join(crate::worker_tmp::TMP_DIR))?
    {
        for entry in entries {
            for name in kept_names(harness) {
                let home = entry.join("harness").join(name);
                if home.is_dir() {
                    homes.push(home);
                }
            }
        }
    }
    homes.sort();
    homes.dedup();
    Ok(homes)
}

/// The session record of the worker started in `cwd` at `start`, and before
/// the next worker's `before`, where its harness keeps a goal: the home it
/// is under, its path and what it says of its goal. Of several, the one
/// begun last — a relaunch's — is the worker's last goal.
pub(crate) fn goal_session(
    cwd: &Path,
    harness: Harness,
    start: u64,
    before: Option<u64>,
) -> Result<Option<(PathBuf, PathBuf, afkconvoy_core::harness::GoalSession)>> {
    if harness.goal_store().is_none() {
        return Ok(None);
    }
    let cwd_text = cwd_str(cwd);
    let brief = brief(cwd);
    let SessionLayout::Tree { dir, extension } = harness.session_place(&cwd_text).layout else {
        return Ok(None);
    };
    let mut found: Option<(u64, PathBuf, PathBuf, String)> = None;
    for home in worker_homes(cwd, harness)? {
        let mut files = Vec::new();
        collect(&home.join(&dir), extension, start.saturating_sub(60), &mut files)?;
        for file in files {
            let Some(text) = read_text(&file)? else { continue };
            let Some(at) = session_first_at(&text) else { continue };
            if at < start.saturating_sub(60)
                || before.is_some_and(|end| at >= end.saturating_sub(2))
            {
                continue;
            }
            if harness.recorded_turn(&cwd_text, &text, None, brief.as_deref()).is_none() {
                continue;
            }
            if found.as_ref().is_none_or(|(last, ..)| at >= *last) {
                found = Some((at, home.clone(), file, text));
            }
        }
    }
    Ok(found.and_then(|(_, home, file, text)| Some((home, file, harness.goal_session(&text)?))))
}

/// Read the usage of sessions started for one worker. Missing or unreadable
/// records are unknown; an unavailable session never becomes a zero count.
pub(crate) fn tokens(
    cwd: &Path,
    harness: Harness,
    start: u64,
    before: Option<u64>,
) -> Result<Option<afkconvoy_core::account::Tokens>> {
    let cwd_text = cwd_str(cwd);
    let brief = brief(cwd);
    usage(&worker_homes(cwd, harness)?, harness, &cwd_text, start, None, |text| {
        let Some(at) = session_first_at(text) else { return false };
        at >= start.saturating_sub(60)
            && !before.is_some_and(|end| at >= end.saturating_sub(2))
            && harness.recorded_turn(&cwd_text, text, None, brief.as_deref()).is_some()
    })
}

/// The usage windows of its account that the newest record of the worker in
/// `cwd` says, on `harness`, with when that record was written: what
/// admission reads of the account (`Harness::windows`). Files last written
/// before `since` are not opened. `None`: no record of it says any — its
/// harness reports none, or has not yet.
pub(crate) fn windows(
    cwd: &Path,
    harness: Harness,
    since: u64,
) -> Result<Option<(u64, Vec<afkconvoy_core::harness::window::Window>)>> {
    let cwd_text = cwd_str(cwd);
    let brief = brief(cwd);
    let layout = harness.session_place(&cwd_text).layout;
    // A tree of sessions holds every worktree's: a session there must record
    // this worktree's turn.
    let tree = matches!(layout, SessionLayout::Tree { .. });
    let mut newest: Option<(u64, Vec<_>)> = None;
    for home in worker_homes(cwd, harness)? {
        let found = read_layout(
            &home,
            harness,
            &cwd_text,
            layout.clone(),
            Some(since),
            whole(|text| {
                if tree && harness.recorded_turn(&cwd_text, text, None, brief.as_deref()).is_none()
                {
                    return None;
                }
                last_windows(harness, text)
            }),
        )?;
        for (at, windows) in found {
            if newest.as_ref().is_none_or(|(newer, _)| at > *newer) {
                newest = Some((at, windows));
            }
        }
    }
    Ok(newest)
}

/// The last record of `text` that says windows, with when it was written.
fn last_windows(
    harness: Harness,
    text: &str,
) -> Option<(u64, Vec<afkconvoy_core::harness::window::Window>)> {
    text.lines().rev().find_map(|line| {
        let value: serde_json::Value = serde_json::from_str(line).ok()?;
        let windows = harness.windows(&value);
        if windows.is_empty() {
            return None;
        }
        Some((session_first_at(line).unwrap_or(0), windows))
    })
}

/// Read one resident session's usage between delivery and report. The brief
/// root distinguishes the watch from the asking session in their shared
/// directory; the whole session is read so cumulative counters can establish
/// their baseline before `start`.
pub(crate) fn tokens_between(
    cwd: &Path,
    harness: Harness,
    start: u64,
    end: u64,
    brief_root: &Path,
) -> Result<Option<afkconvoy_core::account::Tokens>> {
    let cwd_text = cwd_str(cwd);
    let brief_root = cwd_str(brief_root);
    usage(&roots(cwd, harness)?, harness, &cwd_text, start, Some((start, end)), |text| {
        text.contains(&brief_root) && harness.recorded_turn(&cwd_text, text, None, None).is_some()
    })
}

/// The usage `harness` recorded in the sessions under `homes` that `accepts`
/// takes, over `window` or the whole session (`Harness::tokens`). A session
/// that keeps its usage in a file beside it is read from that file. Files
/// last written before `since` are not opened.
fn usage(
    homes: &[PathBuf],
    harness: Harness,
    cwd: &str,
    since: u64,
    window: Option<(u64, u64)>,
    accepts: impl Fn(&str) -> bool,
) -> Result<Option<afkconvoy_core::account::Tokens>> {
    let mut total = afkconvoy_core::account::Tokens::new();
    let mut found = false;
    let layout = harness.session_place(cwd).layout;
    for root in homes {
        let mut texts = Vec::new();
        if let SessionLayout::Named { dir, name, compressed, usage: Some(beside) } = &layout {
            for entry in list_dir(&root.join(dir))?.unwrap_or_default() {
                let path = entry.join(name);
                let text = if *compressed { read_zstd(&path)? } else { read_text(&path)? };
                if text.is_some_and(|text| accepts(&text))
                    && let Some(usage) = read_text(&entry.join(beside))?
                {
                    texts.push(usage);
                }
            }
        } else {
            texts = read_layout(
                root,
                harness,
                cwd,
                layout.clone(),
                Some(since),
                whole(|text| accepts(text).then(|| text.to_owned())),
            )?;
        }
        for text in texts {
            if let Some(tokens) = harness.tokens(&text, window) {
                found = true;
                for (model, counts) in tokens {
                    total.entry(model).or_default().add(&counts);
                }
            }
        }
    }
    Ok(found.then_some(total))
}

fn session_first_at(text: &str) -> Option<u64> {
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else { continue };
        for key in ["timestamp", "time", "ts"] {
            if let Some(n) = value[key].as_u64() {
                return Some(if n > 10_000_000_000 { n / 1000 } else { n });
            }
            if let Some(raw) = value[key].as_str() {
                let whole = raw.split_once('.').map(|(s, _)| format!("{s}Z"));
                if let Ok(at) = crate::doctor::watching::since(whole.as_deref().unwrap_or(raw)) {
                    return Some(at);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod token_tests {
    use super::*;
    use serde_json::json;

    /// The host runner asks for the watch's turn every two seconds: a
    /// session file unchanged since it was answered is not read again, and
    /// one written since is (sunfmin/control#938).
    #[test]
    fn an_unchanged_session_file_is_answered_without_reading_it() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("home");
        let cwd = dir.path().join("work");
        std::fs::create_dir_all(&cwd).unwrap();
        let harness = Harness::ClaudeCode;
        let at = 1_793_106_373;
        let loads = || LOADS.with(std::cell::Cell::get);
        write_session(&root, harness, &cwd_str(&cwd), true, at).unwrap();
        let open = read_root(&root, harness, &cwd, None, None).unwrap();
        assert!(matches!(open, Some(Turn::Open { .. })), "{open:?}");
        let read = loads();
        for _ in 0..20 {
            assert_eq!(read_root(&root, harness, &cwd, None, None).unwrap(), open);
        }
        assert_eq!(loads(), read, "an unchanged file costs a stat");
        write_session(&root, harness, &cwd_str(&cwd), false, at + 5).unwrap();
        let closed = read_root(&root, harness, &cwd, None, None).unwrap();
        assert!(matches!(closed, Some(Turn::Closed { .. })), "{closed:?}");
        assert!(loads() > read, "a written file is read again");
        let other = read_root(&root, harness, &cwd, None, Some(at + 60)).unwrap();
        assert_eq!(other, None, "another bound is its own question");
    }

    #[test]
    fn session_usage_is_unknown_when_missing_or_unreadable() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("work");
        let gitdir = dir.path().join("git");
        let home = dir.path().join("codex");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(home.join("sessions")).unwrap();
        std::fs::write(cwd.join(".git"), format!("gitdir: {}", gitdir.display())).unwrap();
        std::fs::write(gitdir.join("convoy-codex-home"), home.display().to_string()).unwrap();
        let at = 1_793_106_373;
        let file = home.join("sessions/session.jsonl");
        let session = Harness::Codex.session_files(&cwd_str(&cwd), true, at)[0].body.clone();
        std::fs::write(&file, &session).unwrap();
        assert_eq!(tokens(&cwd, Harness::Codex, at, None).unwrap(), None);
        let usage = json!({"type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":30,"output_tokens":10}}}});
        std::fs::write(&file, format!("{session}\n{usage}\n")).unwrap();
        let counts = tokens(&cwd, Harness::Codex, at, None).unwrap().unwrap();
        assert_eq!(counts[""].input, 70);
        assert_eq!(counts[""].cache_read, 30);
        std::fs::remove_file(&file).unwrap();
        std::fs::remove_dir(home.join("sessions")).unwrap();
        std::fs::write(home.join("sessions"), "not a directory").unwrap();
        assert!(tokens(&cwd, Harness::Codex, at, None).is_err());
    }

    #[test]
    fn a_session_keeping_its_usage_beside_it_is_read_from_that_file() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("work");
        let home = cwd.join(crate::worker_tmp::TMP_DIR).join("guard-1");
        std::fs::create_dir_all(&home).unwrap();
        let at = 1_793_106_373;
        let harness = Harness::Grok;
        let files = harness.session_files(&cwd_str(&cwd), false, at);
        for file in &files {
            write_text(&home.join(&file.path), &file.body, at).unwrap();
        }
        assert_eq!(tokens(&cwd, harness, at, None).unwrap(), None);
        let SessionLayout::Named { usage: Some(beside), name, .. } =
            harness.session_place(&cwd_str(&cwd)).layout
        else {
            panic!("grok keeps its usage beside its events");
        };
        let events = files.iter().find(|file| file.path.ends_with(name)).unwrap();
        let updates = home.join(&events.path).with_file_name(beside);
        let usage = json!({"params":{"update":{"usage":{"modelUsage":{"g":{"inputTokens":50,"cachedReadTokens":20,"outputTokens":5}}}}}});
        std::fs::write(&updates, format!("{usage}\n")).unwrap();
        let counts = tokens(&cwd, harness, at, None).unwrap().unwrap();
        assert_eq!((counts["g"].input, counts["g"].cache_read, counts["g"].output), (30, 20, 5));
    }
}
