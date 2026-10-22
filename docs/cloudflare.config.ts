import { defineConfig } from 'cf/config'

export default defineConfig({
  worker: {
    name: 'metewand-docs',
    compatibilityDate: '2026-10-22',
    domains: ['metewand.org'],
    workersDev: false,
    previewUrls: false,
    assets: { notFoundHandling: '404-page' },
  },
})
