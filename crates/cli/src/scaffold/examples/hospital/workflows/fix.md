# fix

For a defect, and for any change in what the code does: anything that needs a diagnosis before it needs code. No code before a plan, no plan without a review.

```yaml
max_rework: 3
max_consecutive_rework: 2
stages:
  - name: workup
    by: fellow
    do: diagnose
    writes: false
    escalate: { to: attending, do: consult }
    on: { pass: plan-review }
  - name: plan-review
    by: attending
    do: review-plan
    writes: false
    on: { pass: treatment, reject: workup }
  - name: treatment
    by: fellow
    do: treat
    escalate: { to: attending, do: consult }
    on: { pass: code-review }
  - name: code-review
    by: attending
    do: review-code
    writes: false
    on: { pass: discharge, reject: treatment }
  - name: discharge
    by: discharge-nurse
    do: audit
    writes: false
    on: { pass: land, reject: treatment }
land: { by: discharge-nurse, do: land }
```
