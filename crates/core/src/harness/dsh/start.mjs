// Keep input away from a session whose guard failed to mount. A fresh worker
// never resumes an existing turn. The plugin acknowledges only after installing
// all listeners; missing files, import errors and apply errors fail closed.
import { spawn } from 'node:child_process'
import { mkdtempSync, existsSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { tmpdir } from 'node:os'
const dir = mkdtempSync(join(tmpdir(), 'afkconvoy-dsh-'))
const ready = join(dir, 'ready')
const quote = text => "'" + text.replaceAll("'", "'\\''") + "'"
const [, , launch, kept, role, worktree] = process.argv
const limits = join(role, '.dsh/cordis.patch.yml')
const command = launch + (existsSync(limits) ? ' --patch ' + quote(limits) : '') +
  ' --patch ' + quote(join(kept, 'afkconvoy.patch.yml')) + ' ' + quote(worktree)
// script supplies a real PTY to the interactive TUI while the supervisor
// withholds input until activation. It is part of the supported Unix hosts.
const scriptArgs = process.platform === 'darwin'
  ? ['-q', '/dev/null', 'sh', '-c', command]
  : ['-q', '-e', '-c', command, '/dev/null']
const child = spawn('script', scriptArgs, {
  env: { ...process.env, AFKCONVOY_DSH_READY: ready },
  stdio: ['pipe', 'inherit', 'inherit'],
  detached: true,
})
let activated = false
const started = Date.now()
const timer = setInterval(() => {
  if (existsSync(ready)) {
    activated = true
    clearInterval(timer)
    if (process.stdin.isTTY) process.stdin.setRawMode(true)
    process.stdin.pipe(child.stdin)
  } else if (Date.now() - started > 30000) {
    console.error('afkconvoy: DSH guard plugin did not activate; refusing worker')
    process.kill(-child.pid, 'SIGTERM')
    clearInterval(timer)
  }
}, 25)
for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(signal, () => { try { process.kill(-child.pid, signal) } catch {} })
}
child.stdin.on('error', () => {})
child.on('error', (error) => { console.error(error.message); process.exitCode = 1 })
child.on('close', (code) => {
  clearInterval(timer)
  process.stdin.unpipe(child.stdin)
  process.stdin.pause()
  if (process.stdin.isTTY) process.stdin.setRawMode(false)
  rmSync(dir, { recursive: true, force: true })
  process.exitCode = activated ? (code ?? 1) : 1
})
