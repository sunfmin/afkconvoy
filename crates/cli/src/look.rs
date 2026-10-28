//! A look begins here (docs/adr/0043).
//!
//! A tick and a fleet-instance start each call [`begin`] once. That forgets
//! every observation a look reads once, so the next read observes again and
//! a later question in the look answers from that read. A further observation
//! is registered in [`ONCE`]: neither call site gains a line.
//!
//! `doctor` is one look for its process and does not call [`begin`], so it
//! does not reset between reads. Host readings stay behind `admission::Readings`.
//! The login stands for the life of the process and is not forgotten here.

/// What a look reads once, forgotten when the next look begins.
const ONCE: &[fn()] = &[
    crate::orca::forget_listing,
    crate::orca::forget_running,
    crate::orca::forget_worktrees,
    crate::gh_quota::forget_note,
    crate::gh::forget_heads,
    #[cfg(test)]
    forget_for_test,
];

/// Begins a look. The next read of each observation a look reads once
/// observes again.
pub fn begin() {
    for forget in ONCE {
        forget();
    }
}

#[cfg(test)]
fn forget_for_test() {
    FURTHER.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}

#[cfg(test)]
static FURTHER: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::sync::atomic::Ordering;

    use serde_json::json;

    /// The listing and the quota note are process-wide. Tests that begin a
    /// look or seed either one take this, so they do not clear each other.
    static LOOK: Mutex<()> = Mutex::new(());

    #[test]
    fn a_tick_and_a_start_begin_a_look_through_one_call() {
        for (name, source) in
            [("tick", include_str!("tick.rs")), ("start", include_str!("instance.rs"))]
        {
            assert_eq!(
                source.matches("crate::look::begin()").count(),
                1,
                "{name} begins a look through one call"
            );
            assert!(
                !source.contains("forget_listing")
                    && !source.contains("forget_note")
                    && !source.contains("forget_heads")
                    && !source.contains("forget_running")
                    && !source.contains("forget_worktrees"),
                "{name} does not reset an observation itself"
            );
        }
        assert!(
            !include_str!("doctor.rs").contains("crate::look::begin()"),
            "doctor is one look and does not reset between reads"
        );
    }

    #[test]
    fn begin_resets_a_further_observation_registered_on_the_look() {
        let _held = LOOK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        FURTHER.store(0, Ordering::SeqCst);
        begin();
        assert_eq!(FURTHER.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn one_look_answers_from_its_read_and_the_next_observes_again() {
        let _held = LOOK.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        let listing = json!({"repos": [{"id": "r1"}]});
        crate::orca::hold_listing(listing.clone());
        assert_eq!(crate::orca::cached_listing(), Some(listing.clone()));
        assert_eq!(
            crate::orca::cached_listing(),
            Some(listing),
            "a later question answers from the repo list already read"
        );
        let dir = PathBuf::from("one-look");
        crate::gh_quota::hold_note(&dir);
        assert!(crate::gh_quota::note_held(&dir));
        crate::gh_quota::hold_rate("github.com");
        assert!(crate::gh_quota::rate_held("github.com"));
        crate::gh::hold_head("acme/dev", "abc");
        assert_eq!(crate::gh::head_held("acme/dev").as_deref(), Some("abc"));
        crate::orca::hold_running(true);
        assert_eq!(crate::orca::running_cached(), Some(true));
        crate::orca::hold_worktrees(json!({"worktrees": []}));
        assert!(crate::orca::worktrees_cached().is_some());
        assert!(
            crate::gh_quota::note_held(&dir),
            "a later question answers from the quota note already read"
        );
        assert_eq!(
            crate::gh::head_held("acme/dev").as_deref(),
            Some("abc"),
            "a later question answers from the HEAD already read"
        );

        begin();

        assert!(
            crate::orca::cached_listing().is_none(),
            "the next read of the repo list observes again"
        );
        assert!(
            !crate::gh_quota::note_held(&dir),
            "the next read of the quota note observes again"
        );
        assert!(!crate::gh_quota::rate_held("github.com"), "the rate-limit body is read again");
        assert!(crate::gh::head_held("acme/dev").is_none(), "the next read of HEAD observes again");
        assert!(crate::orca::running_cached().is_none(), "the next read asks orca again");
        assert!(crate::orca::worktrees_cached().is_none(), "the next read lists worktrees again");
    }
}
