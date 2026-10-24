// What a probe of a DSH model loads in its headless profile, and nothing
// else: the model it names, set on each request as a worker's plugin sets it,
// and that model printed after the mark afkconvoy reads it by
// (`dsh::ANSWERED`): DSH's headless answer names no model.
export const name = 'afkconvoy-probe'

export function apply(ctx) {
  ctx.on('agent/request', async (_event, next) => {
    const config = await next()
    const model = process.env.AFKCONVOY_DSH_MODEL
    const sent = model ? { ...config, model } : config
    if (sent.model) process.stdout.write(`AFKCONVOY_DSH_ANSWERED ${sent.model}\n`)
    return sent
  })
}
