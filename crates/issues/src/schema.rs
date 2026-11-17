//! The database's schema. It is versioned (`PRAGMA user_version`) and each
//! version is one entry of `MIGRATIONS`, run once, in order, in one
//! transaction, by the first open that finds the database behind. A change
//! of the schema is a new entry, never an edit of one.

use rusqlite::{Connection, TransactionBehavior};

use crate::{Error, Result};

/// Version `n` is `MIGRATIONS[n - 1]`.
pub(crate) const MIGRATIONS: &[&str] = &[
    // An issue is open while it has no closing time. `AUTOINCREMENT`: a
    // number is never given again, not even one of a row gone.
    "CREATE TABLE issues (
        number INTEGER PRIMARY KEY AUTOINCREMENT,
        repo TEXT NOT NULL,
        title TEXT NOT NULL,
        body TEXT NOT NULL,
        created INTEGER NOT NULL,
        updated INTEGER NOT NULL,
        body_edited INTEGER,
        closed INTEGER
    );
    CREATE INDEX issues_by_repo ON issues (repo, updated);
    CREATE INDEX issues_by_update ON issues (updated);
    CREATE TABLE labels (
        issue INTEGER NOT NULL REFERENCES issues (number),
        name TEXT NOT NULL,
        PRIMARY KEY (issue, name)
    ) WITHOUT ROWID;
    CREATE INDEX labels_by_name ON labels (name, issue);
    CREATE TABLE comments (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        issue INTEGER NOT NULL REFERENCES issues (number),
        body TEXT NOT NULL,
        created INTEGER NOT NULL,
        edited INTEGER
    );
    CREATE INDEX comments_by_issue ON comments (issue, id);
    CREATE TABLE blockers (
        issue INTEGER NOT NULL REFERENCES issues (number),
        blocker INTEGER NOT NULL REFERENCES issues (number),
        PRIMARY KEY (issue, blocker),
        CHECK (issue <> blocker)
    ) WITHOUT ROWID;
    CREATE INDEX blockers_by_blocker ON blockers (blocker, issue);",
    // When each label an issue carries was put on it: none of one put on
    // before this was kept.
    "ALTER TABLE labels ADD COLUMN added INTEGER;",
];

/// The schema version of `db`.
pub(crate) fn version(db: &Connection) -> Result<i64> {
    Ok(db.query_row("PRAGMA user_version", [], |row| row.get(0))?)
}

/// Brings `db` to the last version of `migrations`. Two opens that find it
/// behind at once run them in turn: the second finds it current.
pub(crate) fn migrate(db: &mut Connection, migrations: &[&str]) -> Result<()> {
    let known = migrations.len() as i64;
    if version(db)? == known {
        return Ok(());
    }
    let tx = db.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let found = version(&tx)?;
    if found > known {
        return Err(Error::Newer { found, known });
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

    fn tables(db: &Connection) -> Vec<String> {
        let mut statement = db
            .prepare("SELECT name FROM sqlite_master WHERE type = 'table' ORDER BY name")
            .unwrap();
        statement.query_map([], |row| row.get(0)).unwrap().map(|name| name.unwrap()).collect()
    }

    #[test]
    fn a_database_behind_runs_only_the_versions_it_lacks() {
        let first = "CREATE TABLE a (x INTEGER)";
        let second = "CREATE TABLE b (x INTEGER); INSERT INTO a VALUES (1)";
        let mut db = Connection::open_in_memory().unwrap();
        migrate(&mut db, &[first]).unwrap();
        assert_eq!(version(&db).unwrap(), 1);
        migrate(&mut db, &[first, second]).unwrap();
        migrate(&mut db, &[first, second]).unwrap();
        assert_eq!(version(&db).unwrap(), 2);
        assert_eq!(tables(&db), ["a", "b"]);
        let rows: i64 = db.query_row("SELECT COUNT(*) FROM a", [], |row| row.get(0)).unwrap();
        assert_eq!(rows, 1, "the second version ran once");
    }

    #[test]
    fn a_database_ahead_is_refused_and_left_as_it_is() {
        let mut db = Connection::open_in_memory().unwrap();
        migrate(&mut db, MIGRATIONS).unwrap();
        db.pragma_update(None, "user_version", MIGRATIONS.len() as i64 + 1).unwrap();
        let refused = migrate(&mut db, MIGRATIONS).unwrap_err();
        let known = MIGRATIONS.len() as i64;
        assert!(
            matches!(refused, Error::Newer { found, known: k } if found == known + 1 && k == known),
            "{refused}"
        );
        assert_eq!(version(&db).unwrap(), known + 1);
    }

    #[test]
    fn a_failing_version_leaves_the_database_at_the_one_before() {
        let mut db = Connection::open_in_memory().unwrap();
        let bad = ["CREATE TABLE a (x INTEGER)", "CREATE TABLE b (x INTEGER); NOT SQL"];
        assert!(matches!(migrate(&mut db, &bad), Err(Error::Storage(_))));
        assert_eq!(version(&db).unwrap(), 0);
        assert!(tables(&db).is_empty());
    }
}
