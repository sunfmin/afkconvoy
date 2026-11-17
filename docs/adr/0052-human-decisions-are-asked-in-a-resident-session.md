---
status: accepted
---

# Human decisions are asked in a resident session

A question for a human must not live only in a scrolling Terminal or a one-way notification. Beside the watch's resident session (ADR-0040), the host runner keeps a second resident interactive Agent and Terminal, titled "afkconvoy asks", in the watch's directory. How its Terminal is found again and re-incarnated is ADR-0096's.

A separate session keeps the watch's rounds bounded and gives every human decision one continuing place to talk. Reusing the watch's conversation would mix two lifecycles and let operator discussion hold up host repair.

The recorded report outbox is the asking session's only source of questions. A human wait keeps its `waiting_on_human` identity and a host condition its finding identity, once per observed episode. The pending queue, current question, delivery and settlement are durable, not conversation state: a reworded report or a host runner restart cannot ask the same episode again.

One question is delivered at a time, only when the Terminal's input is ready, with ADR-0040's request key and session-turn proof. Delivery names the host runner as its source and carries the condition, issue link, watch question, answer line and number waiting. Recovery before delivery removes the question; recovery afterwards delivers a withdrawal. Settlement with the link to the operator's words, or the operator putting the question aside, releases the next. Put-aside questions stay pending and are listed on request.

The session ends its asking turn on the question, first and short, so Orca's existing phone push carries it and opens the Terminal when tapped; afkconvoy builds no second channel. The asking session does no watch work. It may clarify, investigate and consult with the operator, then use the same commands as a human with the operator's authority. The operator's quoted words and their time on the issue establish human intent (ADR-0039); the session's own words establish none.

The shared guard refuses Orca terminal input to this Terminal from every worker and the watch, whatever its harness. It reads the asking launch journal's current handle at each call; a title or directory grants no authority. The host runner's own deliveries run outside worker hooks, and the asking session is exempt by its runtime Terminal identity. As in ADR-0012, this stops an Agent that errs, not one that deliberately evades the guard. Doctor and the panel show the asking session, its current question and the pending queue.
