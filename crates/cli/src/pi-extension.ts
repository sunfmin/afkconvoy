// afkconvoy's extension for pi (docs/adr/0013): what puts a worker started
// with pi in its role. The fleet writes this file beside the worker's brief
// and adds it to the start with `--extension`; nothing else loads it.
//
// Claude Code puts a role into effect natively. pi is started with its own
// discovery left out, so this carries the three things a role is:
//   - the guard, asked before every tool call (docs/adr/0012);
//   - the role's own limits: the `PreToolUse` hooks of its
//     `.claude/settings.json`, run as Claude Code runs them;
//   - the rules: `CLAUDE.md` of the role, of the worktree and of the team.
// A call is described to the guard and to a hook in Claude Code's words, so
// both judge a worker the same under either coding agent.
//
// It imports nothing of pi's: only what Node has.
import { spawnSync } from "node:child_process"
import { existsSync, readFileSync } from "node:fs"
import { join } from "node:path"

// The variables the start puts these in (`afkconvoy_core::role`).
const GUARD_ENV = "AFKCONVOY_GUARD"
const WORKTREE_ENV = "AFKCONVOY_WORKTREE"
const TEAM_DIR_ENV = "AFKCONVOY_TEAM_DIR"

// pi's built-in tools, by the names Claude Code calls its own: the guard
// judges `Bash`, and a role's hook matches on these.
const CLAUDE_TOOLS: Record<string, string> = {
  bash: "Bash",
  read: "Read",
  edit: "Edit",
  write: "Write",
  grep: "Grep",
  find: "Glob",
  ls: "LS",
}
// Where a directory keeps its rules: the first of these it has.
const RULES = ["CLAUDE.md", "AGENTS.md"]
// Where a role keeps its own limits.
const SETTINGS = join(".claude", "settings.json")
// The exit code with which a hook refuses a call.
const REFUSED = 2
// How long a role's hook is given when it names no timeout, in seconds:
// Claude Code's own default.
const HOOK_TIMEOUT = 60

type Hook = { matcher?: string; lines: { command: string; timeout: number }[] }
type Asked = { status: number | null; stdout: string; stderr: string }

function fact(name: string): string {
  const value = process.env[name]
  if (!value) throw new Error(`afkconvoy: ${name} is not set: nothing is allowed unread`)
  return value
}

// The tool call, as Claude Code describes one to a `PreToolUse` hook.
function described(toolName: string, input: Record<string, unknown>, cwd: string): string {
  const tool_input = "path" in input ? { ...input, file_path: input.path } : input
  return JSON.stringify({
    hook_event_name: "PreToolUse",
    tool_name: CLAUDE_TOOLS[toolName] ?? toolName,
    tool_input,
    cwd,
  })
}

function ask(line: string, call: string, cwd: string, timeout: number): Asked {
  const ran = spawnSync("/bin/sh", ["-c", line], {
    input: call,
    cwd,
    encoding: "utf8",
    timeout: timeout * 1000,
    env: { ...process.env, CLAUDE_PROJECT_DIR: cwd },
  })
  return { status: ran.error ? null : ran.status, stdout: ran.stdout ?? "", stderr: ran.stderr ?? "" }
}

// Why a hook that exited 0 denies the call all the same, when what it
// printed says so in either of Claude Code's two ways.
function denied(stdout: string): string | undefined {
  let said: any
  try {
    said = JSON.parse(stdout)
  } catch {
    return undefined
  }
  const specific = said?.hookSpecificOutput
  if (specific?.permissionDecision === "deny") {
    return String(specific.permissionDecisionReason ?? "a hook of the role denied the call")
  }
  if (said?.decision === "block") return String(said.reason ?? "a hook of the role blocked the call")
  return undefined
}

// The role's `PreToolUse` hooks. A settings file that cannot be read as one
// throws: the call that asked is refused, and so is every call after it.
function roleHooks(roleDir: string): Hook[] {
  const file = join(roleDir, SETTINGS)
  if (!existsSync(file)) return []
  const entries = JSON.parse(readFileSync(file, "utf8"))?.hooks?.PreToolUse ?? []
  if (!Array.isArray(entries)) throw new Error(`afkconvoy: ${file}: hooks.PreToolUse is no list`)
  return entries.map((entry: any) => ({
    matcher: typeof entry?.matcher === "string" ? entry.matcher : undefined,
    lines: (Array.isArray(entry?.hooks) ? entry.hooks : [])
      .filter((hook: any) => hook?.type === "command" && typeof hook.command === "string")
      .map((hook: any) => ({
        command: hook.command,
        timeout: typeof hook.timeout === "number" ? hook.timeout : HOOK_TIMEOUT,
      })),
  }))
}

// Whether a hook's matcher takes the tool: none, `*`, a name, or a pattern
// that is the whole of it.
function matches(matcher: string | undefined, tool: string): boolean {
  if (!matcher || matcher === "*") return true
  return new RegExp(`^(?:${matcher})$`).test(tool)
}

// The rules of `dir`, under a heading that says whose they are.
function rules(dir: string): string[] {
  for (const name of RULES) {
    const file = join(dir, name)
    if (existsSync(file)) return [`# Rules: ${file}\n\n${readFileSync(file, "utf8").trim()}`]
  }
  return []
}

export default function (pi: any) {
  let hooks: Hook[] | undefined

  // A handler that throws refuses the call: a guard that cannot be asked, or
  // a role whose limits cannot be read, allows nothing.
  pi.on("tool_call", (event: any, ctx: any) => {
    const roleDir: string = ctx.cwd
    const input = (event.input ?? {}) as Record<string, unknown>
    const call = described(event.toolName, input, roleDir)
    const tool = CLAUDE_TOOLS[event.toolName] ?? event.toolName

    // The guard answers with its exit code alone: anything but 0 refuses.
    const guard = ask(fact(GUARD_ENV), call, roleDir, HOOK_TIMEOUT)
    if (guard.status !== 0) {
      const why = guard.stderr.trim() || "afkconvoy guard: it could not be asked"
      return { block: true, reason: why }
    }

    hooks ??= roleHooks(roleDir)
    for (const hook of hooks) {
      if (!matches(hook.matcher, tool)) continue
      for (const line of hook.lines) {
        const ran = ask(line.command, call, roleDir, line.timeout)
        if (ran.status === REFUSED) {
          return { block: true, reason: ran.stderr.trim() || "a hook of the role refused the call" }
        }
        const why = ran.status === 0 ? denied(ran.stdout) : undefined
        if (why) return { block: true, reason: why }
      }
    }
  })

  // pi was told to discover no rules, the host user's own among them: the
  // role's, the worktree's and the team's are the ones a worker works under.
  pi.on("before_agent_start", (event: any, ctx: any) => {
    const dirs = [ctx.cwd, fact(WORKTREE_ENV), fact(TEAM_DIR_ENV)]
    const loaded = dirs.flatMap(rules)
    if (loaded.length === 0) return undefined
    return { systemPrompt: `${event.systemPrompt}\n\n${loaded.join("\n\n")}` }
  })
}
