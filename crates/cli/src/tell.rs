//! `afkconvoy tell`: the operator's note to the worker at work on an issue,
//! busy or stopped. The note's one home is a comment on the issue, which a
//! later worker reads in its brief's first step; the worker at work now is
//! typed one line that points there, and the note is told only once its
//! harness's session holds that line as the user's.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};

use crate::fleet::Fleet;
use crate::gh::Comment;
use crate::orca::{Handed, Source, Worktree};

/// What opens every note's comment.
const MARK: &str = "<!--convoy:tell-->";

/// Tells the worker at work under this host's claim of `issue` the note
/// `text`: comments it on the issue — once, however often this is rerun for
/// the same note — then types it at the worker and waits for its harness to
/// take it. With no worker of this host at work on it, it only comments, and
/// says why.
pub fn tell(fleet: &Fleet, host: &str, issue: u64, text: &str) -> Result<Value> {
    let text = text.trim();
    if text.is_empty() {
        bail!("the note is empty: nothing was posted or typed");
    }
    let comment = comment(fleet, issue, text)?;
    let (worktree, terminal) = match worker(fleet, host, issue)? {
        Ok(found) => found,
        Err(Absent { why, host }) => {
            let mut answer = json!({"issue": issue, "action": "commented",
                "comment": comment, "why": why});
            if let Some(host) = host {
                answer["host"] = json!(host);
            }
            return Ok(answer);
        }
    };
    let line = format!("Operator's note (also on the issue: {comment}): {}", one_line(text));
    let since = worktree.handed(None)?.unwrap_or(0);
    let handed = worktree.hand_note(&terminal, &line, &comment, since).with_context(|| {
        format!("issue #{issue}: the note was not told; it is on the issue at {comment}")
    })?;
    worktree.clear_turn()?;
    worktree.record_told(fleet.now, &comment)?;
    let mut answer =
        json!({"issue": issue, "action": "told", "terminal": terminal, "comment": comment});
    match handed {
        Handed::Taken { seconds } => answer["seconds"] = json!(seconds),
        Handed::Already => answer["typed"] = json!(false),
        // A note with no send receipt fails past its bound instead.
        Handed::Queued => answer["queued"] = json!(true),
    }
    Ok(answer)
}

/// Why no worker of this host is at work on the issue, and the host whose
/// claim it is where another one holds it.
#[derive(Debug)]
pub(crate) struct Absent {
    why: String,
    host: Option<String>,
}

fn absent(why: &str) -> Result<Result<(Worktree, String), Absent>> {
    Ok(Err(Absent { why: why.to_string(), host: None }))
}

/// The worktree and terminal of the worker at work under `host`'s claim of
/// `issue`, as `nudge` finds it — but whichever fleet instance of the host
/// holds the claim.
pub(crate) fn worker(
    fleet: &Fleet,
    host: &str,
    issue: u64,
) -> Result<Result<(Worktree, String), Absent>> {
    let Some(claim) = fleet.scan.claim_of(issue) else {
        return absent("it is not claimed: no worker is at work on it");
    };
    let Some(owner) = &claim.owner else {
        return absent("its claim names no owner: no worker is at work on it");
    };
    if owner.host != host {
        return Ok(Err(Absent {
            why: format!(
                "it is claimed by fleet instance {} on host {}: tell it there",
                owner.instance, owner.host
            ),
            host: Some(owner.host.clone()),
        }));
    }
    if crate::dispatch::incomplete(fleet, issue)? {
        return absent("its worker has not taken its brief yet");
    }
    let Some(worktree) = Worktree::of_issue(&Source::of(&fleet.repo)?, issue)? else {
        return absent("it has no worktree on this host: not started, or between stages");
    };
    let Some(terminal) = worktree.terminal()? else {
        return absent("its worktree has no live terminal: no worker is at work on it");
    };
    Ok(Ok((worktree, terminal)))
}

/// The URL of the comment that holds the note: the one an earlier run
/// posted, or a new one.
fn comment(fleet: &Fleet, issue: u64, text: &str) -> Result<String> {
    let body = format!(
        "{MARK}\n**Operator's note** to the worker of this issue, typed at it by `afkconvoy tell`:\n\n{text}"
    );
    let posted = |comments: &[Comment]| {
        comments
            .iter()
            .rev()
            .find(|comment| comment.body.trim() == body.trim())
            .map(|c| c.url.clone())
    };
    if let Some(url) = posted(&fleet.gh.comments(issue)?) {
        return Ok(url);
    }
    fleet.gh.comment(issue, &body)?;
    posted(&fleet.gh.comments(issue)?)
        .with_context(|| format!("issue #{issue}: the note was posted and cannot be read back"))
}

/// The note on one line, as a terminal's input takes it: Enter would submit
/// the rest of it early. The comment keeps its lines.
pub(crate) fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// What `afkconvoy tell --help` says after its one line, which
/// lib.rs's `Command` holds.
pub(crate) const HELP: &str = r#"## When to run it

When the worker at work on an issue must know a decision or correction now. Each tick also tells it later issue comments and body edits automatically. Use it rather than `nudge`, which is the fleet's own line to a stopped worker telling it only to carry on: this is the operator's words, to a worker busy or stopped.

## Who runs it

The operator, in a checkout of the repo, on the host the worker runs on. Name the issue, never an orca terminal.

## What it reads and writes

It posts the note as a comment on the issue first: that comment is the note's one home, which a continuation, a restart or a later stage's worker reads in its brief's first step. Rerun for the same note, it finds that comment and posts no second one.

It then finds the worker at work under this host's claim of the issue, whichever fleet instance of the host holds it, and types one line at its terminal: the note, on one line, with the comment's link. At a worker that is stopped it first checks that the terminal has a harness ready for a prompt: a bare shell receives nothing. At a busy worker it types into the harness's input. Claude Code gives a line typed while it works to its model at the next step. A busy Grok keeps that line in its input queue; once the screen shows the line queued, tell presses send now, which gives it to the model at the next step. A harness that does neither waits, bounded, until its turn ends.

It is told only once the harness's session record holds the line as a message its model was given as the user's. A line seen only in the harness's input queue is not delivered: a harness stopped or dead with it there drops it. It records the delivery in the worktree, beside the nudge, and in the tick log.

## What it answers

{"issue", "action": "told", "terminal", "comment", "seconds"} — seconds from the send to the session record's user entry; "typed": false instead where a rerun found the line already taken.

With no worker at work under this host's claim — the issue is not claimed, its worker has not taken its brief, it has no worktree here (not started, or between stages), no live terminal, or another host holds the claim — it posts the comment, types nothing, and answers {"issue", "action": "commented", "comment", "why"}, with "host" naming the host that holds the claim where another one does.

Errors: past the bound (600 seconds) with the line not taken, it fails with the current screen and the comment's link, and types no second line; rerun it to try again. A terminal with no harness at its input is sent nothing.

## What trips you up

- It never edits the issue's body. Each tick tells later body edits and issue comments to the worker under this host's claim, with session proof, and retries failed deliveries. A line the worker's terminal took but its harness has not given the model yet (a harness running a tool hands over queued input when the tool ends) is recorded queued, no failure, and told by the tick that finds the session holding it; still not taken past tell's bound (600 seconds), it fails each tick. A refused line, or a harness at no input, fails at once. Three consecutive failed ticks make a doctor finding naming the issue and comment. Fleet comments carrying a `<!--convoy:` marker and comments the worker itself posted (identified by its session's write receipts) are skipped. With no worker here, the next worker reads the issue at its start. PR comments are not told, and closed issues stay closed.
- It takes no `--instance`: the claim says whose worker it is, and only a worker on this host can be typed at."#;

#[cfg(test)]
mod tests {
    #[test]
    fn a_note_is_typed_on_one_line() {
        assert_eq!(super::one_line("use v2.\n\n  Then  land."), "use v2. Then land.");
    }
}
