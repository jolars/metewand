import { defineConfig } from 'vitepress'

const siteUrl = 'https://metewand.org/'
const description = 'Reproducible, language-neutral computational benchmarks.'
const imageUrl = new URL('brand/og.png', siteUrl).href
const imageAlt = 'Metewand measuring rod logo. Measure. Compare. Reproduce.'

export default defineConfig({
  lang: 'en-US',
  title: 'Metewand',
  description,
  cleanUrls: true,
  sitemap: { hostname: siteUrl },
  head: [
    ['link', { rel: 'icon', type: 'image/png', href: '/favicon-96x96.png', sizes: '96x96' }],
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/favicon.svg' }],
    ['link', { rel: 'shortcut icon', href: '/favicon.ico' }],
    ['link', { rel: 'apple-touch-icon', sizes: '180x180', href: '/apple-touch-icon.png' }],
    ['meta', { name: 'apple-mobile-web-app-title', content: 'Metewand' }],
    ['link', { rel: 'manifest', href: '/site.webmanifest' }],
    ['meta', { name: 'theme-color', content: '#24634a' }],
    ['meta', { property: 'og:type', content: 'website' }],
    ['meta', { property: 'og:site_name', content: 'Metewand' }],
    ['meta', { property: 'og:locale', content: 'en_US' }],
    ['meta', { property: 'og:image', content: imageUrl }],
    ['meta', { property: 'og:image:type', content: 'image/png' }],
    ['meta', { property: 'og:image:width', content: '1200' }],
    ['meta', { property: 'og:image:height', content: '630' }],
    ['meta', { property: 'og:image:alt', content: imageAlt }],
    ['meta', { name: 'twitter:card', content: 'summary_large_image' }],
    ['meta', { name: 'twitter:image', content: imageUrl }],
    ['meta', { name: 'twitter:image:alt', content: imageAlt }],
  ],
  transformHead({ pageData }) {
    const title = pageData.relativePath === 'index.md'
      ? 'Metewand'
      : `${pageData.title} | Metewand`
    const pageDescription = pageData.description || description
    const path = pageData.relativePath
      .replace(/(^|\/)index\.md$/, '$1')
      .replace(/\.md$/, '')

    return [
      ['meta', { property: 'og:title', content: title }],
      ['meta', { property: 'og:description', content: pageDescription }],
      ['meta', { property: 'og:url', content: new URL(path, siteUrl).href }],
      ['meta', { name: 'twitter:title', content: title }],
      ['meta', { name: 'twitter:description', content: pageDescription }],
    ]
  },
  themeConfig: {
    logo: { src: '/brand/logo-small.png', alt: '' },
    nav: [
      { text: 'Guide', link: '/guide/overview', activeMatch: '/guide/' },
      { text: 'Design', link: '/design' },
    ],
    sidebar: [
      {
        text: 'Guide',
        items: [
          { text: 'Overview', link: '/guide/overview' },
          { text: 'Getting started', link: '/guide/getting-started' },
        ],
      },
      {
        text: 'Authoring a benchmark',
        items: [
          { text: '1. Declare the comparison', link: '/guide/authoring/' },
          { text: '2. Write the functions', link: '/guide/authoring/functions' },
          { text: '3. Parameters and repetitions', link: '/guide/authoring/parameters' },
          { text: '4. Scientific validation', link: '/guide/authoring/validation' },
          { text: '5. Run and analyze', link: '/guide/authoring/running' },
          { text: '6. Extend the benchmark', link: '/guide/authoring/extending' },
        ],
      },
      {
        text: 'Further reading',
        items: [
          { text: 'Examples', link: '/guide/examples' },
          { text: 'Design', link: '/design' },
        ],
      },
    ],
    outline: [2, 3],
    search: { provider: 'local' },
    socialLinks: [{ icon: 'github', link: 'https://github.com/jolars/metewand' }],
    editLink: {
      pattern: 'https://github.com/jolars/metewand/edit/main/docs/:path',
      text: 'Edit this page on GitHub',
    },
    footer: {
      message: 'Released under the MIT or Apache-2.0 license.',
      copyright: 'Copyright © 2026 Johan Larsson',
    },
  },
})
