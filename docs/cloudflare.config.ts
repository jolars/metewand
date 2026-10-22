import { bindings, defineConfig } from 'cf/config'

export default defineConfig({
  worker: {
    name: 'metewand-docs',
    entrypoint: './worker.mjs',
    env: { ASSETS: bindings.assets() },
    compatibilityDate: '2026-10-22',
    domains: ['metewand.org'],
    workersDev: false,
    previewUrls: false,
    assets: { notFoundHandling: '404-page', runWorkerFirst: true },
    observability: { enabled: true, traces: { enabled: true } },
  },
})
