# chore

For a small change that needs no diagnosis and changes nothing a user of the code would see: a document, a rename, a dependency, a test for behaviour that is already there, the fix of a broken base branch. The issue is its own plan.

```yaml
max_rework: 2
max_consecutive_rework: 2
stages:
  - name: treatment
    by: fellow
    do: treat
    model: sonnet
    escalate: { to: attending, do: consult }
    on: { pass: code-review }
  - name: code-review
    by: attending
    do: review-code
    model: sonnet
    writes: false
    on: { pass: land, reject: treatment }
land: { by: discharge-nurse, do: land }
```
