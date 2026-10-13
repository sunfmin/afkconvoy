# The hospital

A team after the one Cockroach Labs describes in [An experiment in running a hospital for code](https://www.cockroachlabs.com/blog/experiment-running-hospital-code/): an issue is a patient, and passes from one role to the next until it is discharged. The Chief — a human — stands in no workflow: an issue reaches them only when a stage cannot decide and no consult could either, or when a breaker trips.

```yaml
escalate_label: ready-for-human
roles:
  fellow: ./roles/fellow
  attending: ./roles/attending
  triage-nurse: ./roles/triage-nurse
  discharge-nurse: ./roles/discharge-nurse
  infection-control: ./roles/infection-control
  research: ./roles/research
  safety: ./roles/safety
precedents: { by: safety, do: record-precedent, max_open: 10 }
```

| Role | What it does | Where |
|---|---|---|
| `fellow` | Works an issue up into a plan, and treats it under the reviewed plan | stages `workup`, `treatment` |
| `attending` | Reviews the plan and the code; rules when the fellow cannot decide | stages `plan-review`, `code-review`; consult |
| `discharge-nurse` | Audits the record before a landing, and lands | stage `discharge`; landing |
| `triage-nurse` | Routes an open issue to the workflow that fits it | patrol `triage` |
| `infection-control` | Runs the gate on the base branch; locks the fleet when it is broken | patrol `rounds` |
| `research` | Proposes work it can show the need for | patrol `research` |
| `safety` | Proposes a human's ruling as a precedent; reviews how the work went | precedents; patrol `safety-review` |

The article's Charge Nurse, who finds a stalled stage and runs it again, has no role here: the fleet does that itself, with its nudge and its restart.
