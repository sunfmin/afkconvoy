//! The **daybook**: the operator's one place to read, after the fact, what
//! agents did and decided outside any issue (docs/adr/0071). Its home is a
//! GitHub repo's Discussions, category `Daybook`: one discussion a day,
//! titled with the host's local date, one comment an entry.
//!
//! Two writers of the first entry of a day at once may each find no
//! discussion and each create one. Discussion numbers only grow, so the
//! day's discussion is the one of its title with the lowest number: every
//! entry goes there, and a creator that finds an older one deletes its own,
//! which no entry went to — an entry is only ever written to the lowest
//! numbered discussion its writer saw after any creation of its own.
//!
//! An entry can carry a **lesson**: one sentence its writer thinks should
//! outlast the day. The entries are the lessons' one home; the day's
//! discussion body holds an index of them, derived from its entries —
//! rebuilt whole by each append that writes a lesson, and checked after it
//! is written — so the lessons of a span of days are listed from the days'
//! bodies alone, without fetching an entry. Two lesson writers at once can
//! each write an index the other's lesson is missing from; each checks its
//! own, and the next lesson's rebuild restores any other.
//!
//! GitHub is reached through `GraphQl`, which a program implements with its
//! own client; `Gh` is the crate's own, `gh api graphql` run plainly. What
//! the crate is told is all it knows: the repo whose Discussions hold the
//! daybook, the repo an entry is of, the kind of session that wrote it and
//! the day it goes to. It reads no environment and no list of checkouts.

mod book;
mod day;
mod entry;

pub use book::{Appended, Daybook, Gh, GraphQl, Held, Lesson, Variable, gh_args};
pub use day::{day_at, day_named, next_day, today};
pub use entry::{Indexed, Said, entry, index_of, indexed, lesson_of, one_line};

/// The discussion category every day's discussion is in.
pub const CATEGORY: &str = "Daybook";
