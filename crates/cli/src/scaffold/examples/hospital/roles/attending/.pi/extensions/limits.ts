// An attending's limits under pi: what `.claude/settings.json` refuses under
// Claude Code, in pi's form. pi runs no hook of that file, so each limit is
// written here a second time; change one and change the other.
//
//   - A commit and a push are refused, whatever was asked: see CLAUDE.md.
//   - A `pass` or a `reject` is refused until the skills this stage's skill
//     says to load were read in this session. An `escalate` or a `blocked`
//     is never held up.
import { basename, dirname } from "node:path"

// The skills each of the role's skills says to load before a verdict.
const NEEDED: Record<string, string[]> = {
  "review-plan": ["codebase-design", "single-source-of-truth"],
  "review-code": ["complexity-review", "single-source-of-truth"],
}
// A verdict that passes or rejects, as a worker's brief has it run.
const VERDICT = /afkconvoy[^ ]* verdict( [^;|&]*)? (pass|reject)( |$)/
// Quoted text: a verdict's own text may hold the words looked for.
const QUOTED = /"[^"]*"|'[^']*'/g

export default function (pi) {
  // The skills read so far. pi loads a skill by reading its SKILL.md, and a
  // new session starts with none.
  const loaded = new Set<string>()

  pi.on("tool_call", (event) => {
    const input = event.input ?? {}
    if (event.toolName === "read") {
      const path = String(input.path ?? "")
      if (basename(path) === "SKILL.md") loaded.add(basename(dirname(path)))
      return undefined
    }
    if (event.toolName !== "bash") return undefined
    const command = String(input.command ?? "")
    if (command.includes("git commit") || command.includes("git push")) {
      return {
        block: true,
        reason: "attending: an attending writes no code: say what is wrong in the text of your verdict",
      }
    }
    if (!VERDICT.test(command.replace(QUOTED, ""))) return undefined
    const skill = process.env.AFKCONVOY_SKILL ?? ""
    const missing = (NEEDED[skill] ?? []).filter((name) => !loaded.has(name))
    if (missing.length === 0) return undefined
    return {
      block: true,
      reason:
        `attending: the skill '${skill}' says to load these skills and work by them before a ` +
        `verdict, and this session has not loaded: ${missing.join(" ")}. Read each one's ` +
        `SKILL.md, do the step that names it, then declare.`,
    }
  })
}
