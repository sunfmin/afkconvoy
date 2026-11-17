//! A daybook kept on its host (docs/adr/0118): one SQLite database, a row
//! an entry. A day is the entries of its date; its lessons are read from
//! its entries, so there is no discussion to open nor index to keep.
//!
//! The file and its schema are this crate's alone: more than one program of
//! the host's user writes it at once, each through this crate. Each opens
//! its own connection; WAL lets a read run beside a write, and a writer
//! waits out another's lock (`BUSY`) rather than failing.

use std::path::Path;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result, bail};
use rusqlite::{Connection, ErrorCode, TransactionBehavior, params};

use crate::book::{Held, Lesson};
use crate::day::day_named;
use crate::entry::lesson_of;
use crate::store::Store;

/// How long a write waits for another writer's lock before it fails.
const BUSY: Duration = Duration::from_secs(60);

/// The schema: version `n` is `MIGRATIONS[n - 1]`, run once, in order, by
/// the first open that finds the database behind. A change of the schema is
/// a new entry, never an edit of one.
const MIGRATIONS: &[&str] = &[
    // `day` is the host's local date the entry is of, `at` the epoch second
    // it was written. `AUTOINCREMENT`: an id is never given again.
    "CREATE TABLE entries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        day TEXT NOT NULL,
        at INTEGER NOT NULL,
        text TEXT NOT NULL
    );
    CREATE INDEX entries_by_day ON entries (day, id);",
];

/// One entry as the host's database holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Where it is: `daybook:<id>`, the name a lesson read from it has.
    pub url: String,
    /// The day it is of, `YYYY-MM-DD`.
    pub day: String,
    /// When it was written, in epoch seconds.
    pub at: u64,
    /// Its text, in the shape `entry` gives it.
    pub text: String,
}

/// The name of the entry `id`.
fn url(id: i64) -> String {
    format!("daybook:{id}")
}

/// A daybook in the SQLite database at a path its program chooses.
pub struct Local {
    db: Mutex<Connection>,
}

impl Local {
    /// The daybook at `path`, made with its directory where there is none,
    /// and brought to the schema this crate knows. A database of a newer
    /// schema than this crate knows is refused, and left as it is.
    pub fn open(path: &Path) -> Result<Local> {
        let opened = || -> Result<Local> {
            if let Some(dir) = path.parent().filter(|dir| !dir.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
            let mut db = Connection::open(path)?;
            db.busy_timeout(BUSY)?;
            wal(&db)?;
            migrate(&mut db, MIGRATIONS)?;
            Ok(Local { db: Mutex::new(db) })
        };
        opened().with_context(|| format!("the daybook {}", path.display()))
    }

    fn db(&self) -> std::sync::MutexGuard<'_, Connection> {
        self.db.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// The entries of the days `since` to `until`, both included, oldest
    /// day first and each day's in the order they were written. A day is
    /// read as the span of itself.
    pub fn entries(&self, since: &str, until: &str) -> Result<Vec<Entry>> {
        self.span(since, Some(until))
    }

    fn span(&self, since: &str, until: Option<&str>) -> Result<Vec<Entry>> {
        let db = self.db();
        let mut statement = db.prepare_cached(
            "SELECT id, day, at, text FROM entries
             WHERE day >= ?1 AND (?2 IS NULL OR day <= ?2) ORDER BY day, id",
        )?;
        let rows = statement.query_map(params![since, until], |row| {
            Ok(Entry {
                url: url(row.get(0)?),
                day: row.get(1)?,
                at: row.get::<_, i64>(2)?.max(0) as u64,
                text: row.get(3)?,
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<Vec<Entry>>>()?)
    }
}

impl Store for Local {
    /// Written at once, or not at all: there is nothing to retry later.
    fn write(&self, day: &str, at: u64, entry: &str) -> Result<String> {
        day_named(day).map_err(anyhow::Error::msg)?;
        let at = i64::try_from(at).context("no time a daybook entry is written at")?;
        let db = self.db();
        db.execute(
            "INSERT INTO entries (day, at, text) VALUES (?1, ?2, ?3)",
            params![day, at, entry],
        )
        .context("the daybook entry was not written")?;
        Ok(url(db.last_insert_rowid()))
    }

    fn lessons(&self, since: &str, until: Option<&str>) -> Result<Vec<Lesson>> {
        Ok(self
            .span(since, until)?
            .into_iter()
            .filter_map(|Entry { url, day, text, .. }| {
                lesson_of(&text).map(|lesson| Lesson { day, url, lesson })
            })
            .collect())
    }

    fn held(&self, since: &str, until: &str) -> Result<Held> {
        let entries = self.span(since, Some(until))?;
        let lessons = entries.iter().filter(|entry| lesson_of(&entry.text).is_some()).count();
        Ok(Held { lessons, unlessoned: entries.len() - lessons })
    }
}

/// Puts `db` in WAL mode, the database's once set. The first switch takes
/// the database whole, and SQLite answers it busy at once, not waiting,
/// where another connection is writing: it is asked again until `BUSY` has
/// passed.
fn wal(db: &Connection) -> Result<()> {
    let started = Instant::now();
    loop {
        match db.query_row("PRAGMA journal_mode = WAL", [], |_| Ok(())) {
            Err(rusqlite::Error::SqliteFailure(failure, _))
                if failure.code == ErrorCode::DatabaseBusy && started.elapsed() < BUSY =>
            {
                thread::sleep(Duration::from_millis(5));
            }
            done => return Ok(done?),
        }
    }
}

fn version(db: &Connection) -> rusqlite::Result<i64> {
    db.query_row("PRAGMA user_version", [], |row| row.get(0))
}

/// Brings `db` to the last version of `migrations`. Two opens that find it
/// behind at once run them in turn: the second finds it current.
fn migrate(db: &mut Connection, migrations: &[&str]) -> Result<()> {
    let known = migrations.len() as i64;
    if version(db)? == known {
        return Ok(());
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found = version(&tx)?;
    if found > known {
        bail!("its schema is version {found}, newer than the {known} this program knows");
    }
    for (index, sql) in migrations.iter().enumerate().skip(found as usize) {
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", index as i64 + 1)?;
    }
    tx.commit()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_database_behind_runs_only_the_versions_it_lacks_and_one_ahead_is_refused() {
        let first = "CREATE TABLE a (x INTEGER)";
        let second = "CREATE TABLE b (x INTEGER); INSERT INTO a VALUES (1)";
        let mut db = Connection::open_in_memory().unwrap();
        migrate(&mut db, &[first]).unwrap();
        migrate(&mut db, &[first, second]).unwrap();
        migrate(&mut db, &[first, second]).unwrap();
        assert_eq!(version(&db).unwrap(), 2);
        let rows: i64 = db.query_row("SELECT COUNT(*) FROM a", [], |row| row.get(0)).unwrap();
        assert_eq!(rows, 1, "the second version ran once");
        let refused = migrate(&mut db, &[first]).unwrap_err();
        assert!(format!("{refused}").contains("newer"), "{refused}");
        assert_eq!(version(&db).unwrap(), 2, "left as it is");
    }
}
