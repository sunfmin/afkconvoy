//! One turn of a harness, folded from the notes its session wrote.
//! A child note — a sidechain, a subagent — never starts or ends the
//! worker's own turn. Nothing here reads a file (docs/adr/0007).

/// A start or an end the session recorded, in epoch seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Note {
    pub marker: Marker,
    pub at: u64,
    /// A child of the worker's turn. It is not the worker's turn.
    pub child: bool,
}

/// Whether a note opens a turn or closes one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    Start,
    End,
}

/// The worker's turn, as its session has it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Turn {
    /// The latest lead note is a start. `since` is when that start was written.
    Open { since: u64 },
    /// The turn ended. `since` is when it ended. `started` is when that turn
    /// started, absent when the start is not in the notes.
    Closed { since: u64, started: Option<u64> },
}

/// The turn the notes leave open or closed.
///
/// Notes are in the order the session wrote them. A child is skipped. A
/// start opens a turn and forgets an end before it. An end closes the start
/// it follows. With `not_before` set, a turn that started earlier is not
/// this send's turn, and an end whose start is not in the notes proves
/// nothing. With `not_before` unset, an end with no start in the notes is
/// still a turn that ended — the start was earlier in a file read whole.
pub fn latest(notes: &[Note], not_before: Option<u64>) -> Option<Turn> {
    let mut started: Option<u64> = None;
    let mut ended: Option<u64> = None;
    for note in notes {
        if note.child {
            continue;
        }
        match note.marker {
            Marker::Start => {
                started = Some(note.at);
                ended = None;
            }
            Marker::End => {
                if started.is_some() || not_before.is_none() {
                    ended = Some(note.at);
                }
            }
        }
    }
    if let Some(bound) = not_before {
        match started {
            Some(start) if start < bound => return None,
            None => return None,
            Some(_) => {}
        }
    }
    match (started, ended) {
        (started_at, Some(since)) => Some(Turn::Closed { since, started: started_at }),
        (Some(since), None) => Some(Turn::Open { since }),
        (None, None) => None,
    }
}

/// The turn that speaks for several primary sessions of one worktree.
///
/// The session whose turn started last is the one the harness took. When
/// that turn has ended, the worker is idle from that end: an earlier
/// primary session that never wrote an end does not keep it busy. When
/// that turn is still open, an earlier session that already ended does not
/// make it idle. A session that names no start cannot outrank one that
/// does. With none named, the turn that ended last remains. A child never
/// reaches here: it is not a primary session.
pub fn prefer(turns: &[Turn]) -> Option<Turn> {
    let mut latest_start: Option<u64> = None;
    for turn in turns {
        if let Some(start) = began(*turn) {
            latest_start = Some(match latest_start {
                Some(at) if at > start => at,
                _ => start,
            });
        }
    }
    let Some(start) = latest_start else {
        return ended_last(turns);
    };
    let mut open = false;
    let mut ended: Option<u64> = None;
    for turn in turns {
        if began(*turn) != Some(start) {
            continue;
        }
        match *turn {
            Turn::Open { .. } => open = true,
            Turn::Closed { since, .. } => {
                ended = Some(match ended {
                    Some(at) if at > since => at,
                    _ => since,
                });
            }
        }
    }
    if open {
        Some(Turn::Open { since: start })
    } else {
        ended.map(|since| Turn::Closed { since, started: Some(start) })
    }
}

/// When this session's turn started, when its notes say so.
fn began(turn: Turn) -> Option<u64> {
    match turn {
        Turn::Open { since } => Some(since),
        Turn::Closed { started, .. } => started,
    }
}

/// The turn that ended last, among sessions that name no start.
fn ended_last(turns: &[Turn]) -> Option<Turn> {
    let mut since: Option<u64> = None;
    for turn in turns {
        if let Turn::Closed { since: at, started: None } = *turn {
            since = Some(match since {
                Some(kept) if kept > at => kept,
                _ => at,
            });
        }
    }
    since.map(|since| Turn::Closed { since, started: None })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn start(at: u64) -> Note {
        Note { marker: Marker::Start, at, child: false }
    }

    fn end(at: u64) -> Note {
        Note { marker: Marker::End, at, child: false }
    }

    #[test]
    fn an_open_turn_stays_open_until_its_own_end() {
        assert_eq!(latest(&[start(10)], None), Some(Turn::Open { since: 10 }));
        assert_eq!(
            latest(&[start(10), end(40)], None),
            Some(Turn::Closed { since: 40, started: Some(10) })
        );
        assert_eq!(
            latest(&[start(10), end(40), start(50)], None),
            Some(Turn::Open { since: 50 }),
            "a later start is a new turn"
        );
    }

    #[test]
    fn a_child_end_does_not_close_the_lead_turn() {
        let mut child = end(30);
        child.child = true;
        assert_eq!(latest(&[start(10), child], None), Some(Turn::Open { since: 10 }));
    }

    #[test]
    fn a_start_before_the_bound_proves_no_new_turn() {
        assert_eq!(latest(&[start(10), end(40)], Some(20)), None);
        assert_eq!(latest(&[end(40)], Some(20)), None, "an end alone is not a new start");
        assert_eq!(
            latest(&[start(20), end(40)], Some(20)),
            Some(Turn::Closed { since: 40, started: Some(20) })
        );
        assert_eq!(latest(&[end(40)], None), Some(Turn::Closed { since: 40, started: None }));
    }

    #[test]
    fn the_primary_session_that_started_last_is_the_turn() {
        let killed = Turn::Open { since: 10 };
        let later = Turn::Closed { since: 90, started: Some(50) };
        assert_eq!(
            prefer(&[killed, later]),
            Some(later),
            "a killed earlier session is not the turn"
        );
        assert_eq!(prefer(&[later, killed]), Some(later));

        let earlier = Turn::Closed { since: 40, started: Some(10) };
        let still_open = Turn::Open { since: 50 };
        assert_eq!(prefer(&[earlier, still_open]), Some(still_open));
        assert_eq!(prefer(&[still_open, earlier]), Some(still_open));

        let ended_later = Turn::Closed { since: 100, started: Some(10) };
        let started_later = Turn::Closed { since: 80, started: Some(50) };
        assert_eq!(
            prefer(&[ended_later, started_later]),
            Some(started_later),
            "idle from the end of the session that started last"
        );
    }

    #[test]
    fn a_session_that_names_no_start_cannot_outrank_one_that_does() {
        let unnamed = Turn::Closed { since: 90, started: None };
        assert_eq!(prefer(&[unnamed, Turn::Open { since: 10 }]), Some(Turn::Open { since: 10 }));
        assert_eq!(
            prefer(&[
                Turn::Closed { since: 10, started: None },
                Turn::Closed { since: 40, started: None },
            ]),
            Some(Turn::Closed { since: 40, started: None })
        );
        assert_eq!(prefer(&[]), None);
    }
}
