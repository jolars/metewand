import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import { join, relative } from "node:path";
import { fileURLToPath } from "node:url";
import { load } from "cheerio";
import TurndownService from "turndown";
import { tables } from "turndown-plugin-gfm";

const root = fileURLToPath(new URL("../.vitepress/dist/", import.meta.url));
const converter = new TurndownService({
  headingStyle: "atx",
  codeBlockStyle: "fenced",
});
converter.use(tables);

async function* pages(directory) {
  for (const entry of await readdir(directory, { withFileTypes: true })) {
    const path = join(directory, entry.name);
    if (entry.isDirectory()) yield* pages(path);
    else if (entry.name.endsWith(".html") && entry.name !== "404.html")
      yield path;
  }
}

const routes = {};
for await (const path of pages(root)) {
  const $ = load(await readFile(path, "utf8"));
  const content = $("#VPContent").clone();
  if (!content.length) throw new Error(`Missing page content in ${path}`);
  content
    .find(
      "script, style, nav, aside, footer, button, svg, .header-anchor, .VPDocFooter, .VPDocAside, .lang",
    )
    .remove();
  content.find(".VPHero .name").append(" ");
  content.find("pre").each((_index, element) => {
    const code = $(element).find("code");
    const language = $(element)
      .parent()
      .attr("class")
      ?.match(/(?:^|\s)language-([\w-]+)/)?.[1];
    if (language) code.attr("class", `language-${language}`);
    code.text(code.text());
  });
  const markdown = converter.turndown(content.html()) + "\n";
  const name = relative(root, path).replace(/\.html$/, ".md");
  const target = join(root, "markdown", name);
  await mkdir(join(target, ".."), { recursive: true });
  await writeFile(target, markdown);
  const route = "/" + name.replace(/\.md$/, "").replace(/(^|\/)index$/, "$1");
  routes[route] = `/markdown/${name}`;
}
await writeFile(
  join(root, "markdown-routes.json"),
  JSON.stringify(routes, null, 2) + "\n",
);
console.log(`Generated Markdown for ${Object.keys(routes).length} pages.`);
