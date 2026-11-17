//! The host's recorded gate runs (docs/adr/0037), kept in the host's
//! database (ADR-0117, `host_db`): the green run of a command on a tree, one
//! row per repo, tree and command; and a red run of an issue's tree, kept
//! until its work account cites it. Each row holds the record's one encoding,
//! the whole message `GreenRun::write_with` makes (or `red_subject`): tree,
//! command, when, how long on which host, the kind of machine and each
//! test's times. No other host reads them, and none is pushed: a host
//! certifies what it lands on itself (docs/adr/0037).
//!
//! A write sweeps the repo's records no landing would believe any more, as
//! a start swept the remote's: a day after the newest.

use afkconvoy_core::gate::{GateCommand, RECORD_TTL, Tree};
use afkconvoy_gate::run::Records;
use anyhow::Result;
use rusqlite::{Connection, params};

use crate::git::Repo;

/// The repo's gate records, as `afkconvoy_gate::run` reads and writes them.
pub(crate) struct OnHost<'a>(pub &'a Repo);

impl Records for OnHost<'_> {
    type Error = anyhow::Error;

    fn read(&self, tree: &Tree, command: &GateCommand) -> Result<Vec<String>> {
        let repo = crate::host_db::repo_key(self.0)?;
        let db = crate::host_db::open()?;
        let mut statement = db.prepare(
            "SELECT record FROM gate_records WHERE repo = ?1 AND tree = ?2 AND command = ?3",
        )?;
        let rows = statement
            .query_map(params![repo, tree.name(), command.text()], |row| row.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// Soft: a record that cannot be written says why, and costs a landing
    /// a run of the gate.
    fn write(
        &self,
        tree: &Tree,
        _head: &str,
        command: &GateCommand,
        message: &str,
    ) -> Result<Option<String>> {
        let at = at_of(message);
        let written = (|| -> Result<()> {
            let repo = crate::host_db::repo_key(self.0)?;
            let mut db = crate::host_db::open()?;
            crate::host_db::write(&mut db, |tx| {
                tx.execute(
                    "INSERT OR REPLACE INTO gate_records (repo, tree, command, at, record)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    params![repo, tree.name(), command.text(), at as i64, message],
                )?;
                sweep(tx, &repo, at)
            })
        })();
        Ok(written.err().map(|error| format!("{error:#}")))
    }

    // Soft: none there is what was wanted.
    fn remove(&self, tree: &Tree, command: &GateCommand) {
        let _ = (|| -> Result<()> {
            let repo = crate::host_db::repo_key(self.0)?;
            let mut db = crate::host_db::open()?;
            crate::host_db::write(&mut db, |tx| {
                tx.execute(
                    "DELETE FROM gate_records WHERE repo = ?1 AND tree = ?2 AND command = ?3",
                    params![repo, tree.name(), command.text()],
                )?;
                Ok(())
            })
        })();
    }
}

/// Keeps the record of a red run of `issue`'s tree, finished at `at`, until
/// the work account cites it. Soft: one that cannot be kept is not cited.
pub(crate) fn keep_red(
    repo: &Repo,
    issue: u64,
    tree: &Tree,
    command: &GateCommand,
    at: u64,
    record: &str,
) {
    let _ = (|| -> Result<()> {
        let key = crate::host_db::repo_key(repo)?;
        let mut db = crate::host_db::open()?;
        crate::host_db::write(&mut db, |tx| {
            tx.execute(
                "INSERT OR REPLACE INTO red_gate_runs (repo, issue, at, tree, command, record)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![key, issue as i64, at as i64, tree.name(), command.text(), record],
            )?;
            sweep(tx, &key, at)
        })
    })();
}

/// Every record of the repo, green and red, oldest first: what a work account
/// cites of its issue's runs.
pub(crate) fn every(repo: &Repo) -> Result<Vec<String>> {
    let key = crate::host_db::repo_key(repo)?;
    let db = crate::host_db::open()?;
    let mut statement = db.prepare(
        "SELECT at, record FROM gate_records WHERE repo = ?1
         UNION ALL SELECT at, record FROM red_gate_runs WHERE repo = ?1 ORDER BY 1",
    )?;
    let rows = statement.query_map(params![key], |row| row.get::<_, String>(1))?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// The green records of `command` in the repo, each with its tree: what the
/// gate gone slow reads of the landings (docs/adr/0024).
pub(crate) fn of_command(repo: &Repo, command: &GateCommand) -> Result<Vec<(String, String)>> {
    let key = crate::host_db::repo_key(repo)?;
    let db = crate::host_db::open()?;
    let mut statement =
        db.prepare("SELECT tree, record FROM gate_records WHERE repo = ?1 AND command = ?2")?;
    let rows = statement.query_map(params![key, command.text()], |row| {
        Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
    })?;
    Ok(rows.collect::<rusqlite::Result<_>>()?)
}

/// When the record `message` says its run finished; `0` where it says none.
fn at_of(message: &str) -> u64 {
    afkconvoy_core::record::read(afkconvoy_core::gate::GATE, message)
        .and_then(|fields| fields.number("at"))
        .unwrap_or(0)
}

/// Takes away the repo's records of runs that finished more than a day
/// before `newest`: none is believed any more.
fn sweep(db: &Connection, repo: &str, newest: u64) -> Result<()> {
    let before = newest.saturating_sub(RECORD_TTL) as i64;
    db.execute("DELETE FROM gate_records WHERE repo = ?1 AND at < ?2", params![repo, before])?;
    db.execute("DELETE FROM red_gate_runs WHERE repo = ?1 AND at < ?2", params![repo, before])?;
    Ok(())
}
