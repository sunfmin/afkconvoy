# fix

For a defect, and for any change in what the code does: anything that needs a diagnosis before it needs code. No code before a plan, no plan without a review.

```yaml
max_rework: 3
max_consecutive_rework: 2
stages:
  - name: workup
    by: fellow
    do: diagnose
    model: fable
    writes: false
    escalate: { to: attending, do: consult }
    on: { pass: plan-review }
  - name: plan-review
    by: attending
    do: review-plan
    harness: pi
    model: openai-codex/gpt-5.5
    writes: false
    on: { pass: treatment, reject: workup }
  - name: treatment
    by: fellow
    do: treat
    model: opus
    escalate: { to: attending, do: consult }
    on: { pass: code-review }
  - name: code-review
    by: attending
    do: review-code
    model: opus
    writes: false
    on: { pass: discharge, reject: treatment }
  - name: discharge
    by: discharge-nurse
    do: audit
    model: sonnet
    writes: false
    on: { pass: land, reject: treatment }
land: { by: discharge-nurse, do: land }
```
