---
status: accepted
---

# The help is the only manual, and the binary holds all of it

Whoever has the binary and nothing else — no network, no checkout of this repository — can design a team, start a fleet and answer every case that needs a human, by reading from `afkconvoy --help` onward. What that takes is in the binary; what is not in the binary is not documentation.

The reader is an agent. It has the tool because something installed it, it reads what the tool prints, and it does not go looking for a manual on a website. A manual kept as a file beside the code is reachable only by a link, and a link stops working for everybody else the day the repository goes private. The concepts a command's help cannot hold, because they belong to no one command — what a team is, what each kind of rule is for, when a human is called — would be only there.

Those are help topics: `afkconvoy help <topic>`, Markdown as the rest of the help is, and listed in `--help`. A command's help names the topic for a concept it uses, and so does a fault or a finding that a reader may need explained. The glossary is a topic, and is the file `GLOSSARY.md` itself. The worked example is a topic and a set of files the binary writes out, not a listing in a document. A reader who wants the whole manual runs `afkconvoy help all`.

## Considered

- **More in `--help` itself.** It is the way in, and already long. A reader that wants one concept should not be handed all of them.
- **Concepts in the help of the command that uses them most.** A concept then has a home that is an accident, and two commands that use it either repeat it or send the reader to each other.
- **A manual page kept beside the help**, by hand or generated from it. Either is a second copy of every sentence. A generated page checked against the help still broke: candidates that each changed the help merged cleanly as text into a page that no longer matched the merged help, and went red though no PR was wrong.
- **A translation kept by hand.** Nothing can tell when it is behind. An agent that reads the help can say it in any language.
- **The decision records in the binary.** They say why afkconvoy is as it is, for whoever changes it. A topic says what the rule is and what follows for whoever uses it.

## Consequences

- A topic's name is not a command's and not one letter from one: `help <x>` is one namespace.
- The help is checked mechanically: the topics `--help` lists are the topics there are; every command and topic a help text names exists; no help text uses a word the glossary says to avoid; no help text names a document outside the binary, such as `docs/adr/`.
- That the help is *enough* cannot be checked by a machine. It is tried: an agent that has never seen afkconvoy is given the binary and asked for a team that passes `check`, and where it stalls is a defect.
- The README says what afkconvoy is, how to install it, and to run `--help`. It explains no concept.
- The example team is files in the binary, held to `check` by a test, so it cannot rot the way a listing does.
