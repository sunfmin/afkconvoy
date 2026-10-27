---
status: accepted
---

# A session file is the turn

A worker has started when its harness takes the brief as its current turn, and it is busy until that same turn ends. Orca's send receipt cannot say so for a harness it does not observe: Grok, pi, Kimi and DSH come back `unsupported`, with no `turn_started` and no `working`. The terminal's screen is the same kind of guess — a phrase added for one harness, missed for the next. Both were tried. The workers were visibly at work and were killed because the receipt said they had not started.

The harness already writes the turn down. Each of the six keeps a session for the worktree: a start, and an end when the turn finishes. That file is the record. The fleet sends the line, then reads the session until the turn is there or the bound passes. A line the terminal did not accept is not a turn, and the file is not consulted. An open turn stays busy however quiet the screen is. A turn the file says ended is idle from that end. No live terminal means the worker is gone, even if a killed session never wrote an end. A child session does not end the worker's turn. After the turn ends, whether a PR exists stays with the claim, as before.
