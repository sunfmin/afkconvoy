//! The store: one connection to the tracker's database, and every read and
//! write of its issues.

use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::types::Value;
use rusqlite::{
    Connection, ErrorCode, OptionalExtension, Row, Transaction, TransactionBehavior, params,
};

use crate::schema::{MIGRATIONS, migrate};
use crate::{Error, Result};

/// How long a writer waits for another's transaction to end before it fails.
const BUSY: Duration = Duration::from_secs(60);

/// Whether an issue is open or closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Open,
    Closed,
}

/// Which issues a listing holds, by their state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Which {
    #[default]
    Open,
    Closed,
    All,
}

/// One issue, with its labels. Times are epoch seconds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub number: u64,
    /// The repo it is of, `<owner>/<repo>`.
    pub repo: String,
    pub title: String,
    pub body: String,
    /// Its labels, by name, in order.
    pub labels: Vec<String>,
    pub created: u64,
    /// When a write last changed it, its comments, labels or blockers.
    pub updated: u64,
    /// When its body was last edited, where it ever was.
    pub body_edited: Option<u64>,
    /// When it closed, where it is closed.
    pub closed: Option<u64>,
}

impl Issue {
    pub fn state(&self) -> State {
        if self.closed.is_some() { State::Closed } else { State::Open }
    }
}

/// One comment on an issue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Comment {
    pub id: u64,
    /// The number of the issue it is on.
    pub issue: u64,
    pub body: String,
    pub created: u64,
    /// When it was last edited, where it ever was.
    pub edited: Option<u64>,
}

/// One issue another is blocked by, as it is now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blocker {
    pub number: u64,
    pub repo: String,
    /// When it closed, where it is closed.
    pub closed: Option<u64>,
    pub labels: Vec<String>,
}

impl Blocker {
    pub fn state(&self) -> State {
        if self.closed.is_some() { State::Closed } else { State::Open }
    }
}

/// One issue with its comments, oldest first, and its blockers, lowest
/// number first, from one read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct View {
    pub issue: Issue,
    pub comments: Vec<Comment>,
    pub blockers: Vec<Blocker>,
}

/// Which issues a listing holds: each it names, or every one where it names
/// none.
#[derive(Debug, Clone, Copy, Default)]
pub struct Filter<'a> {
    /// Of this repo alone.
    pub repo: Option<&'a str>,
    pub state: Which,
    /// Carrying every one of these labels.
    pub labels: &'a [&'a str],
    /// Changed at or after this time.
    pub updated_since: Option<u64>,
}

/// One connection to a tracker's database. Each process, and each thread
/// that writes on its own, opens its own.
pub struct Store {
    db: Connection,
}

impl Store {
    /// The database at `path`, made where there is none (its directory must
    /// exist) and brought to the schema this crate knows.
    pub fn open(path: &Path) -> Result<Store> {
        let mut db = Connection::open(path)?;
        db.busy_timeout(BUSY)?;
        wal(&db)?;
        db.pragma_update(None, "foreign_keys", true)?;
        migrate(&mut db, MIGRATIONS)?;
        Ok(Store { db })
    }

    /// Opens an issue of `repo`, `<owner>/<repo>`, at `at` → its number.
    pub fn create(
        &mut self,
        repo: &str,
        title: &str,
        body: &str,
        labels: &[&str],
        at: u64,
    ) -> Result<u64> {
        if !named_repo(repo) {
            return Err(Error::BadRepo(repo.to_string()));
        }
        let tx = self.write()?;
        tx.execute(
            "INSERT INTO issues (repo, title, body, created, updated) VALUES (?1, ?2, ?3, ?4, ?4)",
            params![repo, title, body, at],
        )?;
        let number = tx.last_insert_rowid() as u64;
        for label in labels {
            tx.execute(
                "INSERT OR IGNORE INTO labels (issue, name, added) VALUES (?1, ?2, ?3)",
                params![number, label, at],
            )?;
        }
        tx.commit()?;
        Ok(number)
    }

    /// Issue `number` with its comments and blockers.
    pub fn view(&mut self, number: u64) -> Result<View> {
        let tx = self.db.transaction()?;
        let issue = issue(&tx, number)?;
        view(&tx, issue)
    }

    /// The issues `filter` holds, lowest number first.
    pub fn list(&mut self, filter: &Filter<'_>) -> Result<Vec<Issue>> {
        let tx = self.db.transaction()?;
        listed(&tx, filter)
    }

    /// The issues `filter` holds, each with its comments and blockers, lowest
    /// number first, from one read.
    pub fn views(&mut self, filter: &Filter<'_>) -> Result<Vec<View>> {
        let tx = self.db.transaction()?;
        listed(&tx, filter)?.into_iter().map(|issue| view(&tx, issue)).collect()
    }

    /// Comments `body` on issue `number` at `at` → the comment's id.
    pub fn comment(&mut self, number: u64, body: &str, at: u64) -> Result<u64> {
        let tx = self.write()?;
        touch(&tx, number, at)?;
        tx.execute(
            "INSERT INTO comments (issue, body, created) VALUES (?1, ?2, ?3)",
            params![number, body, at],
        )?;
        let id = tx.last_insert_rowid() as u64;
        tx.commit()?;
        Ok(id)
    }

    /// Replaces the body of comment `id`, at `at`.
    pub fn edit_comment(&mut self, id: u64, body: &str, at: u64) -> Result<()> {
        let tx = self.write()?;
        let number: u64 = tx
            .query_row("SELECT issue FROM comments WHERE id = ?1", [id], |row| row.get(0))
            .optional()?
            .ok_or(Error::NoSuchComment(id))?;
        tx.execute(
            "UPDATE comments SET body = ?2, edited = ?3 WHERE id = ?1",
            params![id, body, at],
        )?;
        touch(&tx, number, at)?;
        tx.commit()?;
        Ok(())
    }

    /// Replaces the body of issue `number`, at `at`.
    pub fn edit_body(&mut self, number: u64, body: &str, at: u64) -> Result<()> {
        let tx = self.write()?;
        touch(&tx, number, at)?;
        tx.execute(
            "UPDATE issues SET body = ?2, body_edited = ?3 WHERE number = ?1",
            params![number, body, at],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Replaces the title of issue `number`, at `at`.
    pub fn edit_title(&mut self, number: u64, title: &str, at: u64) -> Result<()> {
        let tx = self.write()?;
        touch(&tx, number, at)?;
        tx.execute("UPDATE issues SET title = ?2 WHERE number = ?1", params![number, title])?;
        tx.commit()?;
        Ok(())
    }

    /// Puts `labels` on issue `number` at `at`; one it carries already is
    /// left, and keeps when it was put on.
    pub fn add_labels(&mut self, number: u64, labels: &[&str], at: u64) -> Result<()> {
        self.labelled(number, labels, at, |tx, label| {
            tx.execute(
                "INSERT OR IGNORE INTO labels (issue, name, added) VALUES (?1, ?2, ?3)",
                params![number, label, at],
            )
        })
    }

    /// Takes `labels` off issue `number`; one it does not carry is no matter.
    pub fn remove_labels(&mut self, number: u64, labels: &[&str], at: u64) -> Result<()> {
        self.labelled(number, labels, at, |tx, label| {
            tx.execute("DELETE FROM labels WHERE issue = ?1 AND name = ?2", params![number, label])
        })
    }

    /// Makes `change` of each of `labels` on issue `number`, at `at` where
    /// one changed anything.
    fn labelled(
        &mut self,
        number: u64,
        labels: &[&str],
        at: u64,
        change: impl Fn(&Transaction<'_>, &str) -> rusqlite::Result<usize>,
    ) -> Result<()> {
        let tx = self.write()?;
        exists(&tx, number)?;
        let mut changed = 0;
        for label in labels {
            changed += change(&tx, label)?;
        }
        if changed > 0 {
            touch(&tx, number, at)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Closes issue `number` at `at`. One closed already keeps its time.
    pub fn close(&mut self, number: u64, at: u64) -> Result<()> {
        self.stated(number, Some(at), at)
    }

    /// Opens closed issue `number` again at `at`. One open is left.
    pub fn reopen(&mut self, number: u64, at: u64) -> Result<()> {
        self.stated(number, None, at)
    }

    /// Sets issue `number`'s closing time to `closed` where it is not of
    /// that state already.
    fn stated(&mut self, number: u64, closed: Option<u64>, at: u64) -> Result<()> {
        let tx = self.write()?;
        exists(&tx, number)?;
        let changed = tx.execute(
            "UPDATE issues SET closed = ?2 WHERE number = ?1 AND (closed IS NULL) <> (?2 IS NULL)",
            params![number, closed],
        )?;
        if changed > 0 {
            touch(&tx, number, at)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Records issue `number` as blocked by issue `blocker`, at `at`. Both
    /// are of this database; an edge that would make a cycle is refused.
    pub fn add_blocker(&mut self, number: u64, blocker: u64, at: u64) -> Result<()> {
        if number == blocker {
            return Err(Error::BlocksItself(number));
        }
        let tx = self.write()?;
        exists(&tx, number)?;
        exists(&tx, blocker)?;
        // A cycle where `number` already blocks `blocker`, through any edges.
        let cycle: bool = tx.query_row(
            "WITH RECURSIVE below (n) AS (
                 SELECT ?1
                 UNION SELECT blockers.blocker FROM blockers JOIN below ON blockers.issue = below.n
             )
             SELECT EXISTS (SELECT 1 FROM below WHERE n = ?2)",
            params![blocker, number],
            |row| row.get(0),
        )?;
        if cycle {
            return Err(Error::Cycle { issue: number, blocker });
        }
        let added = tx.execute(
            "INSERT OR IGNORE INTO blockers (issue, blocker) VALUES (?1, ?2)",
            params![number, blocker],
        )?;
        if added > 0 {
            touch(&tx, number, at)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Records issue `number` as no longer blocked by issue `blocker`.
    pub fn remove_blocker(&mut self, number: u64, blocker: u64, at: u64) -> Result<()> {
        let tx = self.write()?;
        exists(&tx, number)?;
        exists(&tx, blocker)?;
        let removed = tx.execute(
            "DELETE FROM blockers WHERE issue = ?1 AND blocker = ?2",
            params![number, blocker],
        )?;
        if removed > 0 {
            touch(&tx, number, at)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// The issues `number` is blocked by, open or closed, lowest first.
    pub fn blockers(&mut self, number: u64) -> Result<Vec<Blocker>> {
        let tx = self.db.transaction()?;
        exists(&tx, number)?;
        blockers(&tx, number)
    }

    /// Each label issue `number` carries, in order, with when it was put on
    /// — none where that was not kept.
    pub fn labels_added(&mut self, number: u64) -> Result<Vec<(String, Option<u64>)>> {
        let tx = self.db.transaction()?;
        exists(&tx, number)?;
        let mut statement =
            tx.prepare("SELECT name, added FROM labels WHERE issue = ?1 ORDER BY name")?;
        let rows = statement.query_map([number], |row| Ok((row.get(0)?, row.get(1)?)))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Every label some issue carries, of `repo` alone where it is named,
    /// each once, in order.
    pub fn labels(&mut self, repo: Option<&str>) -> Result<Vec<String>> {
        let mut statement = self.db.prepare(
            "SELECT DISTINCT labels.name FROM labels JOIN issues ON issues.number = labels.issue
             WHERE ?1 IS NULL OR issues.repo = ?1 ORDER BY labels.name",
        )?;
        let names = statement.query_map([repo], |row| row.get(0))?;
        Ok(names.collect::<rusqlite::Result<_>>()?)
    }

    /// A write: it holds the write lock from its start, so another writer
    /// waits for it rather than finding what it read changed.
    fn write(&mut self) -> Result<Transaction<'_>> {
        Ok(self.db.transaction_with_behavior(TransactionBehavior::Immediate)?)
    }
}

/// Puts `db` in WAL mode, where a read runs beside a write; the mode is the
/// database's once set. The first switch takes the database whole, and
/// SQLite answers it busy at once, not waiting, where another connection is
/// writing its schema: it is asked again until `BUSY` has passed.
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

/// `<owner>/<repo>`: two names, neither empty, without space or another `/`.
fn named_repo(repo: &str) -> bool {
    let named = |part: &str| !part.is_empty() && !part.contains(char::is_whitespace);
    repo.split_once('/')
        .is_some_and(|(owner, name)| named(owner) && named(name) && !name.contains('/'))
}

/// `NoSuchIssue` where the database has no issue `number`.
fn exists(tx: &Transaction<'_>, number: u64) -> Result<()> {
    let found =
        tx.query_row("SELECT 1 FROM issues WHERE number = ?1", [number], |_| Ok(())).optional()?;
    found.ok_or(Error::NoSuchIssue(number))
}

/// Records issue `number` as changed at `at`: `NoSuchIssue` where there is
/// none.
fn touch(tx: &Transaction<'_>, number: u64, at: u64) -> Result<()> {
    match tx.execute("UPDATE issues SET updated = ?2 WHERE number = ?1", params![number, at])? {
        0 => Err(Error::NoSuchIssue(number)),
        _ => Ok(()),
    }
}

const ISSUE_COLUMNS: &str = "number, repo, title, body, created, updated, body_edited, closed";

fn issue_row(row: &Row<'_>) -> rusqlite::Result<Issue> {
    Ok(Issue {
        number: row.get(0)?,
        repo: row.get(1)?,
        title: row.get(2)?,
        body: row.get(3)?,
        labels: vec![],
        created: row.get(4)?,
        updated: row.get(5)?,
        body_edited: row.get(6)?,
        closed: row.get(7)?,
    })
}

fn issue(tx: &Transaction<'_>, number: u64) -> Result<Issue> {
    let sql = format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE number = ?1");
    let issue = tx.query_row(&sql, [number], issue_row).optional()?;
    let mut issue = issue.ok_or(Error::NoSuchIssue(number))?;
    issue.labels = labels_of(tx, number)?;
    Ok(issue)
}

fn labels_of(tx: &Transaction<'_>, number: u64) -> Result<Vec<String>> {
    let mut statement =
        tx.prepare_cached("SELECT name FROM labels WHERE issue = ?1 ORDER BY name")?;
    let names = statement.query_map([number], |row| row.get(0))?;
    Ok(names.collect::<rusqlite::Result<_>>()?)
}

fn listed(tx: &Transaction<'_>, filter: &Filter<'_>) -> Result<Vec<Issue>> {
    let mut sql = format!("SELECT {ISSUE_COLUMNS} FROM issues WHERE 1");
    let mut values: Vec<Value> = vec![];
    if let Some(repo) = filter.repo {
        values.push(Value::Text(repo.to_string()));
        sql += &format!(" AND repo = ?{}", values.len());
    }
    match filter.state {
        Which::Open => sql += " AND closed IS NULL",
        Which::Closed => sql += " AND closed IS NOT NULL",
        Which::All => {}
    }
    for label in filter.labels {
        values.push(Value::Text(label.to_string()));
        sql += &format!(
            " AND EXISTS (SELECT 1 FROM labels WHERE issue = number AND name = ?{})",
            values.len()
        );
    }
    if let Some(since) = filter.updated_since {
        values.push(Value::Integer(i64::try_from(since).unwrap_or(i64::MAX)));
        sql += &format!(" AND updated >= ?{}", values.len());
    }
    sql += " ORDER BY number";
    let mut statement = tx.prepare(&sql)?;
    let rows = statement.query_map(rusqlite::params_from_iter(values), issue_row)?;
    let mut issues = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for issue in &mut issues {
        issue.labels = labels_of(tx, issue.number)?;
    }
    Ok(issues)
}

fn view(tx: &Transaction<'_>, issue: Issue) -> Result<View> {
    let mut statement = tx.prepare_cached(
        "SELECT id, issue, body, created, edited FROM comments WHERE issue = ?1 ORDER BY id",
    )?;
    let comments = statement.query_map([issue.number], |row| {
        Ok(Comment {
            id: row.get(0)?,
            issue: row.get(1)?,
            body: row.get(2)?,
            created: row.get(3)?,
            edited: row.get(4)?,
        })
    })?;
    let comments = comments.collect::<rusqlite::Result<_>>()?;
    let blockers = blockers(tx, issue.number)?;
    Ok(View { issue, comments, blockers })
}

fn blockers(tx: &Transaction<'_>, number: u64) -> Result<Vec<Blocker>> {
    let mut statement = tx.prepare_cached(
        "SELECT issues.number, issues.repo, issues.closed FROM blockers
         JOIN issues ON issues.number = blockers.blocker
         WHERE blockers.issue = ?1 ORDER BY issues.number",
    )?;
    let rows = statement.query_map([number], |row| {
        Ok(Blocker { number: row.get(0)?, repo: row.get(1)?, closed: row.get(2)?, labels: vec![] })
    })?;
    let mut blockers = rows.collect::<rusqlite::Result<Vec<_>>>()?;
    for blocker in &mut blockers {
        blocker.labels = labels_of(tx, blocker.number)?;
    }
    Ok(blockers)
}

#[cfg(test)]
mod tests {
    use super::named_repo;

    #[test]
    fn a_repo_is_an_owner_and_a_name() {
        assert!(named_repo("acme/widgets"));
        assert!(named_repo("acme/widgets.rs"));
        for bad in ["", "acme", "/widgets", "acme/", "acme/widgets/x", "ac me/widgets", "a/b c"] {
            assert!(!named_repo(bad), "{bad:?}");
        }
    }
}
