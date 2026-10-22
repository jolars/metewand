import { defineWranglerConfig } from 'wrangler/experimental-config'

export default defineWranglerConfig({
  assetsDirectory: '.vitepress/dist',
  types: { generate: false },
})
