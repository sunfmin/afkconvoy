//! A host's **local tracker** (docs/adr/0091): its issues, their labels,
//! comments and blockers, kept in one SQLite database on the host.
//!
//! Every process of the host — the host runner, a worker's commands, a human
//! — opens the database itself and reads and writes it at once with the
//! others. The database is in WAL mode, so a read never waits on a write;
//! a writer waits for another's transaction to end rather than fail; and
//! every write is one transaction, which begins holding the write lock, so
//! what it read is what it writes over.
//!
//! An issue is of a repo, `<owner>/<repo>`, and has a number, counted from 1
//! in each database and never given again. A comment has an id, counted the
//! same way across every issue. A blocker is an edge between two issues of
//! the one database, and no edge is written that would make a cycle.
//!
//! No clock is read: each write is told when it is made, in epoch seconds.
//! What the crate holds is text, labels and edges alone; what a comment or a
//! label means is its caller's.

mod error;
mod schema;
mod store;

pub use error::{Error, Result};
pub use store::{Blocker, Comment, Filter, Issue, State, Store, View, Which};
