//! The one interface both of a daybook's stores answer (docs/adr/0118): a
//! host's own database (`Local`) and a GitHub repo's Discussions
//! (`Daybook`). A program writes and reads either the same way, and an
//! entry written to either has the one shape `entry` gives it.

use anyhow::Result;

use crate::book::{Held, Lesson};

/// Where a daybook's entries are kept.
pub trait Store {
    /// Appends `entry` to the day `day`, `YYYY-MM-DD`, written at the epoch
    /// second `at` → where the entry is: the URL of its comment on GitHub,
    /// `daybook:<id>` in a host's database. GitHub stamps its own time and
    /// takes no `at`.
    fn write(&self, day: &str, at: u64, entry: &str) -> Result<String>;

    /// The lessons of the days `since` to `until`, both included (every day
    /// from `since` where `until` is none), oldest day first, each entry's in
    /// the order it was written.
    fn lessons(&self, since: &str, until: Option<&str>) -> Result<Vec<Lesson>>;

    /// What the days `since` to `until`, both included, hold: how many
    /// lessons, and how many entries carry none.
    fn held(&self, since: &str, until: &str) -> Result<Held>;
}
