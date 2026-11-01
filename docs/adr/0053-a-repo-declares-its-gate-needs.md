---
status: accepted
---

# A repo declares its gate needs

For sunfmin/control#443, a repo declares the host requirements of its gate in a `yaml gate-needs` block. Each named check runs with `sh -e` in the checkout and has a time limit. The startup probe refuses a host when a check fails. A passing check stands for an hour; a running fleet instance checks again and ends broken when one fails. A changed declaration is checked immediately. Older releases read past the separate block, so they can still read the config during rollout.

A check can read secrets. Its stdout is discarded. On failure, only the last 20 lines of stderr are kept in a user-only file in the host cache, outside the checkout. The next run overwrites or removes that file. Refusals and doctor name the check, status, file and command to fix it, never its output. This follows the operator's ruling on the issue: plan, tick logs, doctor and issues may be copied away from the host.
