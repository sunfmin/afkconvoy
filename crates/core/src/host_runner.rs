//! What a `run` does with the host runner it finds, by the release each is
//! of: a newer afkconvoy moves the host runner onto itself, and an older one
//! changes nothing (docs/adr/0023).

/// The host runner a `run` found, against the `run`'s own release.
#[derive(Debug, PartialEq, Eq)]
pub enum Found {
    /// Of the same release: it is handed the checkout, as ever.
    Same,
    /// Of an older release: it is stopped, and one of the `run`'s own started
    /// on the same checkout list.
    Older,
    /// Of a newer release: the `run` changes nothing.
    Newer,
}

/// The host runner of the afkconvoy `theirs`, as a `run` of the afkconvoy
/// `ours` finds it, each version as `--version` says it: a release
/// (`0.6.0`), or a build past one (`0.6.0+11.g9f58035`), which is of that
/// release. Releases are compared number by number. A version whose release
/// is not all numbers is known to be neither older nor newer: `Same`.
pub fn found(ours: &str, theirs: &str) -> Found {
    match (release(ours), release(theirs)) {
        (Some(ours), Some(theirs)) if theirs < ours => Found::Older,
        (Some(ours), Some(theirs)) if theirs > ours => Found::Newer,
        _ => Found::Same,
    }
}

/// The release `version` is of, number by number: `0.6.0+11.g9f58035` is
/// [0, 6, 0]. `None` where a part of it is no number.
fn release(version: &str) -> Option<Vec<u64>> {
    let release = version.split('+').next().unwrap_or(version);
    release.split('.').map(|number| number.parse().ok()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_runner_of_an_older_release_is_older_and_of_a_newer_newer() {
        assert_eq!(found("0.6.0", "0.5.13"), Found::Older);
        assert_eq!(found("0.5.13", "0.6.0"), Found::Newer);
        // number by number, never as text
        assert_eq!(found("0.5.13", "0.5.9"), Found::Older);
        assert_eq!(found("0.10.0", "0.9.99"), Found::Older);
        assert_eq!(found("1.0.0", "0.99.99"), Found::Older);
        assert_eq!(found("0.5.9", "0.5.13"), Found::Newer);
        // a build past a release is of that release
        assert_eq!(found("0.6.0+3.g9f58035", "0.5.13+11.gabcdef0.dirty"), Found::Older);
        assert_eq!(found("0.5.13", "0.6.0+1.g9f58035"), Found::Newer);
    }

    #[test]
    fn a_build_of_the_same_release_is_the_same_and_so_is_one_of_no_release() {
        assert_eq!(found("0.6.0", "0.6.0"), Found::Same);
        assert_eq!(found("0.6.0+11.g9f58035", "0.6.0"), Found::Same);
        assert_eq!(found("0.6.0", "0.6.0+2.g1234567.dirty"), Found::Same);
        for unknown in ["", "unknown", "0.6.x", "0.6.0-rc.1"] {
            assert_eq!(found("0.6.0", unknown), Found::Same, "{unknown:?}");
            assert_eq!(found(unknown, "0.6.0"), Found::Same, "{unknown:?}");
        }
    }
}
