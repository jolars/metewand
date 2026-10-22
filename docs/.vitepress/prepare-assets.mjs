import { copyFileSync, mkdirSync } from 'node:fs'

const source = new URL('../../assets/', import.meta.url)
const destination = new URL('../public/brand/', import.meta.url)

mkdirSync(destination, { recursive: true })

for (const name of ['logo.svg', 'logo.png', 'logo-small.png', 'og.png']) {
  copyFileSync(new URL(name, source), new URL(name, destination))
}
