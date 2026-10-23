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
for (const file of ['afkconvoy.mjs', 'afkconvoy.patch.yml']) {
  if (!existsSync(join(kept, file))) {
    console.error(`afkconvoy: DSH guard file ${file} is missing; refusing worker`)
    rmSync(dir, { recursive: true, force: true })
    process.exit(1)
  }
}
const limits = join(role, '.dsh/cordis.patch.yml')
const command = launch + (existsSync(limits) ? ' --patch ' + quote(limits) : '') +
  ' --patch ' + quote(join(kept, 'afkconvoy.patch.yml')) + ' ' + quote(worktree)
const child = spawn('sh', ['-c', command], {
  env: { ...process.env, AFKCONVOY_DSH_READY: ready, AFKCONVOY_DSH_TTY: String(process.stdin.isTTY === true) },
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
    try { process.kill(-child.pid, 'SIGTERM') } catch {}
    setTimeout(() => { try { process.kill(-child.pid, 'SIGKILL') } catch {} }, 1000).unref()
    clearInterval(timer)
  }
}, 25)
for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
  process.on(signal, () => { try { process.kill(-child.pid, signal) } catch {} })
}
child.stdin.on('error', () => {})
child.on('error', (error) => { console.error(error.message); clearInterval(timer); process.exitCode = 1 })
child.on('close', (code) => {
  clearInterval(timer)
  process.stdin.unpipe(child.stdin)
  process.stdin.pause()
  if (process.stdin.isTTY) process.stdin.setRawMode(false)
  rmSync(dir, { recursive: true, force: true })
  process.exitCode = activated ? (code ?? 1) : 1
})
