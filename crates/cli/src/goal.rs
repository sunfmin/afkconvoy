//! A worker's goal, where its harness keeps one (sunfmin/control#544): its
//! brief is handed as that goal, and the harness carries the worker on after
//! each turn its model ends short of it. What the store is, its query and
//! what ends a goal are the harness's (`Harness::goal_store`); here they are
//! only read and typed.

use std::path::Path;
use std::time::Duration;

use afkconvoy_core::harness::{Goal, Harness};
use anyhow::{Context, Result, bail};

use crate::orca::Worktree;

/// How long a read waits on a goal store its harness is writing.
const BUSY: Duration = Duration::from_millis(250);

/// The goal of the worker in `worktree` and the harness it runs on. `None`:
/// its harness keeps no goal, or its store cannot be read — which is not a
/// goal that stopped.
pub(crate) fn of(worktree: &Worktree) -> Option<(Harness, Goal)> {
    let harness = worktree.recorded_harness().ok().flatten()?;
    let goal = read(&worktree.path, harness)?;
    Some((harness, goal))
}

/// The goal `harness` keeps for the worker in `cwd`.
fn read(cwd: &Path, harness: Harness) -> Option<Goal> {
    let store = harness.goal_store()?;
    let home = crate::session::kept_home(cwd, harness)?;
    let file = home.join(store.file);
    if !file.exists() {
        return Some(Goal::Unset);
    }
    harness.goal(&answer(&file, store.query)?)
}

/// Whether the worker in `worktree`, running on `harness`, has been
/// continued on its goal by its harness since `since`.
pub(crate) fn continued(worktree: &Worktree, harness: Harness, since: u64) -> Result<bool> {
    crate::session::goal_continued(&worktree.path, harness, since)
}

/// Ends the goal of the worker in `worktree`: its current turn finishes, and
/// its harness begins no other on it.
pub(crate) fn end(worktree: &Worktree, harness: Harness) -> Result<()> {
    let key = harness.goal_end().context("its harness keeps no goal")?;
    let Some(terminal) = worktree.terminal()? else {
        bail!("no live terminal in {} to end its goal", worktree.path.display());
    };
    worktree.press_key(&terminal, key)
}

/// How the goal of the worker started in `cwd` at `start` went, before the
/// next worker's `before` (sunfmin/control#548): its session record, and its
/// row in the goal store of the home that record is under. `None`: its
/// harness keeps no goal, or no session record of it is left.
pub(crate) fn account(
    cwd: &Path,
    harness: Harness,
    start: u64,
    before: Option<u64>,
    verdict_at: Option<u64>,
) -> Option<afkconvoy_core::account::WorkerGoal> {
    let (home, file, said) = crate::session::goal_session(cwd, harness, start, before).ok()??;
    let row = row(&home, harness, said.thread.as_deref());
    let session = file.display().to_string();
    Some(afkconvoy_core::account::WorkerGoal::of(&said, &session, row, verdict_at))
}

/// The row of the session `thread` in the goal store under `home`. `None`:
/// the store or its row cannot be read.
fn row(
    home: &Path,
    harness: Harness,
    thread: Option<&str>,
) -> Option<Option<afkconvoy_core::harness::GoalRow>> {
    let file = home.join(harness.goal_store()?.file);
    if !file.exists() {
        return None;
    }
    let query = harness.goal_row_query(thread?)?;
    harness.goal_row(&answer(&file, &query)?)
}

/// The answer to `query` of the store `file`, opened read-only, as
/// `sqlite3 -readonly` prints it, which the harness reads: a line a row,
/// its columns `|`-separated, a NULL empty. `None`: the store cannot be
/// opened or read, or the query fails.
fn answer(file: &Path, query: &str) -> Option<String> {
    use rusqlite::{Connection, OpenFlags, types::ValueRef};
    let flags = OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX;
    let db = Connection::open_with_flags(file, flags).ok()?;
    db.busy_timeout(BUSY).ok()?;
    let mut statement = db.prepare(query).ok()?;
    let columns = statement.column_count();
    let mut rows = statement.query([]).ok()?;
    let mut text = String::new();
    while let Some(row) = rows.next().ok()? {
        for column in 0..columns {
            if column > 0 {
                text.push('|');
            }
            match row.get_ref(column).ok()? {
                ValueRef::Null => {}
                ValueRef::Integer(value) => text.push_str(&value.to_string()),
                ValueRef::Real(value) => text.push_str(&value.to_string()),
                ValueRef::Text(value) | ValueRef::Blob(value) => {
                    text.push_str(&String::from_utf8_lossy(value));
                }
            }
        }
        text.push('\n');
    }
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const AT: u64 = 1_793_677_856;
    const SCHEMA: &str = "create table thread_goals (thread_id text primary key not null, \
        goal_id text not null, objective text not null, status text not null, \
        token_budget integer, tokens_used integer not null default 0, \
        time_used_seconds integer not null default 0, created_at_ms integer not null, \
        updated_at_ms integer not null);";

    /// A worktree whose noted Codex home holds a goal session record, its
    /// goal turns begun at `turns`, and a goal store holding `row`: a
    /// status, tokens used and budget, or none, as `/goal clear` leaves it.
    fn worker(turns: &[u64], row: Option<(&str, u64)>) -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().join("work");
        let gitdir = dir.path().join("git");
        let home = dir.path().join("codex");
        std::fs::create_dir_all(&cwd).unwrap();
        std::fs::create_dir_all(&gitdir).unwrap();
        std::fs::create_dir_all(home.join("sessions/2026/11/03")).unwrap();
        std::fs::write(cwd.join(".git"), format!("gitdir: {}", gitdir.display())).unwrap();
        std::fs::write(gitdir.join("convoy-codex-home"), home.display().to_string()).unwrap();
        let cwd_text = cwd.display().to_string();
        let mut rollout = Harness::Codex.session_files(&cwd_text, true, AT)[0].body.clone();
        let objective = afkconvoy_core::prompt::goal_objective("/b.md");
        rollout.push_str(&format!(
            "{}\n",
            json!({"timestamp": "2026-11-03T03:50:56.750Z", "type": "event_msg", "payload": {
                "type": "thread_goal_updated", "goal": {"objective": objective, "status": "active"}}})
        ));
        for at in turns {
            rollout.push_str(&format!(
                "{}\n",
                json!({"timestamp": "2026-11-03T03:51:05.379Z", "type": "event_msg", "payload": {
                    "type": "task_started", "started_at": at,
                    "turn_attribution": {"turn_trigger": "goal"}}})
            ));
        }
        std::fs::write(home.join("sessions/2026/11/03/rollout-t.jsonl"), rollout).unwrap();
        let mut sql = SCHEMA.to_string();
        if let Some((status, used)) = row {
            sql.push_str(&format!(
                "insert into thread_goals values ('afkconvoy', 'g', 'o', '{status}', \
                 1000000, {used}, 0, 0, 0);"
            ));
        }
        rusqlite::Connection::open(home.join("goals_1.sqlite"))
            .unwrap()
            .execute_batch(&sql)
            .unwrap();
        (dir, cwd)
    }

    /// A goal store at `file` made by `sql`.
    fn store(file: &Path, sql: &str) {
        rusqlite::Connection::open(file).unwrap().execute_batch(sql).unwrap();
    }

    #[test]
    fn a_goal_store_answers_as_sqlite3_prints_it() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("goals_1.sqlite");
        store(
            &file,
            &format!(
                "{SCHEMA} insert into thread_goals values ('a', 'g', 'o', 'active', null, 7, 0, 0, 2); \
                 insert into thread_goals values ('b', 'g', 'o', 'complete', 1000000, 9, 0, 0, 1);"
            ),
        );
        let codex = Harness::Codex;
        let store = codex.goal_store().unwrap();
        assert_eq!(answer(&file, store.query).as_deref(), Some("active\n"));
        let both = "select status, tokens_used, token_budget from thread_goals order by thread_id";
        assert_eq!(answer(&file, both).as_deref(), Some("active|7|\ncomplete|9|1000000\n"));
        assert_eq!(answer(&file, &codex.goal_row_query("c").unwrap()).as_deref(), Some(""));
        assert_eq!(answer(&file, "select nothing from nowhere"), None, "a failing query");

        let read = |thread: &str| codex.goal_row(&answer(&file, &codex.goal_row_query(thread)?)?);
        let row = read("a").unwrap().unwrap();
        assert_eq!((row.status, row.tokens_used, row.token_budget), (Goal::Active, 7, None));
        let row = read("b").unwrap().unwrap();
        assert_eq!(row.token_budget, Some(1_000_000));
        assert_eq!(read("c"), Some(None), "a session with no row");

        let missing = dir.path().join("missing.sqlite");
        assert_eq!(answer(&missing, store.query), None);
        assert!(!missing.exists(), "a read creates no store");
        let unreadable = dir.path().join("not-a-db.sqlite");
        std::fs::write(&unreadable, "not a database").unwrap();
        assert_eq!(answer(&unreadable, store.query), None);
    }

    #[test]
    fn a_store_its_harness_is_writing_is_not_waited_on() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("goals_1.sqlite");
        store(
            &file,
            &format!(
                "{SCHEMA} insert into thread_goals values ('a', 'g', 'o', 'active', null, 0, 0, 0, 0);"
            ),
        );
        let writer = rusqlite::Connection::open(&file).unwrap();
        writer
            .execute_batch("begin exclusive; update thread_goals set status = 'paused';")
            .unwrap();
        let started = std::time::Instant::now();
        assert_eq!(answer(&file, Harness::Codex.goal_store().unwrap().query), None);
        assert!(started.elapsed() < Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn a_codex_worker_s_account_records_how_its_goal_went() {
        let (_dir, cwd) = worker(&[AT, AT + 60], Some(("complete", 24_959)));
        let goal = account(&cwd, Harness::Codex, AT, None, Some(AT + 100)).unwrap();
        assert!(goal.set);
        assert_eq!(goal.objective, Some(afkconvoy_core::prompt::goal_objective("/b.md")));
        assert_eq!(goal.goal_turns, Some(2));
        assert_eq!(goal.goal_turns_after_verdict, Some(0));
        assert_eq!(goal.status.as_deref(), Some("complete"));
        assert_eq!((goal.tokens_used, goal.token_budget), (Some(24_959), Some(1_000_000)));
        assert!(goal.session.as_deref().unwrap().ends_with("sessions/2026/11/03/rollout-t.jsonl"));
        assert_eq!(goal.misbehaved(), None);

        let (_dir, cwd) = worker(&[AT, AT + 60], Some(("budget_limited", 1_000_212)));
        let goal = account(&cwd, Harness::Codex, AT, None, None).unwrap();
        assert_eq!(goal.status.as_deref(), Some("budget_limited"));
        assert!(goal.misbehaved().unwrap().contains("budget_limited"), "{goal:?}");

        let (_dir, cwd) = worker(&[AT, AT + 200, AT + 260], None);
        let goal = account(&cwd, Harness::Codex, AT, None, Some(AT + 100)).unwrap();
        assert_eq!(goal.status.as_deref(), Some(afkconvoy_core::account::CLEARED));
        assert_eq!(goal.tokens_used, None);
        assert_eq!(goal.goal_turns_after_verdict, Some(2));
        assert!(goal.misbehaved().unwrap().contains("2 goal turns after its verdict"));

        assert_eq!(
            account(&cwd, Harness::ClaudeCode, AT, None, Some(AT + 100)),
            None,
            "a worker of another harness carries no goal"
        );
        assert_eq!(
            account(&cwd, Harness::Codex, AT + 3600, None, None),
            None,
            "a session begun before the worker is not its"
        );
    }
}
