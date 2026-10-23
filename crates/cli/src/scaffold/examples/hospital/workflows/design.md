# design

For a question about what the code should do, where the answer is a decision and not a change: the issue asks "how should we…", and nothing is to be built until somebody has said yes. A fellow works the question up into a proposal; the Chief signs it off. Nothing lands: the issues that build what was decided are opened afterwards.

```yaml
stages:
  - name: proposal
    by: fellow
    do: diagnose
    writes: false
    on: { pass: sign-off }
  - name: sign-off
    by: human
    on: { pass: done, reject: proposal }
```
