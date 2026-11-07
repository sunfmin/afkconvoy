//! What git and `gh` say of a fault of GitHub's that asking again may get
//! past: one list of markers per transport, which its classifier reads and
//! the help names (sunfmin/control#628). A marker is written here once.

/// One thing said that marks a fault in transit.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Marker {
    /// What is said, as GitHub or git says it.
    pub(crate) said: &'static str,
    found: Found,
}

/// Where in what was said a marker is looked for.
#[derive(Debug, Clone, Copy)]
enum Found {
    /// Anywhere, in upper or lower case.
    AnyCase,
    /// Anywhere, as written.
    AsWritten,
    /// At the end of a `! [remote rejected]` line.
    Rejected,
}

const REJECTED: &str = "! [remote rejected]";

const fn any_case(said: &'static str) -> Marker {
    Marker { said, found: Found::AnyCase }
}

const fn as_written(said: &'static str) -> Marker {
    Marker { said, found: Found::AsWritten }
}

const fn rejected(said: &'static str) -> Marker {
    Marker { said, found: Found::Rejected }
}

// What more than one transport says.
const UNEXPECTED_EOF: Marker = any_case("unexpected EOF");
const CONNECTION_RESET: Marker = any_case("connection reset");
const TIMEOUT: Marker = any_case("timeout");
const TIMED_OUT: Marker = any_case("timed out");

/// A read of the remote's refs by git that failed in the connection, or
/// past its limit.
pub(crate) const GIT_READ: &[Marker] = &[
    any_case("TLS connect error"),
    UNEXPECTED_EOF,
    any_case("SSL_ERROR_SYSCALL"),
    CONNECTION_RESET,
    any_case("connection refused"),
    any_case("could not resolve host"),
    TIMEOUT,
    TIMED_OUT,
    any_case(crate::git::PAST_LIMIT),
];

/// A push the remote refused for a fault of its own: it could not write the
/// pack, another push held the ref's lock (`(failed)`, `cannot lock ref`,
/// `failed to lock`), or it failed to update the ref with a 5xx of its own
/// (sunfmin/control#593). Nothing was written.
pub(crate) const GIT_PUSH: &[Marker] = &[
    as_written("remote unpack failed"),
    rejected("(failed)"),
    as_written("cannot lock ref"),
    as_written("failed to lock"),
    rejected("(Internal Server Error)"),
];

/// A `gh` call that got no answer, or one cut short. A 5xx is one too, and
/// is told by its status, not by a marker.
pub(crate) const GH: &[Marker] = &[
    any_case("unexpected end of JSON input"),
    UNEXPECTED_EOF,
    any_case(": EOF"),
    CONNECTION_RESET,
    TIMEOUT,
    TIMED_OUT,
];

/// Whether `said` holds one of `markers`.
pub(crate) fn met(markers: &[Marker], said: &str) -> bool {
    let lower = said.to_ascii_lowercase();
    markers.iter().any(|marker| match marker.found {
        Found::AnyCase => lower.contains(&marker.said.to_ascii_lowercase()),
        Found::AsWritten => said.contains(marker.said),
        Found::Rejected => said.lines().any(|line| {
            let line = line.trim();
            line.starts_with(REJECTED) && line.ends_with(marker.said)
        }),
    })
}

/// `markers` as the help names them: `` `a`, `b` or `c` ``.
pub(crate) fn listed(markers: &[Marker]) -> String {
    let named: Vec<String> = markers
        .iter()
        .map(|marker| match marker.found {
            Found::AnyCase | Found::AsWritten => format!("`{}`", marker.said),
            Found::Rejected => format!("`{REJECTED} … {}`", marker.said),
        })
        .collect();
    match named.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} or {last}", rest.join(", ")),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What a transport might say holding `marker`, among other words.
    fn said_with(marker: &Marker) -> String {
        match marker.found {
            Found::AnyCase | Found::AsWritten => {
                format!("fatal: unable to access 'https://github.com/o/r.git/': {}", marker.said)
            }
            Found::Rejected => format!(
                "To https://github.com/o/r.git\n {REJECTED}   4f2a9c1e -> refs/convoy/claim/7 {}\nerror: failed to push some refs",
                marker.said
            ),
        }
    }

    #[test]
    fn a_marker_in_upper_or_lower_case_is_met_where_its_list_says_any_case() {
        for marker in GIT_READ.iter().chain(GH) {
            let said = said_with(marker);
            assert!(met(&[*marker], &said.to_ascii_uppercase()), "{said}");
            assert!(met(&[*marker], &said.to_ascii_lowercase()), "{said}");
        }
    }

    #[test]
    fn a_rejected_marker_is_met_only_at_the_end_of_a_rejected_line() {
        let failed = rejected("(failed)");
        assert!(!met(&[failed], "error: failed to push some refs (failed)"));
        assert!(!met(&[failed], " ! [remote rejected] HEAD -> main (failed) later"));
        assert!(met(&[failed], " ! [remote rejected] HEAD -> main (failed)"));
    }

    #[test]
    fn a_push_marker_is_met_only_as_written() {
        for marker in GIT_PUSH {
            let said = said_with(marker);
            assert!(met(&[*marker], &said), "{said}");
            assert!(!met(&[*marker], &said.to_ascii_uppercase()), "{said}");
        }
    }

    #[test]
    fn the_list_is_named_with_or_before_its_last() {
        assert_eq!(listed(&[]), "");
        assert_eq!(listed(&[TIMEOUT]), "`timeout`");
        assert_eq!(
            listed(&[TIMEOUT, TIMED_OUT, rejected("(failed)")]),
            "`timeout`, `timed out` or `! [remote rejected] … (failed)`"
        );
    }
}
