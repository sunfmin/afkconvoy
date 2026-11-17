---
status: accepted
---

# The watch answers what the stock consult was asked

The stock consult repeated the host faults it could not fix: on this tracker its eight starts answered no escalation, and its own start met the same fault. Its instruction duplicated the host's watch, but without the operator's authority. We remove it (sunfmin/control#96). An unbound repo's escalation and every hand-off from a stage with no named consult now escalate immediately, remaining visible in `waiting_on_human` for the host's next look. No consult claim, issue marker, instruction, guard or ready-label answer handling remains. Old unbound `escalate_to: consult` configs still load, resolving to `human`; neither value starts a consult.

A consult explicitly named by a team stays as ADR-0018 describes, with its product judgment, commands and bounds. Fleet faults still escalate immediately, as ADR-0030 says. The watch answers any escalation with the commands a human uses: rule, hand back, send, close or change blocked-by. Its brief uses `waiting::ANSWERING`, the same list the human and a named consult receive, with each waiting issue's tracker, number and current answer line. It supplies no consult identity: its answer has the same effect and rework reset as a human's. It may fix the cause on the host or in any repo on the checkout list, through the existing gate and a turn.

Both a stock watch and a watch in the operator's directory leave only spending money with a human (ADR-0073). They act on none of it and list each question and answer line in their report (ADR-0038). This amends ADR-0030's watch rule that a human decision is never answered.

This amends ADR-0018's stock consult and default routing. ADR-0035 has already moved inspections into the watch: no fallback inspection worker remains. The host kit accepts old `stock_consult` choices on read and drops them on its next write.
