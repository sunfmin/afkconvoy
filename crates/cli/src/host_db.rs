//! The host's one afkconvoy database (ADR-0117): `host.db` in the
//! data directory (`copies::data`), where the host-local state lives that
//! no other host reads and nothing else gives back. Its candidate chains
//! (`chain_store`) and each store that was a file of the cache directory
//! before (`Store`) are tables of it.
//!
//! Its schema is versioned (`PRAGMA user_version`) and each version is one
//! entry of `MIGRATIONS`, run once, in order, in one transaction, by the
//! first open that finds the database behind. A change of the schema is a
//! new entry, never an edit of one.
//!
//! What an older afkconvoy kept as files converges into it as it is read
//! (`converge`): a store's old file is taken in, in the transaction that
//! records which file it took, and is moved aside to `<name>.migrated` once
//! no host runner of an older release holds the host — until then that
//! runner may read and write it still, and it is taken in again each time it
//! changes. A `host.db` an older afkconvoy kept in the cache directory is
//! copied here the same way, whole, by SQLite's backup.
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The database's name in the data directory, and an older one's in the
/// cache directory.
pub const FILE: &str = "host.db";

/// What an old file is renamed to once it is taken in.
const MIGRATED: &str = "migrated";

/// How long a writer waits for another's transaction to end.
const BUSY: Duration = Duration::from_secs(60);

/// Version `n` is `MIGRATIONS[n - 1]`.
const MIGRATIONS: &[&str] = &[
    // One candidate chain per repo (the key of its common git dir) and target
    // branch, and the revision a write of it was made on.
    "CREATE TABLE chains (
        repo TEXT NOT NULL,
        branch TEXT NOT NULL,
        revision INTEGER NOT NULL,
        chain TEXT NOT NULL,
        PRIMARY KEY (repo, branch)
    )",
    // Each old file taken in, by its path, and
    // the digest of what it held when it was: one that changed is taken in
    // again.
    "CREATE TABLE imported (
        file TEXT PRIMARY KEY,
        digest TEXT NOT NULL
    )",
    // The checkout list (`checkout_list`): each checkout, by its root, in the
    // list's order; and the one row of what the list holds beside them.
    "CREATE TABLE checkouts (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE checkout_list (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL)",
    // The rest of the host's state that was files (each table's store names
    // it): the harnesses paused, worker health, the account book, the patrol
    // runs' usage, the asking session's and the watch's state, the resource
    // episodes, the human-decision and daybook outboxes, the harnesses that
    // took their briefs, the trash directories; and, by repo, the landing
    // hold and the fresh looks. `daybook_outbox` is read and written by
    // nothing since the watch writes its daybook entry at once (ADR-0118);
    // no later version drops it, which a host runner of the release before
    // that version, still holding the host, could no longer open.
    "CREATE TABLE paused (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE worker_health (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE account_book (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE patrol_runs (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE asking_state (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE asking_terminal (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE watch_state (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE watch_terminal (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE resources (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE notify_outbox (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE daybook_outbox (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE delivered (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE trash_dirs (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE landing_holds (repo TEXT PRIMARY KEY, hold TEXT NOT NULL);
     CREATE TABLE fresh_looks (
        repo TEXT NOT NULL,
        issue INTEGER NOT NULL,
        said TEXT NOT NULL,
        PRIMARY KEY (repo, issue)
     )",
    // The host kit (`kit`), whole, as its file `kit.json` in the config
    // directory held it; and the one row of when it was last written and a
    // refresh of it last tried, in epoch seconds, which were that file's
    // modification time and `kit.refresh` beside it.
    "CREATE TABLE kit (position INTEGER PRIMARY KEY, key TEXT NOT NULL, value TEXT NOT NULL);
     CREATE TABLE kit_refresh (
        one INTEGER PRIMARY KEY CHECK (one = 0),
        written INTEGER,
        tried INTEGER
     );
     CREATE TABLE github_quota (
        host TEXT NOT NULL,
        login TEXT NOT NULL,
        name TEXT NOT NULL,
        note TEXT NOT NULL,
        PRIMARY KEY (host, login, name)
     );
     CREATE TABLE github_identities (
        host TEXT NOT NULL,
        credential TEXT NOT NULL,
        note TEXT NOT NULL,
        PRIMARY KEY (host, credential)
     );
     CREATE TABLE shared_readings (
        scope TEXT NOT NULL,
        key TEXT NOT NULL,
        kept TEXT NOT NULL,
        PRIMARY KEY (scope, key)
     );
     CREATE TABLE need_stderr (name TEXT PRIMARY KEY, text TEXT NOT NULL);
     CREATE TABLE need_installs (name TEXT PRIMARY KEY, installed TEXT NOT NULL)",
];

/// The host's database: `host.db` in the data directory.
pub fn path() -> Result<PathBuf> {
    Ok(crate::copies::data()?.join(FILE))
}

/// The host's database, migrated to the version this afkconvoy knows.
pub(crate) fn open() -> Result<Connection> {
    Host::of_host()?.open()
}

/// What `store` holds on this host, as its file held it (`Host::value`).
pub(crate) fn value(store: &Store) -> Result<Option<Value>> {
    Host::of_host()?.value(store)
}

/// What `store` holds on this host, as a `T` (`Host::read`).
pub(crate) fn read<T: DeserializeOwned + Default>(store: &Store) -> Result<T> {
    Host::of_host()?.read(store)
}

/// Changes what `store` holds on this host in one transaction (`Host::change`).
pub(crate) fn change<T, R>(store: &Store, change: impl FnOnce(&mut T) -> R) -> Result<R>
where
    T: DeserializeOwned + Serialize + Default,
{
    Host::of_host()?.change(store, change)
}

/// Makes `store` on this host hold `value` alone (`Host::replace`).
pub(crate) fn put<T: Serialize>(store: &Store, value: &T) -> Result<()> {
    Host::of_host()?.replace(store, &serde_json::to_value(value)?)
}

/// A host database: its file, and — the host's own — the cache directory an
/// older afkconvoy kept its stores in.
#[derive(Clone, Debug)]
pub struct Host {
    db: PathBuf,
    cache: Option<PathBuf>,
}

impl Host {
    /// This host's: `host.db` in the data directory, converging from the
    /// cache directory.
    pub fn of_host() -> Result<Host> {
        Ok(Host { db: path()?, cache: Some(crate::copies::cache()?) })
    }

    /// The database in `db`, which converges from no files.
    pub fn at(db: PathBuf) -> Host {
        Host { db, cache: None }
    }

    /// The database's file.
    pub fn file(&self) -> &Path {
        &self.db
    }

    /// The cache directory whose old files it converges from, where it does.
    pub(crate) fn cache(&self) -> Option<&Path> {
        self.cache.as_deref()
    }

    /// The database, made where there is none, migrated; the host's copied
    /// from an older afkconvoy's cache directory first, where it has one.
    pub(crate) fn open(&self) -> Result<Connection> {
        let dir = self.db.parent().context("the host database has no directory")?;
        std::fs::create_dir_all(dir).with_context(|| format!("cannot make {}", dir.display()))?;
        if let Some(cache) = &self.cache {
            copied_in(&cache.join(FILE), &self.db)?;
        }
        #[cfg(test)]
        tests::OPENS.set(tests::OPENS.get() + 1);
        let mut db = Connection::open(&self.db)
            .with_context(|| format!("cannot open the host database {}", self.db.display()))?;
        user_only(&self.db)?;
        db.busy_timeout(BUSY)?;
        migrate(&mut db)?;
        if let Some(cache) = &self.cache {
            let old = cache.join(FILE);
            // A hot journal beside it is a write an older process has not
            // ended: it stays until it has.
            if old.exists()
                && !old.with_extension("db-journal").exists()
                && !another_build_holds(cache)
            {
                aside(&old)?;
            }
        }
        Ok(db)
    }

    /// This thread's open connection to the database, with `store`'s old
    /// file taken in: the one it kept from an earlier look where the file is
    /// still the one it opened, else a new one. It is taken out of the thread's
    /// slots while it is used (`keep_slot` puts it back), so a call made
    /// inside another's closure opens a connection of its own.
    fn slot_for(&self, store: &Store) -> Result<Slot> {
        let id = file_id(&self.db);
        let kept = SLOTS.with(|slots| slots.borrow_mut().remove(&self.db));
        let mut slot = match (kept, id) {
            (Some(slot), Some(id)) if slot.id == id => {
                // What `open` does beside making a connection.
                if let Some(cache) = &self.cache {
                    let old = cache.join(FILE);
                    if old.exists()
                        && !old.with_extension("db-journal").exists()
                        && !another_build_holds(cache)
                    {
                        aside(&old)?;
                    }
                }
                slot
            }
            _ => {
                let conn = self.open()?;
                let id = file_id(&self.db).context("the host database was not made")?;
                Slot { conn, id, reads: HashMap::new() }
            }
        };
        if let Some(cache) = &self.cache {
            if cache.join(store.file).exists() {
                // Taking in writes what this connection does not see change.
                slot.reads.clear();
            }
            converge(&mut slot.conn, store, cache)?;
        }
        Ok(slot)
    }

    /// Keeps `slot` for this thread's next look. One not kept — an error
    /// ended its use — is closed.
    fn keep_slot(&self, slot: Slot) {
        SLOTS.with(|slots| slots.borrow_mut().insert(self.db.clone(), slot));
    }

    /// What `store` holds, as its file held it; `None` where it holds
    /// nothing. Where there is no database and nothing to make one of, a
    /// reader makes none. What was read is kept, and read again only after
    /// a write this thread made, or a commit another connection made.
    pub fn value(&self, store: &Store) -> Result<Option<Value>> {
        let to_take_in =
            |cache: &PathBuf| cache.join(FILE).exists() || cache.join(store.file).exists();
        if !self.db.exists() && !self.cache.as_ref().is_some_and(to_take_in) {
            return Ok(None);
        }
        let mut slot = self.slot_for(store)?;
        let version: i64 = slot.conn.query_row("PRAGMA data_version", [], |row| row.get(0))?;
        let held = match slot.reads.get(store.file) {
            Some((at, value)) if *at == version => value.clone(),
            _ => {
                let value = store.export(&slot.conn)?;
                slot.reads.insert(store.file, (version, value.clone()));
                value
            }
        };
        self.keep_slot(slot);
        Ok(held)
    }

    /// What `store` holds, as a `T`; `T`'s default where it holds nothing.
    pub fn read<T: DeserializeOwned + Default>(&self, store: &Store) -> Result<T> {
        typed(store, &self.db, self.value(store)?)
    }

    /// Changes what `store` holds by `change`, in one immediate transaction:
    /// no other writer of the host takes between its reading and its
    /// writing → what `change` answered.
    pub fn change<T, R>(&self, store: &Store, change: impl FnOnce(&mut T) -> R) -> Result<R>
    where
        T: DeserializeOwned + Serialize + Default,
    {
        let mut slot = self.slot_for(store)?;
        let answer = write(&mut slot.conn, |tx| {
            let mut held: T = typed(store, &self.db, store.export(tx)?)?;
            let answer = change(&mut held);
            store.import(tx, &serde_json::to_value(&held)?)?;
            Ok(answer)
        })?;
        slot.reads.clear();
        self.keep_slot(slot);
        Ok(answer)
    }

    /// Makes `store` hold `value` and nothing else, as its file held it.
    pub fn replace(&self, store: &Store, value: &Value) -> Result<()> {
        let mut slot = self.slot_for(store)?;
        write(&mut slot.conn, |tx| store.import(tx, value))?;
        slot.reads.clear();
        self.keep_slot(slot);
        Ok(())
    }
}

/// Has `file` be readable and writable by its user alone, where it is not:
/// the host database keeps what its user alone may read, as the stderr of a
/// host's checks. SQLite makes its journal with the same mode.
fn user_only(file: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(file)?.permissions().mode() & 0o777;
    if mode != 0o600 {
        std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("cannot make {} its user's alone", file.display()))?;
    }
    Ok(())
}

/// A thread's open connection to a host database, and what it read of its
/// stores: each store's value as of the connection's `data_version`, which
/// moves when another connection commits. Its own writes clear it.
struct Slot {
    conn: Connection,
    /// The file the connection opened: a file replaced is opened again.
    id: (u64, u64),
    reads: HashMap<&'static str, (i64, Option<Value>)>,
}

thread_local! {
    /// This thread's open connection to each host database it has looked at.
    static SLOTS: RefCell<HashMap<PathBuf, Slot>> = RefCell::new(HashMap::new());
}

/// Which file `path` is now: none where there is none.
fn file_id(path: &Path) -> Option<(u64, u64)> {
    std::fs::metadata(path).ok().map(|meta| (meta.dev(), meta.ino()))
}

/// `value` as a `T`, or an error naming `store` in `db`.
fn typed<T: DeserializeOwned + Default>(
    store: &Store,
    db: &Path,
    value: Option<Value>,
) -> Result<T> {
    let Some(value) = value else { return Ok(T::default()) };
    serde_json::from_value(value).with_context(|| {
        format!("what the host database {} keeps of {} cannot be read", db.display(), store.name())
    })
}

thread_local! {
    /// Whether this thread is inside a write transaction of the host database.
    static WRITING: Cell<bool> = const { Cell::new(false) };
}

/// Runs `body` in one immediate transaction of `db`, committed where it
/// answers `Ok`. A write begun inside another on the same thread is refused:
/// it would wait on the transaction it is inside for as long as `BUSY`.
pub(crate) fn write<R>(
    db: &mut Connection,
    body: impl FnOnce(&Transaction) -> Result<R>,
) -> Result<R> {
    struct Inside;
    impl Drop for Inside {
        fn drop(&mut self) {
            WRITING.set(false);
        }
    }
    if WRITING.get() {
        bail!(
            "a write of the host database was begun inside another's transaction, on whose end \
             it would wait"
        );
    }
    WRITING.set(true);
    let _inside = Inside;
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let answer = body(&tx)?;
    tx.commit()?;
    Ok(answer)
}

/// A store of the host's database: what one file of the cache directory held
/// before, kept in tables of its own. Every table of a store is
/// `(position, key, value)`: a row's JSON `value`, its `key` to find it by,
/// in the order of `position`.
pub struct Store {
    /// Its old file, under the cache directory.
    pub file: &'static str,
    /// What its old file holds.
    pub old: Old,
    pub layout: Layout,
}

/// What a store's old file holds.
pub enum Old {
    /// One JSON value.
    Json,
    /// One JSON value, which meant what the function makes of it: what
    /// this release would say for the same.
    Meant(fn(Value) -> Value),
    /// Lines of text: a list of them, the empty ones left out.
    Lines,
}

/// How a store is kept in its tables.
pub enum Layout {
    /// Whole, as one JSON value: the one row of the table named.
    Whole(&'static str),
    /// A list: each item a row of the table named, in the list's order, by
    /// the key the function gives it.
    Rows(&'static str, fn(&Value) -> String),
    /// In tables of its own, as `export` reads them back into what the file
    /// held — `None` where they hold nothing — and `import` makes them hold
    /// a value of it, and only that.
    Tables {
        export: fn(&Connection) -> Result<Option<Value>>,
        import: fn(&Connection, &Value) -> Result<()>,
    },
}

impl Store {
    /// The store as a human names it: its old file.
    pub fn name(&self) -> &'static str {
        self.file
    }

    fn export(&self, db: &Connection) -> Result<Option<Value>> {
        #[cfg(test)]
        tests::EXPORTS.set(tests::EXPORTS.get() + 1);
        match self.layout {
            Layout::Whole(table) => Ok(rows(db, table)?.into_iter().next().map(|(_, value)| value)),
            Layout::Rows(table, _) => {
                let rows = rows(db, table)?;
                Ok((!rows.is_empty()).then(|| rows.into_iter().map(|(_, value)| value).collect()))
            }
            Layout::Tables { export, .. } => export(db),
        }
    }

    fn import(&self, db: &Connection, value: &Value) -> Result<()> {
        match self.layout {
            Layout::Whole(table) => put_rows(db, table, [(String::new(), value)]),
            Layout::Rows(table, key) => {
                let items =
                    value.as_array().with_context(|| format!("{} is no list", self.file))?;
                put_rows(db, table, items.iter().map(|item| (key(item), item)))
            }
            Layout::Tables { import, .. } => import(db, value),
        }
    }

    /// Takes in what its old file held, `bytes`.
    fn take_in(&self, db: &Connection, bytes: &[u8]) -> Result<()> {
        let value = match self.old {
            Old::Json => serde_json::from_slice(bytes)?,
            Old::Meant(meant) => meant(serde_json::from_slice(bytes)?),
            Old::Lines => {
                let text = String::from_utf8_lossy(bytes);
                text.lines().filter(|line| !line.is_empty()).map(Value::from).collect()
            }
        };
        self.import(db, &value)
    }
}

/// The key of a store's row of `repo`: its canonical common git dir,
/// hashed, as its candidate worktrees and chains are keyed, so every
/// checkout and worktree of one repo shares it.
pub(crate) fn repo_key(repo: &crate::git::Repo) -> Result<String> {
    let common = std::fs::canonicalize(repo.common_dir()?)?;
    Ok(format!("{:016x}", afkconvoy_core::gate::key(&common.to_string_lossy())))
}

/// The rows of `table`, a table of `(position, key, value)`, in their order.
pub(crate) fn rows(db: &Connection, table: &str) -> Result<Vec<(String, Value)>> {
    let mut statement = db.prepare(&format!("SELECT key, value FROM {table} ORDER BY position"))?;
    let rows =
        statement.query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))?;
    let mut found = vec![];
    for row in rows {
        let (key, text) = row?;
        found.push((key, serde_json::from_str(&text)?));
    }
    Ok(found)
}

/// Makes `table`, a table of `(position, key, value)`, hold `rows` alone, in
/// their order.
pub(crate) fn put_rows<'a>(
    db: &Connection,
    table: &str,
    rows: impl IntoIterator<Item = (String, &'a Value)>,
) -> Result<()> {
    db.execute(&format!("DELETE FROM {table}"), [])?;
    let mut insert =
        db.prepare(&format!("INSERT INTO {table} (position, key, value) VALUES (?1, ?2, ?3)"))?;
    for (position, (key, value)) in rows.into_iter().enumerate() {
        insert.execute(params![position as i64, key, serde_json::to_string(value)?])?;
    }
    Ok(())
}

/// Takes `store`'s old file in from `cache` (`converge_file`).
fn converge(db: &mut Connection, store: &Store, cache: &Path) -> Result<()> {
    converge_file(db, &cache.join(store.file), cache, |tx, bytes| store.take_in(tx, bytes))
}

/// Takes the old file `file` in by `take_in`, given what it holds, where it
/// holds what was not taken in yet — in the transaction that records its
/// digest under its path; and moves it aside once no host runner of an
/// older release holds the host of `cache`, which may read or write it until
/// then. Run again, it changes nothing.
pub(crate) fn converge_file(
    db: &mut Connection,
    file: &Path,
    cache: &Path,
    take_in: impl FnOnce(&Transaction, &[u8]) -> Result<()>,
) -> Result<()> {
    let bytes = match std::fs::read(file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", file.display())),
    };
    let digest = format!("{:016x}", afkconvoy_core::gate::key(&String::from_utf8_lossy(&bytes)));
    let name = file.to_string_lossy().into_owned();
    write(db, |tx| {
        let taken: Option<String> = tx
            .query_row("SELECT digest FROM imported WHERE file = ?1", [&name], |row| row.get(0))
            .optional()?;
        if taken.as_deref() == Some(digest.as_str()) {
            return Ok(());
        }
        take_in(tx, &bytes).with_context(|| {
            format!(
                "{} cannot be read into the host database: mend it, or remove it",
                file.display()
            )
        })?;
        tx.execute(
            "INSERT OR REPLACE INTO imported (file, digest) VALUES (?1, ?2)",
            params![name, digest],
        )?;
        Ok(())
    })?;
    if !another_build_holds(cache) {
        aside(file)?;
    }
    Ok(())
}

/// Moves `file` aside to `<file>.migrated`, kept: renamed back, an older
/// afkconvoy reads it again. One already gone is moved.
pub(crate) fn aside(file: &Path) -> Result<()> {
    let mut to = file.as_os_str().to_owned();
    to.push(format!(".{MIGRATED}"));
    match std::fs::rename(file, &to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("cannot move {} aside", file.display())),
    }
}

/// Whether a host runner of another build holds the host of `cache`, so an
/// old file must stay where it is: one of an older release reads and writes
/// the old files still, until it moves onto this release between two ticks
/// (ADR-0023); and one of this same release but another executable — the
/// installed afkconvoy while a test or a dev build runs — may be older than
/// its version says. Only a free host, a runner of a newer release, or one
/// started from this very executable lets a file be moved aside: a dev
/// build's tests once moved the host's real kit.json aside under a runner
/// whose version matched (2026-11-17).
pub(crate) fn another_build_holds(cache: &Path) -> bool {
    let lock = cache.join(crate::host_runner::LOCK);
    let Ok(file) = std::fs::File::open(&lock) else { return false };
    // Free: no host runner holds it. A runner starting meanwhile waits for
    // it, as it waits for one that holds it.
    if file.try_lock_shared().is_ok() {
        return false;
    }
    let holder = std::fs::read_to_string(&lock).ok();
    let holder = holder.and_then(|text| serde_json::from_str::<Value>(&text).ok());
    let version = holder.as_ref().and_then(|holder| holder["version"].as_str());
    use afkconvoy_core::host_runner::{Found, found};
    // One that says no version is of a release before it was said.
    let Some(version) = version else { return true };
    match found(crate::VERSION, version) {
        Found::Newer => false,
        Found::Older => true,
        // The same release: this build only if it is this executable.
        _ => {
            let theirs = holder.as_ref().and_then(|holder| holder["exe"].as_str());
            theirs.is_none_or(|theirs| Some(PathBuf::from(theirs)) != this_executable())
        }
    }
}

/// This process's executable, its links resolved: what a host runner's lock
/// names as `exe`.
pub(crate) fn this_executable() -> Option<PathBuf> {
    std::fs::canonicalize(std::env::current_exe().ok()?).ok()
}

/// Copies `old`, a host database an older afkconvoy kept, to `new` where
/// there is none yet: by SQLite's backup, which reads it whole while its
/// writers go on, into a file beside `new` that becomes it only where none
/// came meanwhile.
fn copied_in(old: &Path, new: &Path) -> Result<()> {
    if new.exists() || !old.exists() {
        return Ok(());
    }
    let part = new.with_extension(format!("db.{}.part", crate::unique()));
    let copied = backup(old, &part).and_then(|()| match std::fs::hard_link(&part, new) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => Ok(()),
        Err(error) => Err(error.into()),
    });
    let _ = std::fs::remove_file(&part);
    copied.with_context(|| format!("cannot copy {} to {}", old.display(), new.display()))
}

/// Writes a copy of the database `from` to `to`, read whole in one pass of
/// SQLite's online backup, while others go on writing it.
pub(crate) fn backup(from: &Path, to: &Path) -> Result<()> {
    use rusqlite::backup::{Backup, StepResult};
    let source = Connection::open_with_flags(from, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    source.busy_timeout(BUSY)?;
    let mut copy = Connection::open(to)?;
    let backup = Backup::new(&source, &mut copy)?;
    let until = std::time::Instant::now() + BUSY;
    loop {
        match backup.step(-1)? {
            StepResult::Done => return Ok(()),
            StepResult::More => {}
            StepResult::Busy | StepResult::Locked if std::time::Instant::now() < until => {
                std::thread::sleep(Duration::from_millis(100));
            }
            _ => bail!("{} stayed locked for {}s", from.display(), BUSY.as_secs()),
        }
    }
}

/// How many days of backups the data directory keeps, the newest.
pub const BACKUPS_KEPT: usize = 14;

/// The data directory's directory of backups, a directory a day.
const BACKUPS: &str = "backups";

/// In `BACKUPS`: why the last backup failed, {at, error}; none once one
/// is made.
const FAILED: &str = "failed";

/// The databases a backup copies, each where it is: the host's, and its
/// local tracker's (ADR-0091) where it has one. The last is the one whose
/// copy says the day's backup is made.
const BACKED_UP: [&str; 2] = ["issues.db", FILE];

/// Makes the day `day`'s backup of the data directory's databases, where it
/// is not made yet (`back_up_in`) → whether it made it.
pub(crate) fn back_up(day: &str, now: u64) -> Result<bool> {
    back_up_in(&crate::copies::data()?, day, now)
}

/// Makes the backup of `day` (`YYYY-MM-DD`) in `data`: an online copy of
/// each of `BACKED_UP` there into `backups/<day>/`, made beside its name
/// and moved to it whole, then keeps the newest `BACKUPS_KEPT` days. A
/// failure is kept as `backups/failed`, which `doctor` finds, until one is
/// made.
fn back_up_in(data: &Path, day: &str, now: u64) -> Result<bool> {
    let backups = data.join(BACKUPS);
    let into = backups.join(day);
    if into.join(FILE).exists() || !data.join(FILE).exists() {
        return Ok(false);
    }
    let made = (|| -> Result<()> {
        std::fs::create_dir_all(&into)?;
        for name in BACKED_UP {
            let from = data.join(name);
            if !from.exists() {
                continue;
            }
            let part = into.join(format!("{name}.part"));
            let _ = std::fs::remove_file(&part);
            backup(&from, &part)?;
            user_only(&part)?;
            std::fs::rename(&part, into.join(name))?;
        }
        let mut days: Vec<_> = std::fs::read_dir(&backups)?
            .flatten()
            .filter(|entry| entry.path().is_dir())
            .map(|entry| entry.path())
            .collect();
        days.sort();
        let past = days.len().saturating_sub(BACKUPS_KEPT);
        for old in &days[..past] {
            std::fs::remove_dir_all(old)?;
        }
        Ok(())
    })();
    match made {
        Ok(()) => {
            let _ = std::fs::remove_file(backups.join(FAILED));
            Ok(true)
        }
        Err(error) => {
            let said = serde_json::json!({"at": now, "error": format!("{error:#}")});
            let _ = std::fs::create_dir_all(&backups);
            let _ = std::fs::write(backups.join(FAILED), said.to_string());
            Err(error.context(format!(
                "the backup of {} into {} failed",
                data.display(),
                into.display()
            )))
        }
    }
}

/// What `doctor` finds of the host database's backups: the last that failed,
/// where none was made since.
pub fn findings() -> Vec<crate::doctor::Finding> {
    crate::copies::data().map(|data| findings_in(&data)).unwrap_or_default()
}

fn findings_in(data: &Path) -> Vec<crate::doctor::Finding> {
    let backups = data.join(BACKUPS);
    let Ok(text) = std::fs::read_to_string(backups.join(FAILED)) else { return vec![] };
    let failed: Value = serde_json::from_str(&text).unwrap_or_default();
    vec![crate::doctor::finding(
        crate::doctor::Kind::HostDatabase,
        serde_json::json!(["backup-failed"]),
        "the host database was not backed up today".to_string(),
        format!(
            "at {}: {}; the host runner tries again within the hour",
            failed["at"],
            failed["error"].as_str().unwrap_or("no reason kept")
        ),
        format!("ls {}", backups.display()),
    )]
}

/// Every store of the host database.
const STORES: &[&Store] = &[
    &crate::checkout_list::STORE,
    &crate::paused::STORE,
    &crate::health::STORE,
    &crate::health::DELIVERED,
    &crate::engine_accounts::STORE,
    &crate::patrol_usage::STORE,
    &crate::host_asking::STORE,
    &crate::asking_terminal::STORE,
    &crate::host_watch::STORE,
    &crate::watch_terminal::STORE,
    &crate::resources::STORE,
    &crate::watch_notify::STORE,
    &crate::trash::STORE,
];

/// The stores whose old file was not in the cache directory: each takes it
/// in itself, as the host kit takes in `kit.json` of the config directory.
const ELSEWHERE: &[&Store] = &[&crate::kit::STORE];

/// The store whose old file is `file`.
fn store_of(file: &str) -> Result<&'static Store> {
    let store = STORES.iter().chain(ELSEWHERE).find(|store| store.file == file);
    store.copied().with_context(|| format!("the host database keeps no store of {file}"))
}

/// Makes the store whose old file was `file` hold `value`, as that file held
/// it, in the database `db`: how a test's world keeps a store through the
/// store itself.
pub fn keep(db: &Path, file: &str, value: &Value) -> Result<()> {
    Host::at(db.to_path_buf()).replace(store_of(file)?, value)
}

/// Makes the store whose old file was `file` hold nothing in the database
/// `db`, as that file removed did: how a test's world forgets one.
pub fn forget(db: &Path, file: &str) -> Result<()> {
    let store = store_of(file)?;
    let (Layout::Whole(table) | Layout::Rows(table, _)) = store.layout else {
        bail!("{file} is kept in tables of its own: keep what it holds instead");
    };
    let mut db = Host::at(db.to_path_buf()).open()?;
    write(&mut db, |tx| Ok(tx.execute(&format!("DELETE FROM {table}"), []).map(drop)?))
}

/// What the store whose old file was `file` holds in the database `db`, as
/// that file held it; `None` where it holds nothing.
pub fn kept(db: &Path, file: &str) -> Result<Option<Value>> {
    Host::at(db.to_path_buf()).value(store_of(file)?)
}

fn migrate(db: &mut Connection) -> Result<()> {
    let known = MIGRATIONS.len() as i64;
    let at = |db: &Connection| db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0));
    if at(db)? == known {
        return Ok(());
    }
    // Immediate: two opens that find it behind run the migrations in turn.
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found = at(&tx)?;
    if found > known {
        bail!(
            "the host database is at version {found}, newer than this afkconvoy's {known}; \
             install a newer afkconvoy"
        );
    }
    for (index, sql) in MIGRATIONS.iter().enumerate().skip(found as usize) {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as i64 + 1)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    thread_local! {
        /// How many connections, and how many store exports, this thread made.
        pub(super) static OPENS: Cell<usize> = const { Cell::new(0) };
        pub(super) static EXPORTS: Cell<usize> = const { Cell::new(0) };
    }

    #[test]
    fn a_runners_looks_open_the_database_once_and_export_a_store_only_after_it_changed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join(FILE);
        let list = serde_json::json!({"checkouts": (0..5)
            .map(|n| serde_json::json!({"checkout": format!("/c{n}")})).collect::<Vec<_>>()});
        keep(&file, "checkouts.json", &list).unwrap();
        let host = Host::at(file.clone());
        let store = store_of("checkouts.json").unwrap();
        OPENS.set(0);
        EXPORTS.set(0);
        for _ in 0..10 {
            assert_eq!(
                host.value(store).unwrap().unwrap()["checkouts"].as_array().unwrap().len(),
                5
            );
        }
        assert!(OPENS.get() <= 1, "ten looks open the database at most once");
        assert_eq!(EXPORTS.get(), 1, "and export the store once");
        // another connection's commit is seen
        let mut other = Connection::open(&file).unwrap();
        write(&mut other, |tx| store.import(tx, &serde_json::json!({"checkouts": []}))).unwrap();
        assert_eq!(host.value(store).unwrap().unwrap()["checkouts"].as_array().unwrap().len(), 0);
        // so is this thread's own write
        host.change(store, |held: &mut Value| {
            held["checkouts"] = serde_json::json!([{"checkout": "/z"}])
        })
        .unwrap();
        assert_eq!(host.value(store).unwrap().unwrap()["checkouts"][0]["checkout"], "/z");
        assert!(OPENS.get() <= 1, "still the one connection");
        // a file replaced is opened again
        std::fs::remove_file(&file).unwrap();
        assert_eq!(host.value(store).unwrap(), None);
    }

    #[test]
    fn the_host_database_is_its_users_alone() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let db = dir.path().join("host.db");
        std::fs::write(&db, "").unwrap();
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).unwrap();
        Host::at(db.clone()).open().unwrap();
        assert_eq!(std::fs::metadata(&db).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[test]
    fn a_database_behind_is_migrated_once_and_one_ahead_is_refused() {
        let mut db = Connection::open_in_memory().unwrap();
        migrate(&mut db).unwrap();
        migrate(&mut db).unwrap();
        let version: i64 = db.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, MIGRATIONS.len() as i64);
        db.pragma_update(None, "user_version", MIGRATIONS.len() as i64 + 1).unwrap();
        assert!(migrate(&mut db).is_err());
    }

    /// A host whose cache directory is `dir/cache` and whose database is
    /// in `dir/data`.
    pub(crate) fn host(dir: &Path) -> Host {
        Host { db: dir.join("data/host.db"), cache: Some(dir.join("cache")) }
    }

    /// Holds the host runner's lock in `cache`, as a host runner of `version`
    /// of another executable.
    pub(crate) fn runner(cache: &Path, version: &str) -> std::fs::File {
        runner_of(cache, version, Some(PathBuf::from("/elsewhere/afkconvoy")))
    }

    /// The same, as one started from `exe`.
    pub(crate) fn runner_of(cache: &Path, version: &str, exe: Option<PathBuf>) -> std::fs::File {
        std::fs::create_dir_all(cache).unwrap();
        let lock = cache.join(crate::host_runner::LOCK);
        let holder = serde_json::json!({"pid": 1, "version": version, "exe": exe});
        std::fs::write(&lock, holder.to_string()).unwrap();
        let file = std::fs::File::open(&lock).unwrap();
        file.lock().unwrap();
        file
    }

    fn chains(db: &Connection) -> Vec<String> {
        let mut statement = db.prepare("SELECT chain FROM chains ORDER BY branch").unwrap();
        statement.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect()
    }

    #[test]
    fn an_older_cache_database_is_copied_in_and_moved_aside_once_no_older_runner_holds_the_host() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path());
        let cache = dir.path().join("cache");
        let old = Host::at(cache.join(FILE)).open().unwrap();
        old.execute(r#"INSERT INTO chains VALUES ('r', 'main', 3, '{"kept":1}')"#, []).unwrap();
        drop(old);

        // an older host runner holds the host: copied, and left where it reads it
        let older = runner(&cache, "0.0.1");
        assert_eq!(chains(&host.open().unwrap()), [r#"{"kept":1}"#]);
        assert!(cache.join(FILE).exists(), "an older runner may still read it");
        let new = host.open().unwrap();
        new.execute(r#"UPDATE chains SET chain = '{"kept":2}'"#, []).unwrap();
        drop(new);
        assert_eq!(chains(&host.open().unwrap()), [r#"{"kept":2}"#], "copied once, never again");
        drop(older);

        // a runner of this release from another executable: left too
        let other = runner(&cache, crate::VERSION);
        host.open().unwrap();
        assert!(cache.join(FILE).exists(), "another build of this release may read it");
        drop(other);

        // this very build, or none: moved aside, kept
        let ours = runner_of(&cache, crate::VERSION, this_executable());
        assert_eq!(chains(&host.open().unwrap()), [r#"{"kept":2}"#]);
        assert!(!cache.join(FILE).exists());
        assert!(cache.join("host.db.migrated").exists(), "kept, to be renamed back");
        drop(ours);
        assert_eq!(
            chains(&host.open().unwrap()),
            [r#"{"kept":2}"#],
            "a second open changes nothing"
        );
        let left: Vec<_> = std::fs::read_dir(dir.path().join("data"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(left, [FILE], "no part of a copy is left");
    }

    #[test]
    fn a_days_backup_is_a_readable_copy_made_once_and_the_newest_fourteen_days_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path();
        assert!(!back_up_in(data, "2026-11-01", 1).unwrap(), "no database, no backup");
        let db = Host::at(data.join(FILE)).open().unwrap();
        db.execute(r#"INSERT INTO chains VALUES ('r', 'main', 1, '{"kept":1}')"#, []).unwrap();
        // a writer holds it while the backup reads it
        let writing = Host::at(data.join(FILE)).open().unwrap();
        writing.execute_batch("BEGIN IMMEDIATE; UPDATE chains SET revision = 2").unwrap();
        assert!(back_up_in(data, "2026-11-01", 1).unwrap());
        writing.execute_batch("COMMIT").unwrap();
        let copy = Connection::open(data.join("backups/2026-11-01").join(FILE)).unwrap();
        assert_eq!(chains(&copy), [r#"{"kept":1}"#], "a readable copy");
        assert!(!back_up_in(data, "2026-11-01", 2).unwrap(), "made once a day");
        for day in 2..=20 {
            back_up_in(data, &format!("2026-11-{day:02}"), 3).unwrap();
        }
        let mut kept: Vec<_> = std::fs::read_dir(data.join(BACKUPS))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        kept.sort();
        assert_eq!(kept.len(), BACKUPS_KEPT, "{kept:?}");
        assert_eq!((kept[0].as_str(), kept[13].as_str()), ("2026-11-07", "2026-11-20"));
        assert!(findings_in(data).is_empty());
        // a failure is a finding, not a crash, until a backup is made
        std::fs::write(data.join(BACKUPS).join("2026-11-21"), "no directory").unwrap();
        assert!(back_up_in(data, "2026-11-21", 4).is_err());
        let found = findings_in(data);
        assert_eq!(found.len(), 1);
        assert!(found[0].evidence.contains("at 4"), "{}", found[0].evidence);
        std::fs::remove_file(data.join(BACKUPS).join("2026-11-21")).unwrap();
        assert!(back_up_in(data, "2026-11-21", 5).unwrap());
        assert!(findings_in(data).is_empty());
    }

    /// What each store's old file held, as a test writes it, and what the
    /// store then holds.
    fn old_files() -> Vec<(&'static str, String, Value)> {
        use serde_json::json;
        let whole = |file| (file, r#"{"kept": 1}"#.to_string(), json!({"kept": 1}));
        let book = json!({"accounts": {"a": {"windows": []}}});
        let pause = json!([{"harness": "pi", "since": 1, "why": "refused"}]);
        let runs = json!([{"repo": "r", "id": "r:patrol:p:1:pi"}]);
        let list = json!({"checkouts": [{"checkout": "/a", "tracker": "acme/tracker"}]});
        vec![
            ("checkouts.json", list.to_string(), list),
            ("paused.json", pause.to_string(), pause),
            whole("worker-health.json"),
            ("watch/delivered.json", r#"["codex", "pi"]"#.into(), json!(["codex", "pi"])),
            ("accounts/book.json", book.to_string(), book),
            ("accounts/patrol-runs.json", runs.to_string(), runs),
            whole("asking/state.json"),
            whole("asking/terminal.json"),
            whole("watch/state.json"),
            whole("watch/terminal.json"),
            whole("watch/resources.json"),
            whole("watch/notify.json"),
            (
                "trash-dirs",
                "/a/.afkconvoy-trash\n\n/b/.afkconvoy-trash\n".into(),
                json!(["/a/.afkconvoy-trash", "/b/.afkconvoy-trash"]),
            ),
        ]
    }

    #[test]
    fn each_stores_old_file_is_taken_in_and_renamed_and_a_second_open_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let host = host(dir.path());
        let cache = dir.path().join("cache");
        let files = old_files();
        let named: Vec<_> = files.iter().map(|(file, ..)| *file).collect();
        let stores: Vec<_> = STORES.iter().map(|store| store.file).collect();
        assert_eq!(named, stores, "every store's old file is taken in by this test");
        for (file, text, _) in &files {
            let at = cache.join(file);
            std::fs::create_dir_all(at.parent().unwrap()).unwrap();
            std::fs::write(at, text).unwrap();
        }
        for (store, (file, text, held)) in STORES.iter().zip(&files) {
            assert_eq!(host.value(store).unwrap().as_ref(), Some(held), "{file}");
            assert!(!cache.join(file).exists(), "{file} is moved aside");
            let migrated = cache.join(format!("{file}.{MIGRATED}"));
            assert_eq!(&std::fs::read_to_string(migrated).unwrap(), text, "{file} kept whole");
        }
        let taken = |db: &Connection| -> i64 {
            db.query_row("SELECT count(*) FROM imported", [], |row| row.get(0)).unwrap()
        };
        let before = taken(&host.open().unwrap());
        assert_eq!(before, files.len() as i64);
        for (store, (file, _, held)) in STORES.iter().zip(&files) {
            assert_eq!(host.value(store).unwrap().as_ref(), Some(held), "{file} again");
        }
        assert_eq!(taken(&host.open().unwrap()), before, "a second open takes in nothing");
    }

    #[test]
    fn a_write_inside_another_on_its_thread_is_refused_and_ends_none() {
        let dir = tempfile::tempdir().unwrap();
        let host = Host::at(dir.path().join(FILE));
        let mut outer = host.open().unwrap();
        let mut inner = host.open().unwrap();
        let error = write(&mut outer, |_| write(&mut inner, |_| Ok(()))).unwrap_err();
        assert!(format!("{error:#}").contains("inside another's transaction"), "{error:#}");
        write(&mut inner, |_| Ok(())).expect("the refusal left no write begun");
    }
}
