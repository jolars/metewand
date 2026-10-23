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

`cloudflare.config.ts` declares the `metewand-docs` Worker and its
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

## Markdown for agents

The build converts each published page's rendered content to Markdown, including
homepage features, code blocks, and tables, and strips navigation and controls.
`worker.mjs` serves these static representations when a GET or HEAD request
explicitly accepts `text/markdown` with a positive quality at least as high as
`text/html`. Browsers and requests without an explicit Markdown preference
receive HTML. Both representations include `Vary: Accept`.

This uses the same build-time conversion and Worker approach as LazyMatrix and
does not require Cloudflare's paid Markdown for Agents feature. Static assets,
canonical redirects, and missing-page responses retain their normal behavior.
Markdown has its own ETag for conditional requests.

Preview negotiation locally with `npm --prefix docs run preview:worker` after
building, then request a page:

```sh
curl -i -H 'Accept: text/markdown' http://localhost:8787/guide/getting-started
```

`npm --prefix docs run deploy:check` runs the Markdown integration tests against
the local Workers runtime before validating the deployment package. The regular
VitePress preview serves HTML without negotiation.

On Linux, `devenv shell` configures `MINIFLARE_WORKERD_PATH` to launch the
npm-pinned `workerd` executable through the Nix dynamic loader. This keeps the
runtime version aligned with `package-lock.json` and works on NixOS without
patching `node_modules`. Run `npm ci` before starting the runtime. Outside
devenv, set `MINIFLARE_WORKERD_PATH` to a compatible executable if the npm binary
cannot start.

The integration tests share one local Worker harness, check that the runtime can
start before opening the server, and close the harness even if startup fails.
Each test file has a 60-second timeout so a stalled runtime cannot leave the
check running indefinitely.
