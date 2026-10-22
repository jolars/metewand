# Website development

The website at <https://metewand.org> uses VitePress. Its source, npm
dependencies, and Cloudflare deployment configuration live in this directory.

From the repository root, use Node.js 22.18 or later to install dependencies and
start the development server:

```sh
npm --prefix docs ci
npm --prefix docs run dev
```

`devenv shell` includes Node.js. Build the site and validate its Cloudflare
packaging without credentials or uploads:

```sh
npm --prefix docs run deploy:check
```

The generated website is in `docs/.vitepress/dist/`. Preview that build with
`npm --prefix docs run preview`.

## Content

- `index.md` is the homepage.
- `guide/` contains the getting started guide and example index.
- `reference/` contains the implemented interfaces and compatibility contracts.
- `.vitepress/config.ts` defines navigation, search, and site metadata.
- `.vitepress/theme/` extends the default theme.
- `public/` contains static assets.

The favicon files and `site.webmanifest` live directly in `public/`. The shared
`head` configuration in `.vitepress/config.ts` adds their links to every page.

`public/robots.txt` allows crawlers to access the site and points to
`https://metewand.org/sitemap.xml`. VitePress generates the sitemap from the
published pages and copies `robots.txt` to the site root during the build.

The original logos and social preview image live in the repository's `assets/`
directory. The development and build commands copy `logo.svg`, `logo.png`,
`logo-small.png`, and `og.png` into the ignored `public/brand/` directory. Edit
the originals to update the site's branding. The site configuration adds Open
Graph and Twitter card metadata to every page using the shared preview image and
each page's title, description, and URL.

Keep links between published pages relative and include their `.md` extension,
so they work both in GitHub and VitePress. Link to source code, fixtures,
schemas, and runnable examples on GitHub. The build rejects missing page links.
Add new reference pages to the sidebar in `.vitepress/config.ts`.

This README is excluded from the published site.

## Deployment

`cloudflare.config.ts` declares the assets-only `metewand-docs` Worker and its
`metewand.org` custom domain. `wrangler.config.ts` supplies the asset directory
to the Cloudflare CLI's build tool. The CLI and its build tool are pinned by
`package-lock.json`.

The Website workflow builds and validates pull requests. Pushes to `main` that
change the website or its assets also deploy the validated build. Run the
workflow manually on `main` to redeploy. It reads `CLOUDFLARE_ACCOUNT_ID` and
`CLOUDFLARE_API_TOKEN` from GitHub repository secrets only in the deployment
step. The token needs access to deploy Workers and configure the custom domain
in the account that hosts the `metewand.org` Cloudflare zone.

For a manual local deployment, authenticate with `npx cf auth login` from
`docs/`, or set those two environment variables, then run:

```sh
npm --prefix docs run deploy
```
