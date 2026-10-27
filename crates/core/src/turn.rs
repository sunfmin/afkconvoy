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
    /// The turn ended. `since` is when it ended.
    Closed { since: u64 },
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
        (_, Some(since)) => Some(Turn::Closed { since }),
        (Some(since), None) => Some(Turn::Open { since }),
        (None, None) => None,
    }
}

/// The turn that speaks for several sessions of one worktree.
/// Any session still open is the worker's turn: a child that ended does
/// not idle a parent that has not. Otherwise the turn that ended last.
pub fn prefer(turns: &[Turn]) -> Option<Turn> {
    let mut open: Option<u64> = None;
    let mut closed: Option<u64> = None;
    for turn in turns {
        match *turn {
            Turn::Open { since } => open = Some(since),
            Turn::Closed { since } => {
                closed = Some(match closed {
                    Some(at) if at > since => at,
                    _ => since,
                });
            }
        }
    }
    if let Some(since) = open {
        Some(Turn::Open { since })
    } else {
        closed.map(|since| Turn::Closed { since })
    }
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
        assert_eq!(latest(&[start(10), end(40)], None), Some(Turn::Closed { since: 40 }));
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
        assert_eq!(latest(&[start(20), end(40)], Some(20)), Some(Turn::Closed { since: 40 }));
        assert_eq!(latest(&[end(40)], None), Some(Turn::Closed { since: 40 }));
    }

    #[test]
    fn any_open_session_beats_a_newer_closed_one() {
        let turns = [Turn::Closed { since: 90 }, Turn::Open { since: 10 }];
        assert_eq!(prefer(&turns), Some(Turn::Open { since: 10 }));
        assert_eq!(
            prefer(&[Turn::Closed { since: 10 }, Turn::Closed { since: 40 }]),
            Some(Turn::Closed { since: 40 })
        );
        assert_eq!(prefer(&[]), None);
    }
}
