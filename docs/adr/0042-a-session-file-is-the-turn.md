---
status: accepted
---

# A session file is the turn, and the harness names it

A worker has started when its harness takes the brief as its current turn, and it is busy until that same turn ends. Orca's send receipt cannot say so for a harness it does not observe: Grok, pi, Kimi and DSH come back `unsupported`. The terminal's screen is the same kind of guess — a phrase added for one harness, missed for the next. Both were tried, and workers visibly at work were killed because the receipt said they had not started.

The harness already writes the turn down. Each keeps a session for the worktree: a start, and an end when the turn finishes. That file is the record. The fleet sends the line, then reads the session until the turn is there or the bound passes. A line the terminal did not accept is not a turn, and the file is not consulted. An open turn stays busy however quiet the screen is. A turn the file says ended is idle from that end. No live terminal means the worker is gone, even if a killed session never wrote an end. A child session does not end the worker's turn. Of the primary sessions recorded for one worktree, the one that started last is the turn. After the turn ends, whether a PR exists stays with the claim.

Which file holds a harness's session, and which of its lines start or end a turn — a child session among them — is that harness's own to say, in its module. A caller asks the harness for the turn recorded for a worktree. The harness answers from the text it is given; opening the files stays outside the core (ADR-0007).

## Considered options

- **Leave the parsers beside the file walk.** The walk and the meaning of a line would change together, and a new harness would teach the reader its events. What is one harness's stays that harness's.
- **Have the harness open the session file.** The place and the meaning would be one call, but the core does no I/O, and a file that cannot be read is the effect layer's error.

## Consequences

- A harness that grows a session place or a line that starts or ends a turn changes its own module; the file walk does not. The same holds for the lines that record token use: the harness reads each into a usage record, and the account adds them up.
- A stand-in writes the files the harness describes, so it keeps no second copy of the place or the lines.
- Claude Code starts a bound worker in its role copy, so its session is filed under that directory, not the worktree. Its harness ties a primary session to the worktree by the brief pointer in its first user prompt; another worker of the same role copy and a child session cannot prove the start. An unbound session is tied to its worktree by its `cwd`.
