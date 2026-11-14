//! Session files and their directories as the host runner keeps them
//! between looks (sunfmin/control#993).
//!
//! A resident session's file is tens of megabytes and written every few
//! seconds, and several steps of a tick ask about it. One cache per process
//! keeps each file's text with the bytes it was read from: a file that grew
//! is read from where the last read ended, and is read whole again only when
//! it shrank, is another file (its inode changed), or no longer begins and
//! ends where the kept text did. A directory's listing is kept until the
//! directory's own stamp changes, so a tick lists a tree once at most.

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};

/// A session file's text, shared with the cache that keeps it.
pub(super) type Text = Arc<String>;

/// The kept texts' size past which the least recently asked are forgotten.
const KEPT_BYTES: usize = 128 << 20;

/// How many bytes of a kept file's start, and of the end of what was read,
/// are compared before its growth is read: a file rewritten in place is
/// read whole again.
const PRINT: usize = 64;

/// What a `stat` says of a file: which file it is and when it last changed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Seen {
    file: (u64, u64),
    len: u64,
    modified: u128,
    changed: u128,
}

impl Seen {
    fn of(meta: &std::fs::Metadata) -> Self {
        let modified = meta
            .modified()
            .ok()
            .and_then(|at| at.duration_since(UNIX_EPOCH).ok())
            .map_or(0, |at| at.as_nanos());
        Seen { file: identity(meta), len: meta.len(), modified, changed: changed(meta) }
    }
}

#[cfg(unix)]
fn identity(meta: &std::fs::Metadata) -> (u64, u64) {
    use std::os::unix::fs::MetadataExt;
    (meta.dev(), meta.ino())
}

#[cfg(not(unix))]
fn identity(_: &std::fs::Metadata) -> (u64, u64) {
    (0, 0)
}

/// The inode's change time: a write, and also a change of who may read it.
#[cfg(unix)]
fn changed(meta: &std::fs::Metadata) -> u128 {
    use std::os::unix::fs::MetadataExt;
    (meta.ctime().max(0) as u128) * 1_000_000_000 + meta.ctime_nsec().max(0) as u128
}

#[cfg(not(unix))]
fn changed(_: &std::fs::Metadata) -> u128 {
    0
}

/// One file's text as far as it was read.
struct Kept {
    seen: Seen,
    /// The bytes read, from the file's start.
    read: u64,
    /// The first bytes read, and the last.
    head: Vec<u8>,
    tail: Vec<u8>,
    text: Text,
    /// The start of a character the last read ended inside.
    partial: Vec<u8>,
    used: u64,
    /// What was found to hold of the text, which holds while it only grows.
    known: std::collections::HashSet<String>,
}

impl Kept {
    fn view(&self) -> Text {
        if self.partial.is_empty() {
            Arc::clone(&self.text)
        } else {
            Arc::new(format!("{}\u{FFFD}", self.text))
        }
    }

    /// Takes in `bytes`, the file's next ones after what was read.
    fn take(&mut self, bytes: &[u8]) {
        if self.head.len() < PRINT {
            let more = (PRINT - self.head.len()).min(bytes.len());
            self.head.extend_from_slice(&bytes[..more]);
        }
        self.tail.extend_from_slice(bytes);
        let over = self.tail.len().saturating_sub(PRINT);
        self.tail.drain(..over);
        self.read += bytes.len() as u64;
        decode(Arc::make_mut(&mut self.text), &mut self.partial, bytes);
    }

    /// Reads what `file` holds past what was read. False: the file no
    /// longer begins or ends where the kept text did, so it was rewritten.
    fn grow(&mut self, file: &mut std::fs::File) -> std::io::Result<bool> {
        let mut head = vec![0; self.head.len()];
        file.read_exact(&mut head)?;
        let mut tail = vec![0; self.tail.len()];
        file.seek(SeekFrom::Start(self.read - tail.len() as u64))?;
        file.read_exact(&mut tail)?;
        if head != self.head || tail != self.tail {
            return Ok(false);
        }
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        #[cfg(test)]
        BYTES.with(|read| read.set(read.get() + bytes.len()));
        self.take(&bytes);
        Ok(true)
    }
}

/// Appends `bytes` to `text` as `String::from_utf8_lossy` reads them, with
/// `partial` the start of a character a previous call ended inside.
fn decode(text: &mut String, partial: &mut Vec<u8>, bytes: &[u8]) {
    let mut chunk = std::mem::take(partial);
    chunk.extend_from_slice(bytes);
    let mut rest = &chunk[..];
    loop {
        match std::str::from_utf8(rest) {
            Ok(valid) => {
                text.push_str(valid);
                return;
            }
            Err(error) => {
                let (valid, after) = rest.split_at(error.valid_up_to());
                text.push_str(std::str::from_utf8(valid).unwrap_or_default());
                match error.error_len() {
                    Some(bad) => {
                        text.push('\u{FFFD}');
                        rest = &after[bad..];
                    }
                    None => {
                        *partial = after.to_vec();
                        return;
                    }
                }
            }
        }
    }
}

#[derive(Default)]
struct Cache {
    files: HashMap<PathBuf, Kept>,
    dirs: HashMap<PathBuf, (Seen, Listing)>,
    asked: u64,
}

impl Cache {
    fn forget_oldest(&mut self) {
        let mut size: usize = self.files.values().map(|kept| kept.text.len()).sum();
        while size > KEPT_BYTES {
            let Some(oldest) =
                self.files.iter().min_by_key(|(_, kept)| kept.used).map(|(path, _)| path.clone())
            else {
                return;
            };
            if let Some(kept) = self.files.remove(&oldest) {
                size -= kept.text.len();
            }
        }
    }
}

static CACHE: Mutex<Option<Cache>> = Mutex::new(None);

fn cache() -> std::sync::MutexGuard<'static, Option<Cache>> {
    CACHE.lock().unwrap_or_else(|e| e.into_inner())
}

/// The text of the session file at `path`, read lossily as UTF-8. A
/// missing file is `None`; one that is there and cannot be read is an error.
pub(super) fn text(path: &Path) -> Result<Option<Text>> {
    let mut guard = cache();
    let cache = guard.get_or_insert_with(Default::default);
    cache.asked += 1;
    let asked = cache.asked;
    let meta = match path.metadata() {
        Ok(meta) => meta,
        Err(error) => {
            cache.files.remove(path);
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error).with_context(|| format!("cannot read {}", path.display()));
        }
    };
    let seen = Seen::of(&meta);
    if let Some(kept) = cache.files.get_mut(path) {
        kept.used = asked;
        if kept.seen == seen {
            return Ok(Some(kept.view()));
        }
        if kept.seen.file == seen.file && seen.len >= kept.read {
            let grown = std::fs::File::open(path).and_then(|mut file| kept.grow(&mut file));
            match grown {
                Ok(true) => {
                    kept.seen = seen;
                    let text = kept.view();
                    cache.forget_oldest();
                    return Ok(Some(text));
                }
                Ok(false) => {}
                Err(error) => {
                    cache.files.remove(path);
                    if error.kind() == std::io::ErrorKind::NotFound {
                        return Ok(None);
                    }
                    return Err(error).with_context(|| format!("cannot read {}", path.display()));
                }
            }
        }
        cache.files.remove(path);
    }
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", path.display())),
    };
    #[cfg(test)]
    BYTES.with(|read| read.set(read.get() + bytes.len()));
    let mut kept = Kept {
        seen,
        read: 0,
        head: Vec::new(),
        tail: Vec::new(),
        text: Arc::new(String::with_capacity(bytes.len())),
        partial: Vec::new(),
        used: asked,
        known: Default::default(),
    };
    kept.take(&bytes);
    let text = kept.view();
    cache.files.insert(path.to_path_buf(), kept);
    cache.forget_oldest();
    Ok(Some(text))
}

/// Whether `fact` was found to hold of the kept text of `path`
/// (`know`). A file read whole again knows nothing yet.
pub(super) fn known(path: &Path, fact: &str) -> bool {
    cache()
        .as_ref()
        .and_then(|cache| cache.files.get(path))
        .is_some_and(|kept| kept.known.contains(fact))
}

/// Notes that `fact` holds of `text`, the kept text of `path`, and so of
/// every text it grows into: a session's record of whose it is, once
/// written, stays in it. A text the cache no longer keeps notes nothing.
pub(super) fn know(path: &Path, text: &Text, fact: &str) {
    if let Some(kept) = cache().as_mut().and_then(|cache| cache.files.get_mut(path))
        && Arc::ptr_eq(&kept.text, text)
    {
        kept.known.insert(fact.to_owned());
    }
}

/// A directory's entries, each with whether it is a directory.
pub(super) type Listing = Arc<Vec<(PathBuf, bool)>>;

/// The entries of the directory at `dir`, listed again only once the
/// directory's stamp changed: an entry added, removed or renamed changes it.
/// A listing taken within two seconds of its directory's last change is not
/// kept, for a file system whose stamp is coarser than the change. A missing
/// directory is `None`.
pub(super) fn listing(dir: &Path) -> Result<Option<Listing>> {
    let meta = match dir.metadata() {
        Ok(meta) => Some(meta),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", dir.display())),
    };
    let seen = meta.as_ref().map(Seen::of);
    if let Some(seen) = seen {
        let guard = cache();
        if let Some((kept, listing)) = guard.as_ref().and_then(|cache| cache.dirs.get(dir))
            && *kept == seen
        {
            return Ok(Some(Arc::clone(listing)));
        }
    }
    let read = match std::fs::read_dir(dir) {
        Ok(read) => read,
        Err(error) => {
            if let Some(cache) = cache().as_mut() {
                cache.dirs.remove(dir);
            }
            if error.kind() == std::io::ErrorKind::NotFound {
                return Ok(None);
            }
            return Err(error).with_context(|| format!("cannot read {}", dir.display()));
        }
    };
    #[cfg(test)]
    LISTS.with(|lists| lists.set(lists.get() + 1));
    let mut entries = Vec::new();
    for entry in read {
        let entry = entry.with_context(|| format!("cannot read {}", dir.display()))?;
        let path = entry.path();
        let is_dir = match entry.file_type() {
            Ok(kind) if kind.is_symlink() => path.is_dir(),
            Ok(kind) => kind.is_dir(),
            Err(_) => path.is_dir(),
        };
        entries.push((path, is_dir));
    }
    let listing = Arc::new(entries);
    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |at| at.as_nanos());
    let settled = |at: u128| at + 2_000_000_000 <= now;
    if let Some(seen) = seen.filter(|seen| settled(seen.modified) && settled(seen.changed)) {
        let mut guard = cache();
        guard
            .get_or_insert_with(Default::default)
            .dirs
            .insert(dir.to_path_buf(), (seen, Arc::clone(&listing)));
    }
    Ok(Some(listing))
}

#[cfg(test)]
thread_local! {
    /// How many bytes of session files this thread read.
    static BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// How many directories this thread listed.
    static LISTS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn bytes() -> usize {
        BYTES.with(std::cell::Cell::get)
    }

    fn append(path: &Path, more: &[u8]) {
        std::fs::OpenOptions::new().append(true).open(path).unwrap().write_all(more).unwrap();
    }

    /// Asking about a 30 MB session that grew by a few KB reads a few KB.
    #[test]
    fn a_grown_session_file_is_read_from_where_the_last_read_ended() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let line = format!("{{\"type\":\"assistant\",\"text\":\"{}\"}}\n", "x".repeat(1000));
        std::fs::write(&path, line.repeat(30_000)).unwrap();
        let first = text(&path).unwrap().unwrap();
        assert_eq!(first.len(), line.len() * 30_000);
        drop(first);
        let before = bytes();
        append(&path, line.repeat(4).as_bytes());
        let grown = text(&path).unwrap().unwrap();
        assert_eq!(*grown, line.repeat(30_004));
        assert_eq!(bytes() - before, line.len() * 4, "only the appended bytes are read");
        let before = bytes();
        assert_eq!(text(&path).unwrap().unwrap(), grown);
        assert_eq!(bytes(), before, "an unchanged file is not read");
    }

    #[test]
    fn a_rewritten_shrunk_or_replaced_session_file_is_read_whole_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "{\"a\":1}\n").unwrap();
        assert_eq!(*text(&path).unwrap().unwrap(), "{\"a\":1}\n");
        std::fs::write(&path, "{\"b\":2}\n{\"c\":3}\n").unwrap();
        assert_eq!(*text(&path).unwrap().unwrap(), "{\"b\":2}\n{\"c\":3}\n", "rewritten longer");
        std::fs::write(&path, "{\"d\":4}\n").unwrap();
        assert_eq!(*text(&path).unwrap().unwrap(), "{\"d\":4}\n", "shrunk");
        let other = dir.path().join("other.jsonl");
        std::fs::write(&other, "{\"d\":4}\n{\"e\":5}\n").unwrap();
        std::fs::rename(&other, &path).unwrap();
        assert_eq!(*text(&path).unwrap().unwrap(), "{\"d\":4}\n{\"e\":5}\n", "replaced");
        std::fs::remove_file(&path).unwrap();
        assert_eq!(text(&path).unwrap(), None, "removed");
    }

    #[test]
    fn a_character_split_between_reads_reads_as_the_whole_file_does() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        let whole = "{\"t\":\"日本\"}\n".as_bytes();
        let bad = [b'a', 0xff, b'b'];
        let cut = whole.len() - 4;
        std::fs::write(&path, &whole[..cut]).unwrap();
        let open = text(&path).unwrap().unwrap();
        assert_eq!(*open, String::from_utf8_lossy(&whole[..cut]));
        append(&path, &whole[cut..]);
        append(&path, &bad);
        let mut all = whole.to_vec();
        all.extend_from_slice(&bad);
        assert_eq!(*text(&path).unwrap().unwrap(), String::from_utf8_lossy(&all));
    }

    #[test]
    #[cfg(unix)]
    fn a_kept_file_that_can_no_longer_be_read_is_an_error() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "{}\n").unwrap();
        text(&path).unwrap().unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o0)).unwrap();
        let denied = text(&path);
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(denied.unwrap_err().to_string().contains("cannot read"));
    }

    #[test]
    fn what_holds_of_a_session_holds_while_it_grows_and_not_once_it_is_rewritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("session.jsonl");
        std::fs::write(&path, "{\"cwd\":\"/w\"}\n").unwrap();
        let first = text(&path).unwrap().unwrap();
        know(&path, &first, "here");
        drop(first);
        assert!(known(&path, "here"));
        append(&path, b"{}\n");
        text(&path).unwrap().unwrap();
        assert!(known(&path, "here"), "a grown session still holds it");
        std::fs::write(&path, "{\"cwd\":\"/other\"}\n{}\n{}\n").unwrap();
        text(&path).unwrap().unwrap();
        assert!(!known(&path, "here"), "a rewritten session knows nothing yet");
    }

    /// One tick lists each session tree at most once: a directory is
    /// listed again only once an entry in it was added or removed.
    #[test]
    fn a_directory_is_listed_again_only_once_its_entries_change() {
        let dir = tempfile::tempdir().unwrap();
        let tree = dir.path().join("projects");
        std::fs::create_dir(&tree).unwrap();
        std::fs::write(tree.join("a.jsonl"), "{}\n").unwrap();
        let past = SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::open(&tree).unwrap().set_modified(past).unwrap();
        // The change time cannot be set back: the listing is kept only once
        // the directory has been still for two seconds.
        std::thread::sleep(std::time::Duration::from_millis(2100));
        let lists = || LISTS.with(std::cell::Cell::get);
        let first = listing(&tree).unwrap().unwrap();
        assert_eq!(first.len(), 1);
        let listed = lists();
        for _ in 0..10 {
            assert_eq!(listing(&tree).unwrap().unwrap(), first);
        }
        assert_eq!(lists(), listed, "an unchanged directory is not listed again");
        std::fs::write(tree.join("a.jsonl"), "{}\n{}\n").unwrap();
        assert_eq!(listing(&tree).unwrap().unwrap(), first, "a written file adds no entry");
        assert_eq!(lists(), listed);
        std::fs::write(tree.join("b.jsonl"), "{}\n").unwrap();
        assert_eq!(listing(&tree).unwrap().unwrap().len(), 2, "an added file is listed");
        assert_eq!(lists(), listed + 1);
    }
}
