// What a probe of a DSH model loads in its headless profile, and nothing
// else: the model it names, set on each request as a worker's plugin sets it.
export const name = 'afkconvoy-probe'

export function apply(ctx) {
  ctx.on('agent/request', async (_event, next) => {
    const config = await next()
    const model = process.env.AFKCONVOY_DSH_MODEL
    return model ? { ...config, model } : config
  })
}
