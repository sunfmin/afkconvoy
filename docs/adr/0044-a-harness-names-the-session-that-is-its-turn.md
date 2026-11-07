---
status: accepted
---

# A harness names the session that is its turn

A worker is busy while the turn its harness took is still open, and stopped once that turn has ended (ADR-0042). The harness already names its flags, its hook events and its paths (ADR-0013). The session that records the turn was still read apart from the harness: each harness's session place, and which lines of it start or end a turn, were known beside the file walk, including a child session that must not end the worker's turn.

That knowledge belongs to the harness. A caller asks the harness for the turn recorded for a worktree. The read still only opens files. It does not name a session place or an event of one harness. The core stays pure (ADR-0007): the harness answers from the text it is given, and opening the file stays outside the core. Orca's receipt and the terminal screen are still not a turn (ADR-0042).

## Considered options

- **Leave the parsers beside the file walk.** The walk and the meaning of a line would change together, and a new harness would teach the reader its events. Refused: what is one harness's stays that harness's, and a test of a line would reach a parser that is not the harness.
- **Have the harness open the session file.** The place and the meaning would be one call. Refused: the core does no I/O, and a file that cannot be read is the effect layer's error.

## Consequences

- A harness that grows a session place or a line that starts or ends a turn changes its own module. The file walk does not. The same holds for the lines that record token use and a file beside the session that keeps them (sunfmin/control#621): the harness reads each line into a usage record, the account adds the records up, and one walk opens the files for a whole session and a window of it.
- A child session is the harness's to name. The fold that skips it stays the turn's.
- A stand-in writes the files the harness describes, so it does not keep a second copy of the place or the lines.
- Claude Code starts a bound worker in its role copy, so its session is filed under that directory's project, not the worktree's. Its harness names the project tree and associates a primary session with the worktree through its first user prompt's exact brief pointer, using the shared brief matcher also used by Codex. A turn's timestamp must still satisfy the start proof's bound; another worker of the same role copy and a child session cannot prove the start. An unbound session still identifies its worktree by `cwd`.
