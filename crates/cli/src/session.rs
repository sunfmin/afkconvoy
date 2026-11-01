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
        let at = text.lines().find_map(|line| {
            if !harness.holds_note(line, mark) {
                return None;
            }
            let entry: serde_json::Value = serde_json::from_str(line).ok()?;
            entry["timestamp"].as_u64().or_else(|| {
                let stamp = entry["timestamp"].as_str()?;
                let whole = stamp
                    .split_once('.')
                    .filter(|_| stamp.ends_with('Z'))
                    .map(|(whole, _)| format!("{whole}Z"));
                crate::doctor::watching::since(whole.as_deref().unwrap_or(stamp)).ok()
            })
        });
        if at.is_some() {
            found.set(at);
        }
        at.is_some()
    })?;
    Ok(found.get())
}

/// Reads the harness's message files once for a question about this worker.
/// Global session trees must identify this worktree; project message stores
/// already have its directory in their layout.
fn notes_match(
    cwd: &Path,
    harness: Harness,
    since: u64,
    matches: impl Fn(&str) -> bool,
) -> Result<bool> {
    let cwd_text = cwd_str(cwd);
    let place = harness.note_place(&cwd_text);
    let brief = brief(cwd);
    let holds = |text: &str| {
        let belongs = !matches!(place.layout, SessionLayout::Tree { .. })
            || harness.recorded_turn(&cwd_text, text, Some(since), brief.as_deref()).is_some();
        (belongs && matches(text)).then_some(())
    };
    for root in &roots(cwd, harness)? {
        if !root.exists() {
            continue;
        }
        let since = Some(since);
        let held = match harness.note_place(&cwd_text).layout {
            SessionLayout::Files { dir, extension } => {
                read_files(&root.join(dir), extension, since, holds)?
            }
            SessionLayout::Tree { dir, extension } => {
                read_tree(&root.join(dir), extension, since, holds)?
            }
            SessionLayout::Named { dir, name, compressed } => {
                read_named(&root.join(dir), name, compressed, since, holds)?
            }
            SessionLayout::Index { file } => {
                read_index(root, &file, harness, &cwd_text, since, holds)?
            }
        };
        if !held.is_empty() {
            return Ok(true);
        }
        // A bound Claude starts in its role copy and records the worktree
        // in that session. Its project directory is not the worktree's.
        if harness == Harness::ClaudeCode {
            let held = read_tree(&root.join("projects"), "jsonl", since, |text| {
                (harness.recorded_turn(&cwd_text, text, since, brief.as_deref()).is_some()
                    && matches(text))
                .then_some(())
            })?;
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
    notes_match(cwd, harness, since, |text| posted_in(text, url))
}

fn posted_in(text: &str, url: &str) -> bool {
    use serde_json::Value;
    use std::collections::BTreeSet;
    fn visit(value: &Value, writes: &mut BTreeSet<String>, url: &str) -> bool {
        match value {
            Value::Array(items) => items.iter().any(|item| visit(item, writes, url)),
            Value::Object(fields) => {
                let kind = value["type"].as_str().unwrap_or("");
                let id = value["call_id"].as_str().or(value["id"].as_str());
                if matches!(kind, "function_call" | "tool_use" | "toolCall" | "function") {
                    let args = value
                        .get("arguments")
                        .or(value.get("input"))
                        .or_else(|| value["function"].get("arguments"));
                    let args = args.map(Value::to_string).unwrap_or_default();
                    if args.contains("gh ")
                        && (args.contains("issue comment")
                            || args.contains("/comments")
                                && (args.contains("POST") || args.contains("-f")))
                        && let Some(id) = id
                    {
                        writes.insert(id.to_string());
                    }
                }
                let reply = value["call_id"]
                    .as_str()
                    .or(value["tool_use_id"].as_str())
                    .or(value["tool_call_id"].as_str())
                    .or(value["toolCallId"].as_str());
                let result = matches!(kind, "function_call_output" | "tool_result")
                    || value["role"] == "toolResult"
                    || value["role"] == "tool";
                if result
                    && reply.is_some_and(|id| writes.contains(id))
                    && value.to_string().match_indices(url).any(|(at, _)| {
                        value.to_string()[at + url.len()..]
                            .chars()
                            .next()
                            .is_none_or(|next| !next.is_ascii_digit())
                    })
                {
                    return true;
                }
                fields.values().any(|child| visit(child, writes, url))
            }
            _ => false,
        }
    }
    let mut writes = BTreeSet::new();
    text.lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .any(|line| visit(&line, &mut writes, url))
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
/// user's home, the directories the fleet keeps, and a home noted beside the
/// worktree.
fn roots(cwd: &Path, harness: Harness) -> Result<Vec<PathBuf>> {
    let place = harness.session_place(&cwd_str(cwd));
    let mut roots = Vec::new();
    if let Some(home) = user_home() {
        roots.push(home.join(place.under_home));
    }
    roots.extend(kept_roots(harness)?);
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

/// Directories the fleet keeps for `harness`, including a role's own home.
/// Each is already the harness directory.
fn kept_roots(harness: Harness) -> Result<Vec<PathBuf>> {
    let Ok(cache) = crate::copies::cache() else {
        return Ok(Vec::new());
    };
    let base = cache.join("harness");
    let mut roots = Vec::new();
    for name in [harness.key().to_string(), format!("{}-host-user", harness.key())] {
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
    let place = harness.session_place(&cwd);
    let turns = match place.layout {
        SessionLayout::Files { dir, extension } => {
            read_files(&root.join(dir), extension, since, |text| {
                harness.recorded_turn(&cwd, text, not_before, brief.as_deref())
            })?
        }
        SessionLayout::Tree { dir, extension } => {
            read_tree(&root.join(dir), extension, since, |text| {
                harness.recorded_turn(&cwd, text, not_before, brief.as_deref())
            })?
        }
        SessionLayout::Named { dir, name, compressed } => {
            read_named(&root.join(dir), name, compressed, since, |text| {
                harness.recorded_turn(&cwd, text, not_before, brief.as_deref())
            })?
        }
        SessionLayout::Index { file } => read_index(root, &file, harness, &cwd, since, |text| {
            harness.recorded_turn(&cwd, text, not_before, brief.as_deref())
        })?,
    };
    Ok(prefer(&turns))
}

fn read_files<T>(
    dir: &Path,
    extension: &str,
    since: Option<u64>,
    mut turn_of: impl FnMut(&str) -> Option<T>,
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
        if let Some(text) = read_text(&path)?
            && let Some(turn) = turn_of(&text)
        {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn read_tree<T>(
    dir: &Path,
    extension: &str,
    since: Option<u64>,
    mut turn_of: impl FnMut(&str) -> Option<T>,
) -> Result<Vec<T>> {
    let mut files = Vec::new();
    collect(dir, extension, tree_floor(since), &mut files)?;
    let mut turns = Vec::new();
    for path in files {
        if let Some(text) = read_text(&path)?
            && let Some(turn) = turn_of(&text)
        {
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
    mut turn_of: impl FnMut(&str) -> Option<T>,
) -> Result<Vec<T>> {
    let Some(entries) = list_dir(dir)? else { return Ok(Vec::new()) };
    let mut turns = Vec::new();
    for session in entries {
        let path = session.join(name);
        if !path.is_file() || !fresh(&path, since)? {
            continue;
        }
        let text = if compressed { read_zstd(&path)? } else { read_text(&path)? };
        if let Some(text) = text
            && let Some(turn) = turn_of(&text)
        {
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
    mut turn_of: impl FnMut(&str) -> Option<T>,
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
        let Some(body) = read_text(&path)? else { continue };
        if brief.is_some_and(|brief| !harness.names_brief(&body, brief)) {
            continue;
        }
        if let Some(turn) = turn_of(&body) {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn write_harness(home: &Path, harness: Harness, cwd: &str, open: bool, at: u64) -> Result<()> {
    let place = harness.session_place(cwd);
    let root = home.join(place.under_home);
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
                json!({"type":"function_call", "call_id":"write", "name":"functions.exec", "arguments":"await tools.exec_command({cmd: 'gh issue comment 7 --body-file note.md'})"}),
                json!({"type":"function_call_output", "call_id":"write", "output":url}),
            ),
            (
                json!({"type":"tool_use", "id":"write", "name":"Bash", "input":{"command":"gh api repos/acme/tasks/issues/7/comments -f body=done"}}),
                json!({"type":"tool_result", "tool_use_id":"write", "content":url}),
            ),
            (
                json!({"type":"toolCall", "id":"write", "name":"bash", "arguments":{"command":"gh issue comment 7 --body done"}}),
                json!({"role":"toolResult", "toolCallId":"write", "content":[{"type":"text","text":url}]}),
            ),
            (
                json!({"type":"function", "id":"write", "function":{"name":"bash","arguments":"{\"command\":\"gh issue comment 7 --body done\"}"}}),
                json!({"role":"tool", "tool_call_id":"write", "content":url}),
            ),
        ];
        for (call, result) in calls {
            assert!(super::posted_in(&format!("{call}\n{result}\n"), url));
            assert!(!super::posted_in(
                &format!("{call}\n{}\n", result.to_string().replace(url, &format!("{url}0"))),
                url
            ));
            assert!(!super::posted_in(&result.to_string(), url), "a link alone proves no write");
            let read = call
                .to_string()
                .replace("issue comment", "issue view")
                .replace("/comments -f", "/comments --jq");
            assert!(
                !super::posted_in(&format!("{read}\n{result}\n"), url),
                "a read proves no write"
            );
        }
        let quoted = json!({"type":"user", "message":{"role":"user","content":format!("gh issue comment 7: {url}")}});
        assert!(!super::posted_in(&quoted.to_string(), url));
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

/// Read the usage of sessions started for one worker. Missing or unreadable
/// records are unknown; an unavailable session never becomes a zero count.
pub(crate) fn tokens(
    cwd: &Path,
    harness: Harness,
    start: u64,
    before: Option<u64>,
) -> Result<Option<afkconvoy_core::account::Tokens>> {
    use afkconvoy_core::account::{Tokens, parse_tokens};
    let cwd_text = cwd_str(cwd);
    let brief = brief(cwd);
    let mut homes = roots(cwd, harness)?;
    // Codex uses a new private home for each launch; the note names only the
    // latest. Earlier homes remain in the worktree until it is removed.
    if harness == Harness::Codex {
        if let Some(entries) = list_dir(&cwd.join(crate::worker_tmp::TMP_DIR))? {
            for entry in entries {
                for name in ["codex", "codex-host-user"] {
                    let home = entry.join("harness").join(name);
                    if home.is_dir() {
                        homes.push(home);
                    }
                }
            }
        }
    }
    homes.sort();
    homes.dedup();
    let mut total = Tokens::new();
    let mut found = false;
    for root in homes {
        let place = harness.session_place(&cwd_text);
        let accepts = |text: &str| {
            let at = session_first_at(text)?;
            if at < start.saturating_sub(60)
                || before.is_some_and(|end| at >= end.saturating_sub(2))
            {
                return None;
            }
            harness.recorded_turn(&cwd_text, text, None, brief.as_deref())?;
            Some(text.to_owned())
        };
        let mut sessions = match place.layout {
            SessionLayout::Files { dir, extension } => {
                read_files(&root.join(dir), extension, Some(start), accepts)?
            }
            SessionLayout::Tree { dir, extension } => {
                read_tree(&root.join(dir), extension, Some(start), accepts)?
            }
            SessionLayout::Index { file } => {
                read_index(&root, &file, harness, &cwd_text, Some(start), accepts)?
            }
            SessionLayout::Named { dir, name, compressed: _ } if harness == Harness::Grok => {
                let mut texts = Vec::new();
                if let Some(entries) = list_dir(&root.join(dir))? {
                    for entry in entries {
                        let Some(events) = read_text(&entry.join(&name))? else { continue };
                        if accepts(&events).is_some() {
                            if let Some(updates) = read_text(&entry.join("updates.jsonl"))? {
                                texts.push(updates);
                            }
                        }
                    }
                }
                texts
            }
            SessionLayout::Named { dir, name, compressed } => {
                read_named(&root.join(dir), name, compressed, Some(start), accepts)?
            }
        };
        if harness == Harness::ClaudeCode {
            sessions.extend(read_tree(&root.join("projects"), "jsonl", Some(start), accepts)?);
            sessions.sort();
            sessions.dedup();
        }
        for text in sessions {
            if let Some(tokens) = parse_tokens(harness, &text) {
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
