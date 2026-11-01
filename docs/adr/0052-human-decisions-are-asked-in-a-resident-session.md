---
status: accepted
---

# Human decisions are asked in a resident session

Amends ADR-0040 for sunfmin/control#417. The watch keeps its resident session
and independent rounds. Its recorded human entries no longer leave a question
only in a scrolling Terminal or a one-way notification: the host runner keeps
a second resident interactive Agent and Terminal, named “afkconvoy asks”, in
the watch's directory.

The recorded report outbox is the asking session's only source of questions.
A human wait keeps its `waiting_on_human` identity and a host condition its
finding identity, once per observed episode. The pending queue, current
question, delivery and settlement are durable, not conversation state. A
reworded report or a host runner restart cannot ask the same episode again.

One question is delivered at a time, only when the Terminal's input is ready.
ADR-0040's request key and session-turn proof apply. Delivery names the host
runner as its source and carries the condition, issue link, watch question,
answer line and number waiting. Recovery before delivery removes the
question; recovery afterwards delivers a withdrawal. Settlement with the link
to the operator's words, or the operator putting the question aside, releases
the next. Put-aside questions stay pending and are listed on request.

The session ends its asking turn on the question, first and no more than 180
characters. Orca's existing phone push carries that last message and opens
the Terminal when tapped; afkconvoy builds no second channel. The asking
session does no watch work. It may clarify, investigate and consult with the
operator, then use the same commands as a human with the operator's authority.
The operator's quoted words and their time on the issue establish human intent
under ADR-0039; the session's own words establish none.

For sunfmin/control#418, the shared guard refuses `orca terminal send` to
this Terminal from every bound or unbound worker and the watch, through each
harness's hooks, extension or plugin. It reads the asking launch journal's
current handle at each call; a title or directory grants no authority. The
host runner's own deliveries run outside worker hooks, and the asking
session is exempt by its runtime Terminal identity. Other Terminals retain
their existing input behavior. As in ADR-0012, this stops an unattended
Agent that errs, not one that deliberately evades the guard.

A separate session preserves the watch's bounded rounds and gives every human
decision one continuing place to talk. Reusing the watch's conversation would
mix two lifecycles and let operator discussion hold up host repair. Doctor and
the panel expose the asking session, current question and pending queue.

The fake-Orca gate scenarios prove ordering, episode deduplication, recovery,
busy-input safety and push text. The operator ruled on sunfmin/control#417
that the real phone-and-answer check is post-install evidence, not a merge
condition: after a version carrying this is installed, the first real human
decision goes through this session and Felix's quoted, timestamped words on
that issue are its evidence. An agent must never supply that answer.
