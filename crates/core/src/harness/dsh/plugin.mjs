// No DSH imports: a plugin loaded by absolute filename cannot resolve them.
import { spawnSync } from 'node:child_process'
import { readFileSync, existsSync, readdirSync } from 'node:fs'
import { join } from 'node:path'
import http from 'node:http'
export const name = 'afkconvoy'
export const inject = ['systemPrompt']

function fact(name) {
  const value = process.env[name]
  if (!value) throw new Error(`afkconvoy: ${name} is missing; refusing DSH worker`)
  return value
}

// The endpoint is a shell environment file. Read only its literal Orca
// assignments, never execute it or accept a remote HTTP destination.
function endpoint() {
  const env = { ...process.env }
  if (env.ORCA_AGENT_HOOK_ENDPOINT) {
    let text
    try { text = readFileSync(env.ORCA_AGENT_HOOK_ENDPOINT, 'utf8') }
    catch { return env }
    for (const line of text.split(/\r?\n/)) {
      const match = /^(?:export )?(ORCA_AGENT_HOOK_(?:PORT|TOKEN|ENV|VERSION))=(.*)$/.exec(line.trim())
      if (!match) continue
      const value = match[2].replace(/^(['"])(.*)\1$/, '$2')
      env[match[1]] = value
    }
  }
  return env
}

async function report(event, agent) {
  const env = endpoint()
  const port = Number(env.ORCA_AGENT_HOOK_PORT)
  if (!Number.isInteger(port) || port < 1 || port > 65535 || !env.ORCA_AGENT_HOOK_TOKEN) return
  const form = new URLSearchParams({
    paneKey: env.ORCA_PANE_KEY ?? env.ORCA_AGENT_PANE ?? '',
    launchToken: env.ORCA_AGENT_LAUNCH_TOKEN ?? env.ORCA_AGENT_LAUNCH ?? '',
    tabId: env.ORCA_TAB_ID ?? '', worktreeId: env.ORCA_WORKTREE_ID ?? '',
    env: env.ORCA_AGENT_HOOK_ENV ?? '', version: env.ORCA_AGENT_HOOK_VERSION ?? '',
    payload: JSON.stringify({ hook_event_name: event, session_id: agent?.id ?? '', cwd: process.cwd() }),
  }).toString()
  await new Promise(resolve => {
    const req = http.request({ hostname: '127.0.0.1', port, path: '/hook/dsh', method: 'POST',
      headers: { 'Content-Type': 'application/x-www-form-urlencoded',
        'Content-Length': Buffer.byteLength(form), 'X-Orca-Agent-Hook-Token': env.ORCA_AGENT_HOOK_TOKEN },
    }, res => { res.resume(); res.on('end', resolve) })
    req.setTimeout(1500, () => req.destroy())
    req.on('error', resolve)
    req.end(form)
  })
}

function rules(dir, names) {
  for (const name of names) {
    const path = join(dir, name)
    if (existsSync(path)) return `\n${path}:\n${readFileSync(path, 'utf8')}\n`
  }
  return ''
}

function skills(dirs) {
  const named = new Map()
  for (const dir of dirs) {
    if (!existsSync(dir)) continue
    for (const entry of readdirSync(dir).sort()) {
      const path = join(dir, entry, 'SKILL.md')
      if (!existsSync(path) || named.has(entry)) continue
      named.set(entry, `${entry}: ${path}\n${readFileSync(path, 'utf8').split('---')[1] ?? ''}`)
    }
  }
  return '\nAvailable skills (read the named SKILL.md before applying one):\n' + [...named.values()].join('\n')
}

export function apply(ctx) {
  const guard = fact('AFKCONVOY_GUARD')
  const worktree = fact('AFKCONVOY_WORKTREE')
  const team = fact('AFKCONVOY_TEAM_DIR')
  const home = process.env.AFKCONVOY_DSH_KEPT || fact('DSH_HOME')
  const role = process.cwd()
  ctx.on('tools/pre-execute', (exec, next) => {
    const asked = spawnSync('sh', ['-c', guard], {
      cwd: worktree, input: JSON.stringify({ name: exec.name, arguments: exec.arguments }),
      encoding: 'utf8', timeout: 60000,
    })
    if (asked.error || asked.status !== 0) return {
      kind: 'deny', reason: asked.stderr?.trim() || `afkconvoy guard: ${asked.error?.message ?? 'could not be asked'}`,
    }
    return next()
  })
  // agent/status also fires for interrupted turns; turn-stopping does not.
  // Serialize reports and aggregate agents: one idle subagent cannot stop a
  // worker whose parent or another subagent is still running.
  const running = new Set()
  let reports = Promise.resolve()
  const tell = (event, agent) => {
    reports = reports.catch(() => {}).then(() => report(event, agent))
    return reports
  }
  // The fleet waits for tui-idle before sending the first brief. Creation
  // starts idle and emits no status transition, so announce that initial
  // state explicitly. A child created during a turn cannot mark it idle.
  ctx.on('agent/created', ({ agent }) => {
    if (running.size === 0) {
      tell('SessionStart', agent)
      return tell('Stop', agent)
    }
    return reports
  })
  ctx.on('agent/status', ({ agent, status }) => {
    const wasRunning = running.size > 0
    if (status === 'running') running.add(agent.id)
    else if (status === 'idle') running.delete(agent.id)
    const isRunning = running.size > 0
    if (wasRunning !== isRunning) {
      tell(isRunning ? 'UserPromptSubmit' : 'Stop', agent)
    }
    return reports
  })
  ctx.on('agent/disposed', ({ agent }) => {
    if (running.delete(agent.id) && running.size === 0) return tell('Stop', agent)
    return reports
  })
  const text = rules(join(home, 'agents'), ['AGENTS.md']) + rules(home, ['AGENTS.md']) +
    '\nThe brief, role, team and repo rules take precedence over host user rules.\n' +
    rules(role, ['CLAUDE.md']) + rules(team, ['CLAUDE.md']) + rules(worktree, ['AGENTS.md', 'CLAUDE.md']) +
    skills([join(role, '.claude/skills'), join(home, 'host-skills'), join(home, 'host-agents-skills')])
  ctx.systemPrompt.section({ name: 'afkconvoy-rules', order: 0, text })
  ctx.on('agent/request', async (_event, next) => {
    const config = await next()
    const model = process.env.AFKCONVOY_DSH_MODEL
    return model ? { ...config, model } : config
  })
  // Retain the middleware's continuation: ignoring it prevents every turn.
  ctx.on('agent/pre-step', (_event, next) => next())
  // Loader dependencies on this service keep the TUI and agent loop from
  // mounting if import, injection or apply failed. Disposal also withdraws
  // the service, so a plugin unload cannot leave an unguarded worker running.
  ctx.provide('afkconvoyGuard', true)
}
