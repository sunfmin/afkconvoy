//! The host's one afkconvoy database (ADR-0117): `host.db` under the
//! cache directory, where the host-local state lives that no other host
//! reads. Its candidate chains are the first (`chain_store`); the rest of it
//! moves in as later issues take it from files. Its schema is versioned
//! (`PRAGMA user_version`) and each version is one entry of `MIGRATIONS`,
//! run once, in order, in one transaction, by the first open that finds the
//! database behind. A change of the schema is a new entry, never an edit of
//! one.
use anyhow::{Context, Result, bail};
use rusqlite::Connection;
use std::time::Duration;

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
];

/// The database, migrated to the version this afkconvoy knows.
pub(crate) fn open() -> Result<Connection> {
    let directory = crate::copies::cache()?;
    std::fs::create_dir_all(&directory)?;
    let path = directory.join("host.db");
    let mut db = Connection::open(&path)
        .with_context(|| format!("cannot open the host database {}", path.display()))?;
    db.busy_timeout(BUSY)?;
    migrate(&mut db)?;
    Ok(db)
}

fn migrate(db: &mut Connection) -> Result<()> {
    let known = MIGRATIONS.len() as i64;
    let at = |db: &Connection| db.query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0));
    if at(db)? == known {
        return Ok(());
    }
    // Immediate: two opens that find it behind run the migrations in turn.
    let tx = db.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
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
mod tests {
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
}
