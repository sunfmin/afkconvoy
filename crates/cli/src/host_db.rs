//! The host's one afkconvoy database (ADR-0117, ADR-0118): `host.db` in the
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
use std::cell::Cell;
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
    // Each old file of the cache directory taken in, by its name there, and
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
];

/// The host's database: `host.db` in the data directory.
pub fn path() -> Result<PathBuf> {
    Ok(crate::copies::data()?.join(FILE))
}

/// The host's database, migrated to the version this afkconvoy knows.
pub(crate) fn open() -> Result<Connection> {
    Host::of_host()?.open()
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

    /// The database, made where there is none, migrated; the host's copied
    /// from an older afkconvoy's cache directory first, where it has one.
    pub(crate) fn open(&self) -> Result<Connection> {
        let dir = self.db.parent().context("the host database has no directory")?;
        std::fs::create_dir_all(dir).with_context(|| format!("cannot make {}", dir.display()))?;
        if let Some(cache) = &self.cache {
            copied_in(&cache.join(FILE), &self.db)?;
        }
        let mut db = Connection::open(&self.db)
            .with_context(|| format!("cannot open the host database {}", self.db.display()))?;
        db.busy_timeout(BUSY)?;
        migrate(&mut db)?;
        if let Some(cache) = &self.cache {
            let old = cache.join(FILE);
            // A hot journal beside it is a write an older process has not
            // ended: it stays until it has.
            if old.exists() && !old.with_extension("db-journal").exists() && !older_runner(cache) {
                aside(&old)?;
            }
        }
        Ok(db)
    }

    /// The database, with `store`'s old file taken in.
    fn open_for(&self, store: &Store) -> Result<Connection> {
        let mut db = self.open()?;
        if let Some(cache) = &self.cache {
            converge(&mut db, store, cache)?;
        }
        Ok(db)
    }

    /// What `store` holds, as its file held it; `None` where it holds
    /// nothing. Where there is no database and nothing to make one of, a
    /// reader makes none.
    pub fn value(&self, store: &Store) -> Result<Option<Value>> {
        let to_take_in =
            |cache: &PathBuf| cache.join(FILE).exists() || cache.join(store.file).exists();
        if !self.db.exists() && !self.cache.as_ref().is_some_and(to_take_in) {
            return Ok(None);
        }
        store.export(&self.open_for(store)?)
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
        let mut db = self.open_for(store)?;
        write(&mut db, |tx| {
            let mut held: T = typed(store, &self.db, store.export(tx)?)?;
            let answer = change(&mut held);
            store.import(tx, &serde_json::to_value(&held)?)?;
            Ok(answer)
        })
    }

    /// Makes `store` hold `value` and nothing else, as its file held it.
    pub fn replace(&self, store: &Store, value: &Value) -> Result<()> {
        let mut db = self.open_for(store)?;
        write(&mut db, |tx| store.import(tx, value))
    }
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
/// before, kept in tables of its own.
pub struct Store {
    /// Its old file, under the cache directory.
    pub file: &'static str,
    pub layout: Layout,
}

/// How a store is kept in its tables.
pub enum Layout {
    /// Whole, as one JSON value, in the table named: `(id = 1, value)`.
    Whole(&'static str),
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
        match self.layout {
            Layout::Whole(table) => {
                let sql = format!("SELECT value FROM {table} WHERE id = 1");
                let text: Option<String> = db.query_row(&sql, [], |row| row.get(0)).optional()?;
                Ok(text.map(|text| serde_json::from_str(&text)).transpose()?)
            }
            Layout::Tables { export, .. } => export(db),
        }
    }

    fn import(&self, db: &Connection, value: &Value) -> Result<()> {
        match self.layout {
            Layout::Whole(table) => {
                db.execute(
                    &format!("INSERT OR REPLACE INTO {table} (id, value) VALUES (1, ?1)"),
                    params![serde_json::to_string(value)?],
                )?;
                Ok(())
            }
            Layout::Tables { import, .. } => import(db, value),
        }
    }
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

/// Takes `store`'s old file in from `cache` where it holds what was not
/// taken in yet, in the transaction that records it so; and moves it aside
/// once no older host runner may read or write it. Run again, it changes
/// nothing.
fn converge(db: &mut Connection, store: &Store, cache: &Path) -> Result<()> {
    let file = cache.join(store.file);
    let bytes = match std::fs::read(&file) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).with_context(|| format!("cannot read {}", file.display())),
    };
    let digest = format!("{:016x}", afkconvoy_core::gate::key(&String::from_utf8_lossy(&bytes)));
    write(db, |tx| {
        let taken: Option<String> = tx
            .query_row("SELECT digest FROM imported WHERE file = ?1", [store.file], |row| {
                row.get(0)
            })
            .optional()?;
        if taken.as_deref() == Some(digest.as_str()) {
            return Ok(());
        }
        let taken_in = serde_json::from_slice::<Value>(&bytes)
            .map_err(anyhow::Error::from)
            .and_then(|value| store.import(tx, &value));
        taken_in.with_context(|| {
            format!(
                "{} cannot be read into the host database: mend it, or remove it",
                file.display()
            )
        })?;
        tx.execute(
            "INSERT OR REPLACE INTO imported (file, digest) VALUES (?1, ?2)",
            params![store.file, digest],
        )?;
        Ok(())
    })?;
    if !older_runner(cache) {
        aside(&file)?;
    }
    Ok(())
}

/// Moves `file` aside to `<file>.migrated`, kept: renamed back, an older
/// afkconvoy reads it again. One already gone is moved.
fn aside(file: &Path) -> Result<()> {
    let mut to = file.as_os_str().to_owned();
    to.push(format!(".{MIGRATED}"));
    match std::fs::rename(file, &to) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("cannot move {} aside", file.display())),
    }
}

/// Whether a host runner of a release older than this one holds the host of
/// `cache`: it reads and writes the old files still, until it moves onto
/// this release between two ticks (ADR-0032).
fn older_runner(cache: &Path) -> bool {
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
    version.is_none_or(|version| found(crate::VERSION, version) == Found::Older)
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

/// Every store of the host database.
const STORES: &[&Store] = &[&crate::checkout_list::STORE];

/// The store whose old file is `file`.
fn store_of(file: &str) -> Result<&'static Store> {
    let store = STORES.iter().find(|store| store.file == file);
    store.copied().with_context(|| format!("the host database keeps no store of {file}"))
}

/// Makes the store whose old file was `file` hold `value`, as that file held
/// it, in the database `db`: how a test's world keeps a store through the
/// store itself.
pub fn keep(db: &Path, file: &str, value: &Value) -> Result<()> {
    Host::at(db.to_path_buf()).replace(store_of(file)?, value)
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

    /// Holds the host runner's lock in `cache`, as a host runner of `version`.
    pub(crate) fn runner(cache: &Path, version: &str) -> std::fs::File {
        std::fs::create_dir_all(cache).unwrap();
        let lock = cache.join(crate::host_runner::LOCK);
        std::fs::write(&lock, format!(r#"{{"pid": 1, "version": "{version}"}}"#)).unwrap();
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

        // a runner of this release, or none: moved aside, kept
        let ours = runner(&cache, crate::VERSION);
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
