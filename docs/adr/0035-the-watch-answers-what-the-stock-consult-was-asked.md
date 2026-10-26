---
status: accepted
---

# The watch answers what the stock consult was asked

The stock consult repeated the host faults it could not fix: on this tracker its eight starts answered no escalation, and its own start met the same fault. Its instruction duplicated the host's watch, but without the operator's authority. We remove it (sunfmin/control#96). An unbound repo's escalation and every hand-off from a stage with no named consult now escalate immediately, remaining visible in `waiting_on_human` for the host's next look. No consult claim, issue marker, instruction, guard or ready-label answer handling remains. Old unbound `escalate_to: consult` configs still load, resolving to `human`; neither value starts a consult.

A consult explicitly named by a team stays as ADR-0018 describes, with its product judgment, commands and bounds. Fleet faults still escalate immediately, as ADR-0030 says. The watch answers any escalation with the commands a human uses: rule, hand back, send, close or change blocked-by. Its brief uses `waiting::ANSWERING`, the same list the human and a named consult receive, with each waiting issue's tracker, number and current answer line. It supplies no consult identity: its answer has the same effect and rework reset as a human's. It may fix the cause on the host or in any repo on the checkout list, through the existing gate and landing train.

Both a stock watch and a watch in the operator's directory leave only four kinds with a human: spending money or needing an account, a credential or a person's hands; what cannot be undone; taking a secret or private data off the host; changing what a product is for or who it is for. They act on none and list each question and answer line in their printed record, which `doctor` lists as what the watch did. This amends ADR-0030's watch rule that a human decision is never answered.

This amends ADR-0018's stock consult and default routing, and ADR-0033's temporary retention of the stock consult. The separate inspections remain until their own move to the watch: their fallback worker is the stock inspector, with inspection rules and no consult duties. The host kit calls its choice `stock_inspector` and accepts its former `stock_consult` key on read. A stage's explicit `to: human` still opts its hot worker out of inspection; escalation routing no longer selects an unbound inspection.
