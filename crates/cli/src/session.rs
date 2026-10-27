//! A harness's session for one worktree: the turn it has open, or the one
//! it ended. The fleet reads that record. Orca's receipt and the terminal
//! screen are not a turn.
//!
//! Each harness writes its sessions in its own place. A worker's home is
//! the host user's, and the directory the fleet keeps for that harness:
//! Grok in particular keeps sessions in the user's home even when its
//! launch points somewhere else. Parsing stays here; the fold is
//! `afkconvoy_core::turn`.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use afkconvoy_core::harness::Harness;
use afkconvoy_core::turn::{Marker, Note, Turn, latest, prefer};
use anyhow::{Context, Result};
use serde_json::{Value, json};

/// The turn `harness` has recorded for `cwd`.
///
/// `since` drops session files last written before the worker started, so a
/// killed worker's open file does not keep the next one busy. `not_before`
/// is the start proof's bound: a turn that started earlier is not this
/// send. A missing session directory is no turn. A file that is there and
/// cannot be read is an error, so a tick does not treat it as idle.
pub fn read_turn(
    cwd: &Path,
    harness: Harness,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Option<Turn>> {
    let mut roots = Vec::new();
    if let Some(home) = user_home() {
        roots.push(user_root(&home, harness));
    }
    roots.extend(kept_roots(harness)?);
    if harness == Harness::Codex
        && let Some(home) = codex_noted(cwd)
    {
        roots.push(home);
    }
    roots.sort();
    roots.dedup();
    let mut turns = Vec::new();
    for root in &roots {
        if let Some(turn) = read_root(root, harness, cwd, since, not_before)? {
            turns.push(turn);
        }
    }
    Ok(prefer(&turns))
}

/// Writes one turn for every harness under `home`, closed unless `open`.
/// The stand-in calls this: the fleet no longer reads Orca's working flag,
/// and a test does not have to know which harness will be read.
pub fn record(home: &Path, cwd: &Path, open: bool, at: u64) -> Result<()> {
    let cwd = cwd_str(cwd);
    let iso = format_iso(at);
    write_claude(home, &cwd, open, at, &iso)?;
    write_pi(home, &cwd, open, at, &iso)?;
    write_codex(home, &cwd, open, at, &iso)?;
    write_dsh(home, &cwd, open, at)?;
    write_kimi(home, &cwd, open, at)?;
    write_grok(home, &cwd, open, at, &iso)?;
    Ok(())
}

fn user_home() -> Option<PathBuf> {
    std::env::var_os("HOME").filter(|home| !home.is_empty()).map(PathBuf::from)
}

/// The harness's own directory under a user's home.
fn user_root(home: &Path, harness: Harness) -> PathBuf {
    match harness {
        Harness::ClaudeCode => home.join(".claude"),
        Harness::Pi => home.join(".pi/agent"),
        Harness::Codex => home.join(".codex"),
        Harness::Dsh => home.join(".dsh"),
        Harness::Kimi => home.join(".kimi-code"),
        Harness::Grok => home.join(".grok"),
    }
}

/// Directories the fleet keeps for `harness`, including a role's own home.
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

/// Codex's per-worktree home, as the launch wrote it. Absent for every
/// other harness, and when this directory is not a worktree.
fn codex_noted(cwd: &Path) -> Option<PathBuf> {
    let git = std::fs::read_to_string(cwd.join(".git")).ok()?;
    let dir = git.trim().strip_prefix("gitdir:")?.trim();
    let note = Path::new(dir).join(crate::codex_launch::HOME_NOTE);
    let home = std::fs::read_to_string(note).ok()?;
    let home = home.trim();
    if home.is_empty() { None } else { Some(PathBuf::from(home)) }
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
    let cwd = cwd_str(cwd);
    let turns = match harness {
        Harness::ClaudeCode => claude_turns(root, &cwd, since, not_before)?,
        Harness::Pi => pi_turns(root, &cwd, since, not_before)?,
        Harness::Codex => codex_turns(root, &cwd, since, not_before)?,
        Harness::Dsh => dsh_turns(root, &cwd, since, not_before)?,
        Harness::Kimi => kimi_turns(root, &cwd, since, not_before)?,
        Harness::Grok => grok_turns(root, &cwd, since, not_before)?,
    };
    Ok(prefer(&turns))
}

fn claude_turns(
    root: &Path,
    cwd: &str,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Vec<Turn>> {
    let dir = root.join("projects").join(claude_dir(cwd));
    jsonl_turns(&dir, since, not_before, |value| {
        if value["cwd"].as_str().is_some_and(|recorded| !same_cwd(recorded, cwd)) {
            return None;
        }
        claude_note(value)
    })
}

fn pi_turns(
    root: &Path,
    cwd: &str,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Vec<Turn>> {
    let dir = root.join("sessions").join(wrapped_dir(cwd));
    let Some(entries) = list_dir(&dir)? else { return Ok(Vec::new()) };
    let mut turns = Vec::new();
    for path in entries {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        if !fresh(&path, since)? {
            continue;
        }
        if let Some(turn) = fold_pi(&path, cwd, not_before)? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn fold_pi(path: &Path, cwd: &str, not_before: Option<u64>) -> Result<Option<Turn>> {
    let Some(text) = read_text(path)? else { return Ok(None) };
    let mut notes = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        if value["type"] == "session"
            && value["cwd"].as_str().is_some_and(|recorded| !same_cwd(recorded, cwd))
        {
            return Ok(None);
        }
        if let Some(note) = pi_note(&value) {
            notes.push(note);
        }
    }
    Ok(latest(&notes, not_before))
}

fn codex_turns(
    root: &Path,
    cwd: &str,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Vec<Turn>> {
    let mut files = Vec::new();
    collect_jsonl(&root.join("sessions"), mtime_floor(since), &mut files)?;
    let mut turns = Vec::new();
    for path in files {
        if let Some(turn) = fold_codex(&path, cwd, not_before)? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn fold_codex(path: &Path, cwd: &str, not_before: Option<u64>) -> Result<Option<Turn>> {
    let Some(text) = read_text(path)? else { return Ok(None) };
    let mut saw_cwd = false;
    let mut notes = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        if value["type"] == "session_meta" {
            let recorded = value["payload"]["cwd"].as_str().unwrap_or("");
            if !same_cwd(recorded, cwd) {
                return Ok(None);
            }
            saw_cwd = true;
        }
        if let Some(note) = codex_note(&value) {
            notes.push(note);
        }
    }
    if !saw_cwd {
        return Ok(None);
    }
    Ok(latest(&notes, not_before))
}

fn dsh_turns(
    root: &Path,
    cwd: &str,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Vec<Turn>> {
    let dir = root.join("sessions").join(wrapped_dir(cwd));
    let Some(sessions) = list_dir(&dir)? else { return Ok(Vec::new()) };
    let mut turns = Vec::new();
    for session in sessions {
        let path = session.join("session.v4.jsonl.zstd");
        if !path.is_file() || !fresh(&path, since)? {
            continue;
        }
        if let Some(turn) = fold_zstd(&path, not_before, dsh_note)? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn kimi_turns(
    root: &Path,
    cwd: &str,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Vec<Turn>> {
    let Some(text) = read_text(&root.join("session_index.jsonl"))? else {
        return Ok(Vec::new());
    };
    let mut turns = Vec::new();
    for line in text.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else { continue };
        if value["workDir"].as_str().is_none_or(|dir| !same_cwd(dir, cwd)) {
            continue;
        }
        let Some(session) = value["sessionDir"].as_str() else { continue };
        let path = root.join(session).join("agents/main/wire.jsonl");
        let path = if Path::new(session).is_absolute() {
            PathBuf::from(session).join("agents/main/wire.jsonl")
        } else {
            path
        };
        if !fresh(&path, since)? {
            continue;
        }
        if let Some(turn) = fold_jsonl(&path, not_before, &kimi_note)? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn grok_turns(
    root: &Path,
    cwd: &str,
    since: Option<u64>,
    not_before: Option<u64>,
) -> Result<Vec<Turn>> {
    let dir = root.join("sessions").join(grok_dir(cwd));
    let Some(sessions) = list_dir(&dir)? else { return Ok(Vec::new()) };
    let mut turns = Vec::new();
    for session in sessions {
        let path = session.join("events.jsonl");
        if !path.is_file() || !fresh(&path, since)? {
            continue;
        }
        if let Some(turn) = fold_jsonl(&path, not_before, &grok_note)? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn jsonl_turns(
    dir: &Path,
    since: Option<u64>,
    not_before: Option<u64>,
    note: impl Fn(&Value) -> Option<Note>,
) -> Result<Vec<Turn>> {
    let Some(entries) = list_dir(dir)? else { return Ok(Vec::new()) };
    let mut turns = Vec::new();
    for path in entries {
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        if !fresh(&path, since)? {
            continue;
        }
        if let Some(turn) = fold_jsonl(&path, not_before, &note)? {
            turns.push(turn);
        }
    }
    Ok(turns)
}

fn fold_jsonl(
    path: &Path,
    not_before: Option<u64>,
    note: &impl Fn(&Value) -> Option<Note>,
) -> Result<Option<Turn>> {
    let Some(text) = read_text(path)? else { return Ok(None) };
    Ok(fold_lines(&text, not_before, note))
}

fn fold_zstd(
    path: &Path,
    not_before: Option<u64>,
    note: impl Fn(&Value) -> Option<Note>,
) -> Result<Option<Turn>> {
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| format!("cannot read {}", path.display()));
        }
    };
    let decoder = zstd::stream::read::Decoder::new(file)
        .with_context(|| format!("cannot read {}", path.display()))?;
    let mut notes = Vec::new();
    for line in BufReader::new(decoder).lines() {
        let line = line.with_context(|| format!("cannot read {}", path.display()))?;
        if let Some(parsed) = parse_note(&line, &note) {
            notes.push(parsed);
        }
    }
    Ok(latest(&notes, not_before))
}

fn fold_lines(
    text: &str,
    not_before: Option<u64>,
    note: &impl Fn(&Value) -> Option<Note>,
) -> Option<Turn> {
    let mut notes = Vec::new();
    for line in text.lines() {
        if let Some(parsed) = parse_note(line, note) {
            notes.push(parsed);
        }
    }
    latest(&notes, not_before)
}

fn parse_note(line: &str, note: &impl Fn(&Value) -> Option<Note>) -> Option<Note> {
    if line.trim().is_empty() {
        return None;
    }
    note(&serde_json::from_str(line).ok()?)
}

fn claude_note(value: &Value) -> Option<Note> {
    let child = value["isSidechain"] == true;
    let at = parse_iso(value["timestamp"].as_str()?)?;
    match value["type"].as_str()? {
        "user" => Some(Note { marker: Marker::Start, at, child }),
        "assistant" if value["message"]["stop_reason"] == "end_turn" => {
            Some(Note { marker: Marker::End, at, child })
        }
        _ => None,
    }
}

fn pi_note(value: &Value) -> Option<Note> {
    if value["type"] != "message" {
        return None;
    }
    let at = parse_iso(value["timestamp"].as_str()?)?;
    match value["message"]["role"].as_str()? {
        "user" => Some(Note { marker: Marker::Start, at, child: false }),
        "assistant" => match value["message"]["stopReason"].as_str()? {
            "stop" | "error" => Some(Note { marker: Marker::End, at, child: false }),
            _ => None,
        },
        _ => None,
    }
}

fn codex_note(value: &Value) -> Option<Note> {
    let payload = &value["payload"];
    match payload["type"].as_str()? {
        "task_started" => {
            let at = number_at(payload["started_at"].as_u64(), value["timestamp"].as_str())?;
            Some(Note { marker: Marker::Start, at, child: false })
        }
        "task_complete" => {
            let at = number_at(payload["completed_at"].as_u64(), value["timestamp"].as_str())?;
            Some(Note { marker: Marker::End, at, child: false })
        }
        "turn_aborted" => {
            let at = parse_iso(value["timestamp"].as_str()?)?;
            Some(Note { marker: Marker::End, at, child: false })
        }
        _ => None,
    }
}

fn dsh_note(value: &Value) -> Option<Note> {
    let at = epoch_number(value["time"].as_u64()?);
    match value["type"].as_str()? {
        "turn/start" => Some(Note { marker: Marker::Start, at, child: false }),
        "turn/end" => Some(Note { marker: Marker::End, at, child: false }),
        _ => None,
    }
}

fn kimi_note(value: &Value) -> Option<Note> {
    // The wire's other lines are huge and are not a turn. The type is the
    // whole match, so a prompt that mentions a turn is not one.
    let at = epoch_number(value["time"].as_u64()?);
    match value["type"].as_str()? {
        "agent.turn.started" => Some(Note { marker: Marker::Start, at, child: false }),
        "agent.turn.ended" | "turn.ended" => Some(Note { marker: Marker::End, at, child: false }),
        _ => None,
    }
}

fn grok_note(value: &Value) -> Option<Note> {
    let at = parse_iso(value["ts"].as_str()?)?;
    let child = value["session_relationship"].as_str().is_some_and(|kind| kind != "primary");
    match value["type"].as_str()? {
        "turn_started" => Some(Note { marker: Marker::Start, at, child }),
        "turn_ended" => Some(Note { marker: Marker::End, at, child }),
        _ => None,
    }
}

fn number_at(seconds: Option<u64>, timestamp: Option<&str>) -> Option<u64> {
    seconds.map(epoch_number).or_else(|| timestamp.and_then(parse_iso))
}

/// Epoch seconds. A value past year 2286 is milliseconds.
fn epoch_number(value: u64) -> u64 {
    if value > 10_000_000_000 { value / 1000 } else { value }
}

fn same_cwd(recorded: &str, cwd: &str) -> bool {
    recorded.trim_end_matches('/') == cwd.trim_end_matches('/')
}

fn cwd_str(cwd: &Path) -> String {
    cwd.to_string_lossy().into_owned()
}

/// `/Users/sunfmin/Developments/chessmirror` → `-Users-sunfmin-Developments-chessmirror`.
fn claude_dir(cwd: &str) -> String {
    stored_name(cwd.replace('/', "-"), cwd)
}

/// The same path → `--Users-sunfmin-Developments-chessmirror--`.
fn wrapped_dir(cwd: &str) -> String {
    let mut with_slash = cwd.to_string();
    if !with_slash.ends_with('/') {
        with_slash.push('/');
    }
    let mut body = String::new();
    for ch in with_slash.chars() {
        if ch.is_ascii_alphanumeric() {
            body.push(ch);
        } else {
            body.push('-');
        }
    }
    stored_name(format!("-{body}-"), cwd)
}

/// One path component holds at most this many bytes. A worktree path longer
/// than that cannot be the harness's own directory name, so the record and
/// the reader share a short name for it. A path that fits keeps the name the
/// harness writes.
fn stored_name(natural: String, cwd: &str) -> String {
    if natural.len() <= 255 { natural } else { format!("afkconvoy-{}", mark(cwd)) }
}

/// The same path → `%2FUsers%2Fsunfmin%2FDevelopments%2Fchessmirror`.
fn grok_dir(cwd: &str) -> String {
    let mut out = String::new();
    for byte in cwd.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    stored_name(out, cwd)
}

fn mark(cwd: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    cwd.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn mtime_floor(since: Option<u64>) -> u64 {
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

fn collect_jsonl(dir: &Path, floor: u64, out: &mut Vec<PathBuf>) -> Result<()> {
    let Some(entries) = list_dir(dir)? else { return Ok(()) };
    for entry in entries {
        if entry.is_dir() {
            collect_jsonl(&entry, floor, out)?;
            continue;
        }
        if entry.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
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

fn replace_dir(path: &Path) -> Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error).with_context(|| format!("cannot replace {}", path.display()));
        }
    }
    std::fs::create_dir_all(path).with_context(|| format!("cannot replace {}", path.display()))
}

fn write_claude(home: &Path, cwd: &str, open: bool, at: u64, iso: &str) -> Result<()> {
    let dir = user_root(home, Harness::ClaudeCode).join("projects").join(claude_dir(cwd));
    replace_dir(&dir)?;
    let user = json!({"type":"user","timestamp":iso,"cwd":cwd,"isSidechain":false,
        "message":{"role":"user","content":"brief"}});
    let mut text = format!("{user}\n");
    if !open {
        let end = json!({"type":"assistant","timestamp":iso,"cwd":cwd,"isSidechain":false,
            "message":{"role":"assistant","stop_reason":"end_turn"}});
        text.push_str(&end.to_string());
        text.push('\n');
    }
    write_text(&dir.join("afkconvoy-turn.jsonl"), &text, at)
}

fn write_pi(home: &Path, cwd: &str, open: bool, at: u64, iso: &str) -> Result<()> {
    let dir = user_root(home, Harness::Pi).join("sessions").join(wrapped_dir(cwd));
    replace_dir(&dir)?;
    let mut text = format!("{}\n", json!({"type":"session","cwd":cwd,"timestamp":iso}));
    text.push_str(&format!(
        "{}\n",
        json!({"type":"message","timestamp":iso,"message":{"role":"user"}})
    ));
    if !open {
        text.push_str(&format!(
            "{}\n",
            json!({"type":"message","timestamp":iso,"message":{"role":"assistant","stopReason":"stop"}})
        ));
    }
    write_text(&dir.join("afkconvoy-turn.jsonl"), &text, at)
}

fn write_codex(home: &Path, cwd: &str, open: bool, at: u64, iso: &str) -> Result<()> {
    let root = user_root(home, Harness::Codex).join("sessions");
    let name = format!("rollout-afkconvoy-{}.jsonl", mark(cwd));
    remove_named(&root, &name)?;
    let (year, month, day) = civil_from_days((at / 86_400) as i64);
    let path = root
        .join(format!("{year:04}"))
        .join(format!("{month:02}"))
        .join(format!("{day:02}"))
        .join(name);
    let mut text = format!(
        "{}\n",
        json!({"timestamp":iso,"type":"session_meta","payload":{"cwd":cwd,"id":"afkconvoy"}})
    );
    text.push_str(&format!(
        "{}\n",
        json!({"timestamp":iso,"type":"event_msg","payload":{"type":"task_started","started_at":at,"turn_id":"afkconvoy"}})
    ));
    if !open {
        text.push_str(&format!(
            "{}\n",
            json!({"timestamp":iso,"type":"event_msg","payload":{"type":"task_complete","completed_at":at,"turn_id":"afkconvoy"}})
        ));
    }
    write_text(&path, &text, at)
}

fn write_dsh(home: &Path, cwd: &str, open: bool, at: u64) -> Result<()> {
    let dir = user_root(home, Harness::Dsh)
        .join("sessions")
        .join(wrapped_dir(cwd))
        .join("session-afkconvoy");
    replace_dir(&dir)?;
    let ms = at.saturating_mul(1000);
    let mut text = format!("{}\n", json!({"type":"turn/start","time":ms,"data":{"turn":1}}));
    if !open {
        text.push_str(&format!(
            "{}\n",
            json!({"type":"turn/end","time":ms,"data":{"turn":1,"reason":{"kind":"completed"}}})
        ));
    }
    write_zstd(&dir.join("session.v4.jsonl.zstd"), &text, at)
}

fn write_kimi(home: &Path, cwd: &str, open: bool, at: u64) -> Result<()> {
    let root = user_root(home, Harness::Kimi);
    let session = root.join("sessions").join(format!("afkconvoy-{}", mark(cwd)));
    replace_dir(&session.join("agents/main"))?;
    let ms = at.saturating_mul(1000);
    let mut text =
        format!("{}\n", json!({"type":"agent.turn.started","time":ms,"turnId":0,"kind":"event"}));
    if !open {
        text.push_str(&format!(
            "{}\n",
            json!({"type":"agent.turn.ended","time":ms,"turnId":0,"outcome":"completed"})
        ));
    }
    write_text(&session.join("agents/main/wire.jsonl"), &text, at)?;
    let index = root.join("session_index.jsonl");
    let mut kept = String::new();
    if let Some(existing) = read_text(&index)? {
        for line in existing.lines() {
            let Ok(value) = serde_json::from_str::<Value>(line) else {
                kept.push_str(line);
                kept.push('\n');
                continue;
            };
            if value["workDir"].as_str().is_some_and(|dir| same_cwd(dir, cwd)) {
                continue;
            }
            kept.push_str(line);
            kept.push('\n');
        }
    }
    kept.push_str(
        &json!({"sessionId": mark(cwd), "sessionDir": session, "workDir": cwd}).to_string(),
    );
    kept.push('\n');
    write_text(&index, &kept, at)
}

fn write_grok(home: &Path, cwd: &str, open: bool, at: u64, iso: &str) -> Result<()> {
    let dir =
        user_root(home, Harness::Grok).join("sessions").join(grok_dir(cwd)).join("afkconvoy-turn");
    replace_dir(&dir)?;
    let mut text = format!(
        "{}\n",
        json!({"ts":iso,"type":"turn_started","turn_number":1,"session_relationship":"primary"})
    );
    if !open {
        text.push_str(&format!(
            "{}\n",
            json!({"ts":iso,"type":"turn_ended","outcome":"completed"})
        ));
    }
    write_text(&dir.join("events.jsonl"), &text, at)?;
    write_text(
        &dir.join("summary.json"),
        &json!({"info":{"cwd":cwd,"created_at":iso}}).to_string(),
        at,
    )
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

fn parse_iso(text: &str) -> Option<u64> {
    let bytes = text.as_bytes();
    if bytes.len() < 20 || bytes[4] != b'-' || bytes[7] != b'-' || bytes[10] != b'T' {
        return None;
    }
    if bytes[13] != b':' || bytes[16] != b':' {
        return None;
    }
    let year: i32 = text[0..4].parse().ok()?;
    let month: u32 = text[5..7].parse().ok()?;
    let day: u32 = text[8..10].parse().ok()?;
    let hour: u32 = text[11..13].parse().ok()?;
    let minute: u32 = text[14..16].parse().ok()?;
    let second: u32 = text[17..19].parse().ok()?;
    let mut rest = &text[19..];
    if let Some(stripped) = rest.strip_prefix('.') {
        let digits = stripped.find(|ch: char| !ch.is_ascii_digit()).unwrap_or(stripped.len());
        rest = &stripped[digits..];
    }
    if rest != "Z" && rest != "+00:00" && rest != "-00:00" {
        return None;
    }
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || hour > 23
        || minute > 59
        || second > 60
    {
        return None;
    }
    let days = days_from_civil(year, month, day)?;
    Some(days * 86_400 + u64::from(hour) * 3600 + u64::from(minute) * 60 + u64::from(second))
}

fn format_iso(at: u64) -> String {
    let (year, month, day) = civil_from_days((at / 86_400) as i64);
    let rest = at % 86_400;
    let hour = rest / 3600;
    let minute = (rest % 3600) / 60;
    let second = rest % 60;
    format!("{year:04}-{month:02}-{day:02}T{hour:02}:{minute:02}:{second:02}Z")
}

fn days_from_civil(mut year: i32, month: u32, day: u32) -> Option<u64> {
    if !(1..=12).contains(&month) || day < 1 {
        return None;
    }
    year -= i32::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let yoe = (year - era * 400) as u64;
    let month_index = if month > 2 { month - 3 } else { month + 9 };
    let day_of_year = (153 * u64::from(month_index) + 2) / 5 + u64::from(day) - 1;
    let day_of_era = yoe * 365 + yoe / 4 - yoe / 100 + day_of_year;
    let shifted = i64::from(era) * 146_097 + day_of_era as i64 - 719_468;
    u64::try_from(shifted).ok()
}

fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let shifted = days + 719_468;
    let era = if shifted >= 0 { shifted } else { shifted - 146_096 } / 146_097;
    let day_of_era = (shifted - era * 146_097) as u64;
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
    let mut year = year_of_era as i64 + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_index + 2) / 5 + 1;
    let month = if month_index < 10 { month_index + 3 } else { month_index - 9 };
    if month <= 2 {
        year += 1;
    }
    (year as i32, month as u32, day as u32)
}

#[cfg(test)]
mod tests {
    use super::*;

    const AT: u64 = 1_793_106_373;

    #[test]
    fn an_iso_stamp_is_epoch_seconds() {
        assert_eq!(parse_iso("2026-10-27T13:06:13Z"), Some(AT));
        assert_eq!(parse_iso("2026-10-27T13:06:13.022Z"), Some(AT));
        assert_eq!(format_iso(AT), "2026-10-27T13:06:13Z");
        assert_eq!(parse_iso(&format_iso(AT)), Some(AT));
    }

    #[test]
    fn session_directories_match_each_harness() {
        let cwd = "/Users/sunfmin/Developments/chessmirror";
        assert_eq!(claude_dir(cwd), "-Users-sunfmin-Developments-chessmirror");
        assert_eq!(wrapped_dir(cwd), "--Users-sunfmin-Developments-chessmirror--");
        assert_eq!(grok_dir(cwd), "%2FUsers%2Fsunfmin%2FDevelopments%2Fchessmirror");
    }

    #[test]
    fn a_worktree_path_past_one_component_keeps_a_short_name() {
        let dir = tempfile::tempdir().unwrap();
        let mut cwd = dir.path().to_path_buf();
        for i in 0..40 {
            cwd.push(format!("segment-{i:02}"));
        }
        std::fs::create_dir_all(&cwd).unwrap();
        let text = cwd_str(&cwd);
        assert!(text.len() > 255);
        for name in [claude_dir(&text), wrapped_dir(&text), grok_dir(&text)] {
            assert!(name.len() <= 255, "{name}");
            assert!(name.starts_with("afkconvoy-"), "{name}");
        }
        let home = dir.path().join("home");
        record(&home, &cwd, false, AT).unwrap();
        let turn = read_root(&home.join(".claude"), Harness::ClaudeCode, &cwd, None, None).unwrap();
        assert_eq!(turn, Some(Turn::Closed { since: AT }));
    }

    #[test]
    fn every_harness_round_trips_an_open_and_a_closed_turn() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("home");
        let cwd = dir.path().join("work");
        std::fs::create_dir(&cwd).unwrap();
        record(&home, &cwd, false, AT).unwrap();
        for harness in Harness::ALL {
            let turn =
                read_root(&user_root(&home, harness), harness, &cwd, None, Some(AT)).unwrap();
            assert!(
                matches!(turn, Some(Turn::Closed { since: AT })),
                "{} closed: {turn:?}",
                harness.key()
            );
        }
        record(&home, &cwd, true, AT).unwrap();
        for harness in Harness::ALL {
            let turn =
                read_root(&user_root(&home, harness), harness, &cwd, None, Some(AT - 2)).unwrap();
            assert!(
                matches!(turn, Some(Turn::Open { since: AT })),
                "{} open: {turn:?}",
                harness.key()
            );
            let earlier =
                read_root(&user_root(&home, harness), harness, &cwd, None, Some(AT + 1)).unwrap();
            assert_eq!(earlier, None, "{} must not prove a start before the bound", harness.key());
        }
    }

    #[test]
    fn a_sidechain_does_not_end_the_lead_turn() {
        let note = claude_note(&json!({"type":"assistant","timestamp":"2026-10-27T13:06:13Z",
            "isSidechain":true,"message":{"stop_reason":"end_turn"}}));
        assert_eq!(note.map(|note| note.child), Some(true));
        let grok = grok_note(&json!({"type":"turn_ended","ts":"2026-10-27T13:06:13Z",
            "session_relationship":"subagent"}));
        assert_eq!(grok.map(|note| note.child), Some(true));
        assert!(
            grok_note(&json!({"type":"turn_ended","ts":"2026-10-27T13:06:13Z"}))
                .is_some_and(|n| !n.child)
        );
    }

    #[test]
    fn a_missing_session_directory_is_no_turn() {
        let dir = tempfile::tempdir().unwrap();
        let turn = read_root(dir.path(), Harness::Grok, Path::new("/nowhere"), None, None).unwrap();
        assert_eq!(turn, None);
    }
}
