// What DSH's headless profile loads to say the models of the provider routes
// it holds, and nothing else: each route's models, as DSH lists them with
// its own credentials, one line each after the mark afkconvoy reads them by
// (`dsh::LISTED`). It stops at the run's first request, so no prompt is sent.
// A route with no credential lists none.
export const name = 'afkconvoy-models'
export const inject = ['llm']

export function apply(ctx) {
  ctx.on('agent/request', async () => {
    const lines = []
    for (const { id: provider } of ctx.llm.listProviders()) {
      for (const { id } of await ctx.llm.listModels(provider).catch(() => [])) {
        lines.push(`AFKCONVOY_DSH_LISTED ${id}\n`)
      }
    }
    process.stdout.write(lines.join(''), () => process.exit(0))
    return new Promise(() => {})
  })
}
