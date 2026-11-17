---
status: accepted
---

# A repo declares its gate needs

A repo declares the host requirements of its gate in a `yaml gate-needs` block. Each named check runs with `sh -e` in the checkout and has a time limit. The startup probe refuses a host when a check fails. A pass stands for a while; a running fleet instance checks again and ends broken when one fails. A changed declaration is checked at once.

A repo may also declare `yaml cut-needs` checks in the same form. A due cut's needs are checked on this host before it claims the cut; a failure leaves the cut for another host. The tick's evaluation that starts a cut is its check of the cut needs, so no tick both names a failed need and starts the cut. A held cut is one condition, whichever need fails: a look checks every cut need and names each that fails now, and doctor's `cut-need` finding is keyed by the checkout and the version the cut is due for, not by the failing need. A repo without cut needs cuts as before.

## Output stays on the host

A check can read secrets. Its stdout is discarded; on failure only the tail of its stderr is kept, in a user-only file in the host cache outside the checkout, overwritten or removed by the next run. Refusals, plan, tick logs and doctor name the check, its status, that file and the command to fix it, never its output: those may be copied away from the host.

## A need may install itself

A gate or cut need may declare its `install`, with its limit `install_seconds`. Where the check fails, the host runs the install with `sh -e` in the checkout, through the host's exclusive queue as a heavy command, then runs the check again. A pass admits; a fail refuses, naming the check, the install's status and its stderr file. An install keeps the check's output privacy. A failed install is noted on the host and not run again for a while unless the declaration changes, so no loop spends a host. The startup probe, `plan` and `doctor` say when an install ran, how long it took and its result.

A need is the host's wherever it is set off: its check and install run with the host's environment — a fresh login shell's, as the host runner has it — never with the sandbox of the worker whose command started them. A failed install a worker's command set off is not noted on the host, since the sandbox may still hold it; the host runner's own look installs and notes.

The owner gives standing consent for exactly this: "what a repo declares as a gate or cut need, at the version it pins, installed under the user's home. Anything else still asks first." Without it, a host missing a pinned toolchain that another host had stopped its fleets for hours while workers rightly waited for consent. So an install's contract is: it writes only under the user's home, never to system directories and never with sudo, and installs the pinned versions the repo already names, reading them from where the repo keeps them, not from a copy. afkconvoy runs the declared command and cannot check what it writes; keeping to the contract is the repo's.
