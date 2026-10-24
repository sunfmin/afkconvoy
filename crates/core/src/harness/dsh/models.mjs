// DSH's provider catalog can be stale. Ask its own server with its own
// credential service, without refreshing, writing or exporting credentials.
export const name = 'afkconvoy-models'
export const inject = ['credentials']

export function apply(ctx) {
  ctx.on('agent/request', async () => {
    let token, key
    let result
    try {
      const record = await ctx.credentials.readRecord('deepseek-account-platform/default')
      token = record?.payload?.token
      if (typeof token !== 'string' || !token.trim()) token = undefined
      if (!token) key = (await ctx.credentials.resolve('DEEPSEEK_API_KEY'))?.value
      if (!token && !key) {
        result = { error: 'not signed in: sign in to DSH or set DEEPSEEK_API_KEY' }
      } else {
        const response = await fetch('https://api.deepseek.com/models', {
          headers: token ? { 'x-dsh-auth-token': token } : { Authorization: `Bearer ${key}` },
          // Never forward a credential to a redirected destination.
          redirect: 'error',
          signal: AbortSignal.timeout(10000),
        })
        const body = await response.text()
        let answer
        try { answer = JSON.parse(body) } catch {}
        if (response.ok && Array.isArray(answer?.data) && answer.data.length &&
            answer.data.every(model => typeof model.id === 'string' && model.id.trim())) {
          result = { models: answer.data.map(({ id, name }) => ({
            id, ...(typeof name === 'string' && name.trim() ? { name } : {}),
          })) }
        } else {
          result = { error: `server answered HTTP ${response.status}: ${body.slice(0, 2000) || 'no model list'}` }
        }
      }
    } catch {
      // An exception may quote a credential or request headers.
      result = { error: 'could not read DSH credentials or ask the model server' }
    }
    let printed = JSON.stringify(result)
    for (const secret of [token, key]) {
      if (secret) {
        printed = printed.replaceAll(JSON.stringify(secret).slice(1, -1), '[redacted]')
      }
    }
    process.stdout.write(`AFKCONVOY_DSH_LISTED ${printed}\n`, () => process.exit(0))
    return new Promise(() => {})
  })
}
