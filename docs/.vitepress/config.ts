import { defineConfig } from 'vitepress'

const siteUrl = 'https://metewand.org/'
const description = 'Reproducible, language-neutral computational benchmarks.'
const imageUrl = new URL('brand/og.png', siteUrl).href
const imageAlt = 'Metewand measuring rod logo. Measure. Compare. Reproduce.'

const reference = [
  {
    text: 'Commands and workflows',
    items: [
      ['authoring-cli', 'Benchmark authoring'],
      ['read-only-cli', 'Checking and planning'],
      ['worker-checks', 'Checking workers'],
      ['run-cli', 'Running benchmarks'],
      ['inspection-cli', 'Status and explanations'],
      ['exports', 'Exporting results'],
      ['machine-output', 'Machine output'],
      ['datamonger', 'Datamonger sources'],
    ],
  },
  {
    text: 'Contracts and identities',
    items: [
      ['problem-contracts', 'Problem contracts'],
      ['public-schemas', 'Public schemas'],
      ['manifest-parsing', 'Manifest parsing'],
      ['parameter-namespaces', 'Parameter namespaces'],
      ['manifest-compatibility', 'Manifest compatibility'],
      ['schema-validation', 'Schema validation'],
      ['canonical-json', 'Canonical JSON'],
      ['domain-records', 'Domain records'],
      ['identities', 'Typed identities'],
      ['manifest-hashing', 'Manifest hashing'],
      ['seeds', 'Seed derivation'],
    ],
  },
  {
    text: 'Planning and scheduling',
    items: [
      ['configuration-expansion', 'Configuration expansion'],
      ['logical-planning', 'Logical planning'],
      ['scheduling', 'Scheduling'],
    ],
  },
  {
    text: 'Sources and artifacts',
    items: [
      ['source-bundles', 'Source bundles'],
      ['local-tree-hashing', 'Local tree hashing'],
      ['source-resolution', 'Source resolution'],
      ['artifact-store', 'Artifact store'],
      ['cache-maintenance', 'Cache maintenance'],
      ['dataset-materialization', 'Dataset materialization'],
    ],
  },
  {
    text: 'Execution and storage',
    items: [
      ['fixed-dataset-run', 'Timing envelopes'],
      ['execution-measurements', 'Execution measurements'],
      ['execution-controls', 'Execution controls'],
      ['worker-reuse', 'Worker reuse and warm-ups'],
      ['worker-environments', 'Worker environments'],
      ['worker-process-transport', 'Worker process transport'],
      ['worker-output-validation', 'Worker output validation'],
      ['durable-publication', 'Durable publication'],
      ['run-store-schema', 'Run store and recovery'],
    ],
  },
  {
    text: 'Worker protocol and SDKs',
    items: [
      ['worker-protocol-framing', 'Protocol framing'],
      ['worker-protocol', 'Worker sessions'],
      ['python-sdk', 'Python SDK'],
      ['r-sdk', 'R SDK'],
      ['sdk-conformance', 'SDK conformance'],
      ['raw-worker-fixtures', 'Raw worker fixtures'],
    ],
  },
]

export default defineConfig({
  lang: 'en-US',
  title: 'Metewand',
  description,
  cleanUrls: true,
  srcExclude: ['README.md'],
  sitemap: { hostname: siteUrl },
  head: [
    ['link', { rel: 'icon', type: 'image/svg+xml', href: '/brand/logo.svg' }],
    ['link', { rel: 'icon', type: 'image/png', sizes: '192x192', href: '/brand/logo-small.png' }],
    ['link', { rel: 'apple-touch-icon', href: '/brand/logo-small.png' }],
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
      { text: 'Guide', link: '/guide/getting-started', activeMatch: '/guide/' },
      { text: 'Reference', link: '/reference/', activeMatch: '/reference/' },
    ],
    sidebar: {
      '/guide/': [
        {
          text: 'Guide',
          items: [
            { text: 'Getting started', link: '/guide/getting-started' },
            { text: 'Examples', link: '/guide/examples' },
          ],
        },
        {
          text: 'SDKs',
          items: [
            { text: 'Python', link: '/reference/python-sdk' },
            { text: 'R', link: '/reference/r-sdk' },
          ],
        },
      ],
      '/reference/': [
        { text: 'Reference', link: '/reference/' },
        ...reference.map(({ text, items }) => ({
          text,
          collapsed: true,
          items: items.map(([slug, text]) => ({ text, link: `/reference/${slug}` })),
        })),
      ],
    },
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
