//! What can go wrong: what a caller asked of an issue, a comment or an edge
//! that the tracker refuses, told apart from the database failing.

use std::fmt;

/// An answer of the tracker, or why it gave none.
pub type Result<T> = std::result::Result<T, Error>;

/// Why the tracker did not do what it was asked.
#[derive(Debug)]
pub enum Error {
    /// The database has no issue of this number.
    NoSuchIssue(u64),
    /// The database has no comment of this id.
    NoSuchComment(u64),
    /// An issue was to be opened for a repo not named `<owner>/<repo>`.
    BadRepo(String),
    /// An issue was to be blocked by itself.
    BlocksItself(u64),
    /// `issue` was to be blocked by `blocker`, which is already blocked by
    /// `issue`, directly or through others.
    Cycle { issue: u64, blocker: u64 },
    /// The database's schema is of a version newer than this crate knows.
    Newer { found: i64, known: i64 },
    /// The database failed: it could not be opened, read or written.
    Storage(rusqlite::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::NoSuchIssue(number) => write!(f, "no issue #{number}"),
            Error::NoSuchComment(id) => write!(f, "no comment {id}"),
            Error::BadRepo(repo) => write!(f, "{repo:?} is not a repo named <owner>/<repo>"),
            Error::BlocksItself(number) => write!(f, "issue #{number} cannot block itself"),
            Error::Cycle { issue, blocker } => {
                write!(f, "issue #{blocker} is already blocked by #{issue}: a cycle")
            }
            Error::Newer { found, known } => write!(
                f,
                "the tracker's database is at version {found}, newer than this one's {known}; \
                 install a newer afkconvoy"
            ),
            Error::Storage(error) => write!(f, "the tracker's database failed: {error}"),
        }
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Error::Storage(error) => Some(error),
            _ => None,
        }
    }
}

impl From<rusqlite::Error> for Error {
    fn from(error: rusqlite::Error) -> Error {
        Error::Storage(error)
    }
}
