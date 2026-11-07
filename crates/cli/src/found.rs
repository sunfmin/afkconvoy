//! What a tick step found besides how it ended (`afkconvoy_core::tick::Outcome`):
//! what the tick's line names, and its log keeps. Each step is handed one of
//! its own, empty, and writes in it as it goes, so a step that fails after
//! it escalated an issue or ended a cut still says so; the tick takes it
//! whichever way the step ended. Nothing in it lives past its tick.

use afkconvoy_core::cut::Held;

use crate::gh::Gone;
use crate::log::What;

/// What one step, or the work of a tick before its pass, found.
#[derive(Debug, Default)]
pub struct Found {
    /// The issues it escalated, each with why in a few words. Every
    /// escalation is made through `judge::escalate_claim`, which is what
    /// adds to it.
    pub escalated: Vec<(u64, String)>,
    /// The issues it found gone — deleted, transferred or never issued —
    /// each with whether a claim on it was released.
    pub forgotten: Vec<(Gone, bool)>,
    /// The landings it asked a turn for again that had stopped at a fleet
    /// lock, each with the locks that turn named, closed since.
    pub after_locks: Vec<(u64, Vec<u64>)>,
    /// The issues a dispatch skipped, each with the open blockers it read
    /// just before the claim.
    pub blocked: Vec<(u64, Vec<u64>)>,
    /// What its start and end of a cut said.
    pub cut_said: Vec<What>,
    /// The cut needs as its start of a cut evaluated them, where it did:
    /// what the tick says in place of its rebuild's look, so no tick both
    /// names a failed need and starts the cut (`cut::start`).
    pub cut_needs: Option<Option<Held>>,
}

impl Found {
    /// Adds what a later step found: a cut's needs evaluated again are the
    /// ones said.
    pub fn extend(&mut self, later: Found) {
        self.escalated.extend(later.escalated);
        self.forgotten.extend(later.forgotten);
        self.after_locks.extend(later.after_locks);
        self.blocked.extend(later.blocked);
        self.cut_said.extend(later.cut_said);
        if later.cut_needs.is_some() {
            self.cut_needs = later.cut_needs;
        }
    }
}
