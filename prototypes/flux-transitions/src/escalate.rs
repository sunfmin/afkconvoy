//! Rule 1 — the release is the last step of an escalation, and each step
//! needs the one before it: status board → comment → relabel → release.
//!
//! The steps already taken are not known at compile time: an escalation cut
//! short is finished by a later tick, which reads from GitHub how far it got.
//! So the state is a runtime value, and the order is a refinement on it.

use flux_rs::attrs::*;

#[derive(Debug)]
#[refined_by(board: bool, commented: bool, relabelled: bool)]
pub struct Escalation {
    issue: u32,
    #[field(bool[board])]
    board: bool,
    #[field(bool[commented])]
    commented: bool,
    #[field(bool[relabelled])]
    relabelled: bool,
}

/// What is left of an escalation once its claim is gone: nothing to act on.
#[derive(Debug)]
pub struct Released {
    #[allow(dead_code)]
    issue: u32,
}

impl Escalation {
    /// How far an escalation got, as read from GitHub. Any combination may
    /// come back; the caller learns which only by looking.
    pub fn read(issue: u32, board: bool, commented: bool, relabelled: bool) -> Escalation {
        Escalation { issue, board, commented, relabelled }
    }

    #[spec(fn(&Self[@e]) -> bool[e.board])]
    pub fn board(&self) -> bool {
        self.board
    }

    #[spec(fn(&Self[@e]) -> bool[e.commented])]
    pub fn commented(&self) -> bool {
        self.commented
    }

    #[spec(fn(self: &mut Self[@e]) ensures self: Self[true, e.commented, e.relabelled])]
    pub fn post_board(&mut self) {
        println!("   board    #{}", self.issue);
        self.board = true;
    }

    #[spec(fn(self: &mut Self[@e], &str) requires e.board ensures self: Self[e.board, true, e.relabelled])]
    pub fn comment(&mut self, reason: &str) {
        println!("   comment  #{}: {reason}", self.issue);
        self.commented = true;
    }

    #[spec(fn(self: &mut Self[@e]) requires e.commented ensures self: Self[e.board, e.commented, true])]
    pub fn relabel(&mut self) {
        println!("   relabel  #{}", self.issue);
        self.relabelled = true;
    }

    /// Consumes the escalation: nothing can be done to it afterwards.
    #[spec(fn(Self{e: e.board && e.commented && e.relabelled}) -> Released)]
    pub fn release(self) -> Released {
        println!("   release  #{}", self.issue);
        Released { issue: self.issue }
    }
}

/// The whole transition, safe to run again on an escalation that was cut
/// anywhere: it does what is left, in order, and releases last.
pub fn escalate(mut e: Escalation, reason: &str) -> Released {
    if !e.board {
        e.post_board();
    }
    if !e.commented {
        e.comment(reason);
    }
    if !e.relabelled {
        e.relabel();
    }
    e.release()
}
