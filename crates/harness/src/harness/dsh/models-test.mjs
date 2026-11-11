// Offline contract test: the real listing plugin, a credential service and an
// HTTP server. Only this test redirects fetch to loopback; production has no
// endpoint override that could receive the host user's credential.
import assert from 'node:assert/strict'
import http from 'node:http'
import { apply, inject } from './models.mjs'
import { apply as probe } from './probe.mjs'
assert.deepEqual(inject, ['credentials'])
const mode = process.argv[2]
const token = 'account-secret-sentinel-396'
const key = 'api-key-secret-sentinel-396'
let probeRequest
probe({
  deepseekAccount: { async getState() { return { status: mode === 'key' ? 'signed-out' : 'credential-stored' } } },
  on(_event, handler) { probeRequest = handler },
})
// Check routing without sending a prompt or exposing the account token.
const probeConfig = await probeRequest({}, async () => ({ provider: 'deepseek-official' }))
assert.equal(probeConfig.provider, mode === 'key' ? 'deepseek-official' : 'deepseek-account')
const server = http.createServer((req, res) => {
  assert.equal(req.url, '/models')
  assert.equal(req.headers['x-dsh-auth-token'], mode === 'key' ? undefined : token)
  assert.equal(req.headers.authorization, mode === 'key' ? `Bearer ${key}` : undefined)
  if (['refused', 'truncated'].includes(mode)) {
    res.writeHead(401)
    res.end(`Authentication Fails ${mode === 'truncated' ? 'x'.repeat(1970) : ''}${token}`)
  } else if (mode === 'malformed') {
    res.end('{"data":[{"name":"missing id"}]}')
  } else if (mode === 'redirect') {
    res.writeHead(302, { Location: 'https://other.example/models' })
    res.end()
  } else {
    res.setHeader('Content-Type', 'application/json')
    res.end(JSON.stringify({ data: [
      { id: 'deepseek-flash', name: 'DeepSeek V4.1 Flash' },
      { id: 'deepseek-v4-pro', name: 'DeepSeek V4 Pro' },
    ] }))
  }
})
await new Promise(resolve => server.listen(0, '127.0.0.1', resolve))
const realFetch = globalThis.fetch
let requests = 0
globalThis.fetch = (url, options) => {
  assert.equal(url, 'https://api.deepseek.com/models')
  assert.equal(options.redirect, 'error')
  requests++
  return realFetch(`http://127.0.0.1:${server.address().port}/models`, options)
}
// The production plugin exits after flushing its marked line. Check its
// output here too, including a refusal that echoes the account token.
const originalWrite = process.stdout.write
process.stdout.write = (printed, flushed) => {
  assert(!printed.includes('account-secret'))
  assert(!printed.includes('api-key-secret'))
  assert.equal(requests, mode === 'none' ? 0 : 1)
  process.stdout.write = originalWrite
  originalWrite.call(process.stdout, printed, flushed)
}
let request
apply({
  credentials: {
    async readRecord(id) {
      assert.equal(id, 'deepseek-account-platform/default')
      return ['none', 'key'].includes(mode) ? undefined : { payload: { token } }
    },
    async resolve(ref) {
      assert.equal(ref, 'DEEPSEEK_API_KEY')
      assert(['none', 'key'].includes(mode), 'account token must take precedence')
      return mode === 'none' ? undefined : { value: key }
    },
  },
  on(event, handler) { assert.equal(event, 'agent/request'); request = handler },
})
await request()
