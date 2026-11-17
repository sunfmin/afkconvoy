//! The candidate chain of a repo's target branch, kept by the host that builds
//! it, in the host's database (ADR-0117, `host_db`): one row per repo
//! and target branch. A reader asks the database, which starts no process and
//! asks no remote. Every write names the revision it was made on and takes
//! only where the row is still at it, as the remote record's did: a turn
//! whose edit was made on a chain another turn of this host moved reads it
//! again and edits again. The remote is asked for the base tip alone, and
//! only to build a candidate or to publish.
use afkconvoy_core::candidate::Chain;
use anyhow::Result;
use rusqlite::{Connection, OptionalExtension, params};

use crate::git::Repo;

/// A chain as a look found it: where it was read, which a write is made on.
#[derive(Clone)]
pub(crate) struct Stored {
    pub revision: u64,
    pub chain: Chain,
}

/// The key of this repo's chains: the same as its candidate worktrees', so
/// every checkout and worktree of one repo shares them.
fn key(repo: &Repo) -> Result<String> {
    crate::host_db::repo_key(repo)
}

fn find(db: &Connection, repo: &str, branch: &str) -> Result<Stored> {
    let row = db
        .query_row(
            "SELECT revision, chain FROM chains WHERE repo = ?1 AND branch = ?2",
            params![repo, branch],
            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()?;
    Ok(match row {
        Some((revision, text)) => {
            Stored { revision: revision as u64, chain: serde_json::from_str(&text)? }
        }
        None => Stored { revision: 0, chain: Chain::default() },
    })
}

/// The chain of `branch` as the host holds it now; an empty one where none.
pub(crate) fn read(repo: &Repo, branch: &str) -> Result<Stored> {
    find(&crate::host_db::open()?, &key(repo)?, branch)
}

/// Every chain of this repo on this host, by target branch.
pub(crate) fn all(repo: &Repo) -> Result<Vec<(String, Chain)>> {
    let db = crate::host_db::open()?;
    let mut statement =
        db.prepare("SELECT branch, chain FROM chains WHERE repo = ?1 ORDER BY branch")?;
    let rows = statement.query_map(params![key(repo)?], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    let mut chains = vec![];
    for row in rows {
        let (branch, text) = row?;
        chains.push((branch, serde_json::from_str(&text)?));
    }
    Ok(chains)
}

/// The chain of `branch` inside a write transaction: no other writer of this
/// host takes between its reading and its `write`, and a `Held` dropped
/// without one rolls back.
pub(crate) struct Held {
    db: Connection,
    repo: String,
    branch: String,
    stored: Stored,
    done: bool,
}

/// Opens the write transaction of `branch`'s chain and reads it in it.
pub(crate) fn hold(repo: &Repo, branch: &str) -> Result<Held> {
    let db = crate::host_db::open()?;
    db.execute_batch("BEGIN IMMEDIATE")?;
    let repo = key(repo)?;
    let mut held = Held {
        db,
        repo,
        branch: branch.to_string(),
        stored: Stored { revision: 0, chain: Chain::default() },
        done: false,
    };
    held.stored = find(&held.db, &held.repo, branch)?;
    Ok(held)
}

impl Held {
    /// The revision the chain is at.
    pub(crate) fn revision(&self) -> u64 {
        self.stored.revision
    }

    /// Writes `chain` as the revision after the one held, and commits.
    pub(crate) fn write(mut self, chain: &Chain) -> Result<Stored> {
        let revision = self.stored.revision + 1;
        self.db.execute(
            "INSERT INTO chains (repo, branch, revision, chain) VALUES (?1, ?2, ?3, ?4)
             ON CONFLICT (repo, branch) DO UPDATE SET revision = ?3, chain = ?4",
            params![self.repo, self.branch, revision as i64, serde_json::to_string(chain)?],
        )?;
        self.db.execute_batch("COMMIT")?;
        self.done = true;
        Ok(Stored { revision, chain: chain.clone() })
    }
}

impl Drop for Held {
    fn drop(&mut self) {
        if !self.done {
            let _ = self.db.execute_batch("ROLLBACK");
        }
    }
}

/// Writes `chain` as the revision after `revision`, where the row is still
/// at it → the stored chain, or `None` where another writer moved it first.
pub(crate) fn write(
    repo: &Repo,
    branch: &str,
    revision: u64,
    chain: &Chain,
) -> Result<Option<Stored>> {
    let held = hold(repo, branch)?;
    if held.revision() != revision {
        return Ok(None);
    }
    held.write(chain).map(Some)
}
