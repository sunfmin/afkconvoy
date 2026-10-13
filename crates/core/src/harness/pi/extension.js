// afkconvoy's extension for pi (docs/adr/0013): what the fleet adds to a pi
// worker that pi takes on no flag. The tool carries this file, keeps it in
// the fleet's own user directory for pi, and names it on the line that
// starts the worker; nothing else loads it.
//
//   - The guard (docs/adr/0012) is asked before every tool call, with the
//     call as pi describes it. It is decided in `afkconvoy guard`, for every
//     harness: nothing is judged here. A call the guard did not allow is
//     refused, with what the guard said; so is one it could not be asked of.
//   - The rules of the issue's worktree, and the team rules, are loaded as pi
//     loads a rules file. pi finds the role's own, in the directory it starts
//     in; the worktree and the team's directory are others.
//
// It is plain JavaScript and imports nothing of pi's: only what Node has.
import { spawnSync } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import { join } from "node:path"

// The variables the start puts these in (`afkconvoy_core::harness::pi`).
const GUARD_ENV = "AFKCONVOY_GUARD"
const WORKTREE_ENV = "AFKCONVOY_WORKTREE"
const TEAM_ENV = "AFKCONVOY_TEAM_DIR"
// Where a directory keeps its rules: the first of these it has, as pi reads
// the directory it starts in.
const RULES = ["AGENTS.md", "CLAUDE.md"]
// Where a team keeps its rules, for every harness (`afkconvoy_core::team`).
const TEAM_RULES = ["CLAUDE.md"]
// How long the guard is given to answer one call, in milliseconds.
const GUARD_TIMEOUT = 60000

function fact(name) {
  const value = process.env[name]
  if (!value) throw new Error(`afkconvoy: ${name} is not set: nothing is allowed unread`)
  return value
}

export default function (pi) {
  // The guard answers with its exit status: 0 allows the call, and anything
  // else — a refusal, a guard that is not there, one that did not answer in
  // time — refuses it. A handler that throws refuses the call too.
  pi.on("tool_call", (event, ctx) => {
    const described = JSON.stringify({ toolName: event.toolName, input: event.input ?? {} })
    const asked = spawnSync("/bin/sh", ["-c", fact(GUARD_ENV)], {
      input: described,
      cwd: ctx.cwd,
      encoding: "utf8",
      timeout: GUARD_TIMEOUT,
    })
    if (!asked.error && asked.status === 0) return undefined
    const said = (asked.stderr ?? "").trim()
    const failed = asked.error ? `: ${asked.error.message}` : ""
    return { block: true, reason: said || `afkconvoy guard: it could not be asked${failed}` }
  })

  pi.on("before_agent_start", (event) => {
    const loaded = event.systemPromptOptions.contextFiles
    const kept = [
      [fact(WORKTREE_ENV), RULES],
      [fact(TEAM_ENV), TEAM_RULES],
    ]
    for (const [dir, names] of kept) {
      for (const name of names) {
        const path = join(dir, name)
        if (!existsSync(path)) continue
        if (!loaded.some((file) => file.path === path)) {
          loaded.push({ path, content: readFileSync(path, "utf8") })
        }
        break
      }
    }
  })
}
